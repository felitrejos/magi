use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::net;
use crate::proto::{self, BasisPoints, FIVE_HOURS, ONE_WEEK, StatusReport, Window, basis_points};

const SEND_WAIT: Duration = Duration::from_millis(300);
const NEW_FILE_MODE: u32 = 0o600;

static TEMP_FILES: AtomicU32 = AtomicU32::new(0);

#[derive(Deserialize, Default)]
struct StatusInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    context_window: Option<ContextWindow>,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct ContextWindow {
    #[serde(default)]
    used_percentage: Option<f64>,
}

#[derive(Deserialize)]
struct RateLimits {
    #[serde(default)]
    five_hour: Option<WindowJson>,
    #[serde(default)]
    seven_day: Option<WindowJson>,
}

#[derive(Deserialize)]
struct WindowJson {
    #[serde(default)]
    used_percentage: Option<f64>,
    #[serde(default)]
    resets_at: Option<u64>,
}

pub fn run() -> io::Result<()> {
    let input: StatusInput = serde_json::from_reader(io::stdin().lock()).unwrap_or_default();
    let (text, report_to_send) = read(input);
    println!("{text}");
    io::stdout().flush()?;
    if let Some(status) = report_to_send {
        report(status);
    }
    Ok(())
}

fn read(input: StatusInput) -> (String, Option<StatusReport>) {
    let context_used = input
        .context_window
        .and_then(|window| window.used_percentage)
        .map(basis_points);
    let windows: Vec<Window> = input
        .rate_limits
        .map(|limits| {
            [
                window(FIVE_HOURS, limits.five_hour.as_ref()),
                window(ONE_WEEK, limits.seven_day.as_ref()),
            ]
            .into_iter()
            .flatten()
            .collect()
        })
        .unwrap_or_default();
    let text = status_text(context_used, &windows);
    let status = input.session_id.map(|session_id| StatusReport {
        session_id,
        context_used,
        windows,
    });
    (text, status)
}

fn report(report: StatusReport) {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let sent = net::status_socket()
            .and_then(|path| proto::write_frame(&mut UnixStream::connect(path)?, &report));
        let _ = done.send(sent);
    });
    if let Ok(Err(error)) = finished.recv_timeout(SEND_WAIT)
        && !matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        )
    {
        eprintln!("magi-overview: {error}");
    }
}

fn window(minutes: u32, json: Option<&WindowJson>) -> Option<Window> {
    let json = json?;
    Some(Window {
        minutes,
        used: basis_points(json.used_percentage?),
        resets_at: json.resets_at?,
    })
}

fn status_text(context_used: Option<BasisPoints>, windows: &[Window]) -> String {
    let shown = |label: &str, used: BasisPoints| format!("{label} {}%", used / 100);
    context_used
        .map(|used| shown("ctx", used))
        .into_iter()
        .chain(
            windows
                .iter()
                .map(|window| shown(&window.label(), window.used)),
        )
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn install() -> io::Result<()> {
    let command = format!(
        "{} statusline",
        shell_quote(&env::current_exe()?.display().to_string())
    );
    edit_settings(|settings| match settings.get("statusLine") {
        Some(existing) if !is_ours(existing) => {
            println!(
                "You already have a Claude Code status line, so it was left as is. Context % and plan limits won't be recorded."
            );
            false
        }
        _ => {
            settings.insert(
                "statusLine".into(),
                json!({ "type": "command", "command": command }),
            );
            println!("magi status line installed. Context % and plan limits now reach the agent.");
            true
        }
    })
}

pub fn uninstall() -> io::Result<()> {
    edit_settings(|settings| {
        let ours = settings.get("statusLine").is_some_and(is_ours);
        if ours {
            settings.remove("statusLine");
            println!("magi status line removed.");
        } else {
            println!("No magi status line to remove.");
        }
        ours
    })
}

fn is_ours(status_line: &Value) -> bool {
    status_line["command"].as_str().is_some_and(|command| {
        command.ends_with("/magi-overview' statusline")
            || command.ends_with("/magi-overview statusline")
            || command.ends_with("/magi' statusline")
            || command.ends_with("/magi statusline")
    })
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn settings_path() -> io::Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
    resolve(Path::new(&home).join(".claude/settings.json"))
}

fn resolve(path: PathBuf) -> io::Result<PathBuf> {
    match fs::canonicalize(&path) {
        Ok(target) => Ok(target),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path),
        Err(error) => Err(error),
    }
}

fn edit_settings(change: impl FnOnce(&mut Map<String, Value>) -> bool) -> io::Result<()> {
    edit_settings_at(&settings_path()?, change)
}

fn edit_settings_at(
    path: &Path,
    change: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> io::Result<()> {
    let before = Snapshot::of(path)?;
    let mut settings = match &before {
        Some(snapshot) => parse_settings(&snapshot.bytes, path)?,
        None => Map::new(),
    };
    if !change(&mut settings) {
        return Ok(());
    }
    let mut text = serde_json::to_string_pretty(&settings).map_err(io::Error::other)?;
    text.push('\n');
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("settings path has no directory"))?;
    fs::create_dir_all(dir)?;
    let mode = before
        .as_ref()
        .map_or(NEW_FILE_MODE, |snapshot| snapshot.mode);
    if let Some(snapshot) = &before {
        replace(
            dir,
            &path.with_extension("json.magi-backup"),
            &snapshot.bytes,
            mode,
        )?;
    }
    let temp = write_new(dir, text.as_bytes(), mode)?;
    let changed = match (&before, Snapshot::of(path)?) {
        (None, None) => false,
        (Some(before), Some(now)) => now.modified != before.modified || now.bytes != before.bytes,
        _ => true,
    };
    if changed {
        fs::remove_file(&temp)?;
        return Err(io::Error::other(
            "Claude Code changed settings.json while magi was editing it; run the command again",
        ));
    }
    fs::rename(&temp, path)
}

fn replace(dir: &Path, path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let temp = write_new(dir, bytes, mode)?;
    fs::rename(&temp, path)
}

fn write_new(dir: &Path, bytes: &[u8], mode: u32) -> io::Result<PathBuf> {
    let path = dir.join(format!(
        ".settings.json.magi-{}-{}",
        std::process::id(),
        TEMP_FILES.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&path)?;
    let written = file
        .set_permissions(fs::Permissions::from_mode(mode))
        .and_then(|()| file.write_all(bytes))
        .and_then(|()| file.sync_all());
    match written {
        Ok(()) => Ok(path),
        Err(error) => {
            let _ = fs::remove_file(&path);
            Err(error)
        }
    }
}

struct Snapshot {
    bytes: Vec<u8>,
    modified: SystemTime,
    mode: u32,
}

impl Snapshot {
    fn of(path: &Path) -> io::Result<Option<Snapshot>> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(Snapshot {
            bytes: fs::read(path)?,
            modified: metadata.modified()?,
            mode: metadata.permissions().mode() & 0o777,
        }))
    }
}

fn parse_settings(bytes: &[u8], path: &Path) -> io::Result<Map<String, Value>> {
    match serde_json::from_slice(bytes) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(io::Error::other(format!(
            "{} is not a JSON object",
            path.display()
        ))),
        Err(error) => Err(io::Error::other(format!(
            "{} is not valid JSON ({error}); fix it first",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_context_and_plan_windows_and_prints_them() {
        let input: StatusInput = serde_json::from_value(json!({
            "session_id": "s",
            "context_window": { "used_percentage": 34.4, "current_usage": null },
            "rate_limits": { "five_hour": { "used_percentage": 12.0, "resets_at": 1_800_000_000u64 },
                             "seven_day": { "used_percentage": null, "resets_at": 1_800_500_000u64 } }
        }))
        .unwrap();
        let (text, report) = read(input);
        assert_eq!(text, "ctx 34% · 5h 12%");
        assert_eq!(
            report,
            Some(StatusReport {
                session_id: "s".into(),
                context_used: Some(3_440),
                windows: vec![Window {
                    minutes: FIVE_HOURS,
                    used: 1_200,
                    resets_at: 1_800_000_000
                }],
            })
        );
        assert_eq!(read(StatusInput::default()), (String::new(), None));
    }

    #[test]
    fn early_status_lines_with_nulls_still_parse() {
        let input: StatusInput = serde_json::from_value(
            json!({ "session_id": "s", "context_window": { "used_percentage": null } }),
        )
        .unwrap();
        assert!(input.context_window.unwrap().used_percentage.is_none());
        assert!(input.rate_limits.is_none());
    }

    #[test]
    fn only_magi_status_lines_count_as_ours() {
        assert!(is_ours(
            &json!({ "command": "'/Users/me/.local/bin/magi' statusline" })
        ));
        assert!(!is_ours(&json!({ "command": "~/.claude/statusline.sh" })));
        assert!(!is_ours(&json!("not an object")));
        assert_eq!(shell_quote("/a b/it's/magi"), "'/a b/it'\\''s/magi'");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("magi-settings-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn edits_keep_other_keys_their_order_and_the_file_mode() {
        let dir = temp_dir("keep");
        let path = dir.join("settings.json");
        fs::write(
            &path,
            "{\n  \"theme\": \"dark\",\n  \"env\": { \"TOKEN\": \"x\" },\n  \"hooks\": {}\n}\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o664)).unwrap();
        edit_settings_at(&path, |settings| {
            settings.insert(
                "statusLine".into(),
                json!({ "type": "command", "command": "'/x/magi' statusline" }),
            );
            true
        })
        .unwrap();
        let settings: Map<String, Value> =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            settings.keys().collect::<Vec<_>>(),
            vec!["theme", "env", "hooks", "statusLine"]
        );
        assert_eq!(settings["hooks"], json!({}));
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let backup = path.with_extension("json.magi-backup");
        assert_eq!((mode(&path), mode(&backup)), (0o664, 0o664));
        assert!(!fs::read_to_string(&backup).unwrap().contains("statusLine"));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_symlinked_settings_file_is_edited_through_the_link() {
        let dir = temp_dir("link");
        let real = dir.join("dotfiles-settings.json");
        let link = dir.join("settings.json");
        fs::write(&real, "{}").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let path = resolve(link.clone()).unwrap();
        edit_settings_at(&path, |settings| {
            settings.insert("statusLine".into(), json!({}));
            true
        })
        .unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(fs::read_to_string(&real).unwrap().contains("statusLine"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_settings_are_refused_and_left_untouched() {
        let dir = temp_dir("invalid");
        let path = dir.join("settings.json");
        fs::write(&path, "{ \"theme\": \"dark\", }").unwrap();
        assert!(edit_settings_at(&path, |_| true).is_err());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{ \"theme\": \"dark\", }"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_must_be_a_json_object() {
        assert!(parse_settings(br#"{"theme":"dark"}"#, Path::new("s")).is_ok());
        assert!(parse_settings(b"[]", Path::new("s")).is_err());
        assert!(parse_settings(b"{ // comment", Path::new("s")).is_err());
    }
}
