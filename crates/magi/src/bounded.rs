use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(20);

pub fn output(command: &mut Command, limit: Duration) -> io::Result<Output> {
    let mut child = spawn(command)?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait(&mut child, Instant::now() + limit)?;
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
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
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "took too long and was stopped",
            ));
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
}
