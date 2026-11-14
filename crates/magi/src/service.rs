use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::agent;

const NAME: &str = "magi-agent";

pub fn install(config: &agent::Config) -> io::Result<()> {
    let exe = env::current_exe()?.canonicalize()?;
    let args = vec![
        exe.display().to_string(),
        "agent".into(),
        "--port".into(),
        config.port.to_string(),
        "--interval".into(),
        config.interval.as_millis().to_string(),
    ];
    let path = env::var("PATH").map_err(|_| io::Error::other("PATH is not set"))?;
    platform::install(&args, &path)
}

pub fn uninstall() -> io::Result<()> {
    platform::uninstall()
}

fn home() -> io::Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("HOME is not set"))
}

fn run(program: &str, args: &[&str]) -> io::Result<()> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "{program} {}: {}: {}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn write(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, contents)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    use super::{NAME, home, run, write};

    fn plist_path() -> io::Result<PathBuf> {
        Ok(home()?.join(format!("Library/LaunchAgents/{NAME}.plist")))
    }

    fn domain() -> String {
        format!("gui/{}", unsafe { libc::getuid() })
    }

    pub fn install(args: &[String], path: &str) -> io::Result<()> {
        let plist = plist_path()?;
        let log = home()?.join(format!("Library/Logs/{NAME}.log"));
        write(
            &plist,
            &super::launchd_plist(args, path, &log.display().to_string()),
        )?;
        let _ = run("launchctl", &["bootout", &format!("{}/{NAME}", domain())]);
        run(
            "launchctl",
            &["bootstrap", &domain(), &plist.display().to_string()],
        )?;
        println!(
            "magi agent is running and will start at login.\n  service: {}\n  log:     {}",
            plist.display(),
            log.display()
        );
        Ok(())
    }

    pub fn uninstall() -> io::Result<()> {
        let plist = plist_path()?;
        let _ = run("launchctl", &["bootout", &format!("{}/{NAME}", domain())]);
        match fs::remove_file(&plist) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        println!("magi agent service removed.");
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::env;
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::process::Command;

    use super::{NAME, home, run, write};

    fn unit_path() -> io::Result<PathBuf> {
        Ok(home()?.join(format!(".config/systemd/user/{NAME}.service")))
    }

    pub fn install(args: &[String], path: &str) -> io::Result<()> {
        let unit = unit_path()?;
        write(&unit, &super::systemd_unit(args, path))?;
        let service = format!("{NAME}.service");
        run("systemctl", &["--user", "daemon-reload"])?;
        run("systemctl", &["--user", "enable", &service])?;
        run("systemctl", &["--user", "restart", &service])?;
        println!(
            "magi agent is running and starts with your user session.\n  service: {}\n  log:     journalctl --user -u {NAME}",
            unit.display()
        );
        if !lingering()? {
            let user = env::var("USER").unwrap_or_else(|_| "$USER".into());
            println!(
                "\nIt stops when you log out. To keep it running, run:\n  sudo loginctl enable-linger {user}"
            );
        }
        Ok(())
    }

    pub fn uninstall() -> io::Result<()> {
        let service = format!("{NAME}.service");
        let _ = run("systemctl", &["--user", "disable", "--now", &service]);
        match fs::remove_file(unit_path()?) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        run("systemctl", &["--user", "daemon-reload"])?;
        println!("magi agent service removed.");
        Ok(())
    }

    fn lingering() -> io::Result<bool> {
        let user = env::var("USER").map_err(|_| io::Error::other("USER is not set"))?;
        let output = Command::new("loginctl")
            .args(["show-user", &user, "-p", "Linger"])
            .output()?;
        Ok(String::from_utf8_lossy(&output.stdout).trim() == "Linger=yes")
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launchd_plist(args: &[String], path: &str, log: &str) -> String {
    let arguments: String = args
        .iter()
        .map(|arg| format!("\n    <string>{}</string>", xml(arg)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{NAME}</string>
  <key>ProgramArguments</key>
  <array>{arguments}
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{path}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        path = xml(path),
        log = xml(log),
    )
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn systemd_unit(args: &[String], path: &str) -> String {
    let exec: Vec<String> = args.iter().map(|arg| systemd_quote(arg)).collect();
    format!(
        "[Unit]\nDescription=MAGI agent\nAfter=network-online.target\n\n[Service]\nExecStart={}\nEnvironment={}\nRestart=always\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        exec.join(" "),
        systemd_quote(&format!("PATH={path}")),
    )
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn systemd_quote(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Vec<String> {
        [
            "/Users/me/My Tools/magi",
            "agent",
            "--port",
            "2015",
            "--interval",
            "1000",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn launchd_plist_escapes_paths_and_keeps_the_agent_alive() {
        let plist = launchd_plist(
            &args(),
            "/opt/a&b/bin:/usr/bin",
            "/Users/me/Library/Logs/magi-agent.log",
        );
        assert!(plist.contains("<string>/Users/me/My Tools/magi</string>"));
        assert!(plist.contains("<string>/opt/a&amp;b/bin:/usr/bin</string>"));
        assert!(plist.contains("<key>KeepAlive</key>\n  <true/>"));
        assert!(plist.contains("<string>--interval</string>\n    <string>1000</string>"));
    }

    #[test]
    fn systemd_unit_quotes_arguments_and_escapes_percent() {
        let unit = systemd_unit(&args(), "/home/me/50%/bin:/usr/bin");
        assert!(
            unit.contains("ExecStart=\"/Users/me/My Tools/magi\" \"agent\" \"--port\" \"2015\"")
        );
        assert!(unit.contains("Environment=\"PATH=/home/me/50%%/bin:/usr/bin\""));
        assert!(unit.contains("Restart=always"));
    }
}
