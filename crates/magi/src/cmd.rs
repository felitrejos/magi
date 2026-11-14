use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

const STDERR_TAIL: usize = 300;

pub fn json<T: DeserializeOwned>(program: &str, args: &[&str], timeout: Duration) -> io::Result<T> {
    let invocation = format!("{program} {}", args.join(" "));
    let deadline = Instant::now() + timeout;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| io::Error::new(error.kind(), format!("{invocation}: {error}")))?;
    let stdout = drain(child.stdout.take().expect("stdout is piped"));
    let stderr = drain(child.stderr.take().expect("stderr is piped"));
    let timed_out = || {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{invocation}: timed out after {timeout:?}"),
        )
    };
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err(timed_out());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let remaining = || {
        deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(50))
    };
    let output = stdout
        .recv_timeout(remaining())
        .map_err(|_| timed_out())??;
    if !status.success() {
        let stderr = stderr
            .recv_timeout(remaining())
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        let tail = String::from_utf8_lossy(&stderr[stderr.len().saturating_sub(STDERR_TAIL)..])
            .trim()
            .to_string();
        return Err(io::Error::other(format!("{invocation}: {status}: {tail}")));
    }
    serde_json::from_slice(&output).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{invocation}: {error}"))
    })
}

fn drain(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let _ = sender.send(pipe.read_to_end(&mut output).map(|_| output));
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_from_a_successful_command() {
        let value: serde_json::Value = json(
            "sh",
            &["-c", "echo '{\"ok\": true}'"],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(value["ok"], true);
    }

    #[test]
    fn reports_the_tail_of_stderr_when_a_command_fails() {
        let error = json::<serde_json::Value>(
            "sh",
            &["-c", "echo 'tailscale is stopped' >&2; exit 1"],
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("tailscale is stopped"),
            "{error}"
        );
    }

    #[test]
    fn a_hung_command_is_killed_at_the_deadline() {
        let started = Instant::now();
        let error =
            json::<serde_json::Value>("sleep", &["3"], Duration::from_millis(300)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_grandchild_holding_the_pipe_cannot_outlive_the_deadline() {
        let started = Instant::now();
        let error = json::<serde_json::Value>(
            "sh",
            &["-c", "sleep 3 & echo '{}'"],
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_missing_program_names_itself() {
        let error = json::<serde_json::Value>("magi-no-such-tool", &[], Duration::from_secs(1))
            .unwrap_err();
        assert!(error.to_string().contains("magi-no-such-tool"));
    }
}
