use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

const POLL: Duration = Duration::from_millis(20);
const STDERR_TAIL: usize = 300;

pub fn output(command: &mut Command, limit: Duration) -> io::Result<Output> {
    let deadline = Instant::now() + limit;
    let mut child = spawn(command)?;
    let stdout = collect(child.stdout.take());
    let stderr = collect(child.stderr.take());
    let status = wait(&mut child, deadline)?;
    let left = || deadline.saturating_duration_since(Instant::now());
    match (stdout.recv_timeout(left()), stderr.recv_timeout(left())) {
        (Ok(stdout), Ok(stderr)) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        _ => {
            kill_group(&child);
            Err(timed_out())
        }
    }
}

pub fn json<T: DeserializeOwned>(program: &str, args: &[&str], limit: Duration) -> io::Result<T> {
    let invocation = format!("{program} {}", args.join(" "));
    let named = |error: io::Error| io::Error::new(error.kind(), format!("{invocation}: {error}"));
    let output = output(Command::new(program).args(args), limit).map_err(named)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.trim().chars().rev().take(STDERR_TAIL).collect();
        let tail: String = tail.chars().rev().collect();
        return Err(io::Error::other(format!(
            "{invocation}: {}: {tail}",
            output.status
        )));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{invocation}: {error}"))
    })
}

fn collect(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = sender.send(bytes);
    });
    receiver
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "took too long and was stopped")
}

pub fn spawn(command: &mut Command) -> io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
}

pub fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

pub fn wait(child: &mut Child, deadline: Instant) -> io::Result<std::process::ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            kill_group(child);
            let _ = child.wait();
            return Err(timed_out());
        }
        thread::sleep(POLL);
    }
}

pub fn kill_group(child: &Child) {
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_command_returns_its_output() {
        let output = output(
            Command::new("/bin/sh").args(["-c", "echo out; echo err >&2"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

    #[test]
    fn a_stuck_command_and_what_it_started_are_stopped_at_the_deadline() {
        let started = Instant::now();
        let error = output(
            Command::new("/bin/sh").args(["-c", "sleep 30 & sleep 30"]),
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_command_that_floods_stderr_cannot_deadlock() {
        let output = output(
            Command::new("/bin/sh").args(["-c", "head -c 300000 /dev/zero >&2; echo done"]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(output.stdout, b"done\n");
        assert_eq!(output.stderr.len(), 300_000);
    }

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
