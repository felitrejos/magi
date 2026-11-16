use std::io::Write;
use std::process::{Command, Stdio};

use serde::Deserialize;

use super::limits::percent;
use crate::handoff;
use crate::proto::{FIVE_HOURS, ONE_WEEK, Window};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const EXPIRY_MARGIN_MS: u64 = 60_000;

#[derive(Deserialize)]
struct Stored {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OAuth>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuth {
    access_token: String,
    expires_at: u64,
}

#[derive(Deserialize)]
struct Usage {
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
}

#[derive(Deserialize)]
struct UsageWindow {
    utilization: f64,
    resets_at: Option<String>,
}

pub fn ask(now_secs: u64) -> Option<Vec<Window>> {
    let token = access_token(now_secs * 1000)?;
    let curl = handoff::find("curl", &handoff::search_dirs())?;
    let mut child = Command::new(curl)
        .args([
            "-sS",
            "--fail",
            "--connect-timeout",
            "5",
            "--max-time",
            "10",
            "-H",
            "@-",
        ])
        .arg(USAGE_URL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let headers = format!("Authorization: Bearer {token}\nanthropic-beta: {OAUTH_BETA}\n");
    drop(token);
    let written = child
        .stdin
        .take()
        .and_then(|mut stdin| stdin.write_all(headers.as_bytes()).ok());
    drop(headers);
    let output = child.wait_with_output().ok()?;
    written?;
    if !output.status.success() {
        return None;
    }
    windows(&output.stdout, now_secs)
}

fn access_token(now_ms: u64) -> Option<String> {
    usable_token(&stored_credentials()?, now_ms)
}

fn usable_token(stored: &[u8], now_ms: u64) -> Option<String> {
    let stored: Stored = serde_json::from_slice(stored).ok()?;
    let oauth = stored.oauth?;
    (oauth.expires_at > now_ms + EXPIRY_MARGIN_MS).then_some(oauth.access_token)
}

#[cfg(target_os = "macos")]
fn stored_credentials() -> Option<Vec<u8>> {
    use crate::bounded;
    const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
    const READ_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);
    let user = std::env::var("USER").ok()?;
    let output = bounded::output(
        Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                &user,
                "-w",
            ])
            .stdin(Stdio::null()),
        READ_LIMIT,
    )
    .ok()?;
    output.status.success().then_some(output.stdout)
}

#[cfg(not(target_os = "macos"))]
fn stored_credentials() -> Option<Vec<u8>> {
    use std::path::PathBuf;
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".claude")))?;
    std::fs::read(dir.join(".credentials.json")).ok()
}

fn windows(body: &[u8], now_secs: u64) -> Option<Vec<Window>> {
    let usage: Usage = serde_json::from_slice(body).ok()?;
    let window = |minutes: u32, window: Option<UsageWindow>| {
        let window = window?;
        let resets_at = match window.resets_at.as_deref() {
            Some(text) => unix_secs(text)?,
            None => now_secs + u64::from(minutes) * 60,
        };
        Some(Window {
            minutes,
            used: percent(window.utilization),
            resets_at,
        })
    };
    let windows: Vec<Window> = [
        window(FIVE_HOURS, usage.five_hour),
        window(ONE_WEEK, usage.seven_day),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!windows.is_empty()).then_some(windows)
}

pub(crate) fn unix_secs(text: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let zone = text[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone {
        "Z" | "" => 0,
        _ => {
            let sign = match zone.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours = zone.get(1..3)?.parse::<i64>().ok()?;
            let minutes = zone.get(4..6)?.parse::<i64>().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    u64::try_from(secs).ok()
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_with_fractions_and_offsets_become_unix_seconds() {
        assert_eq!(unix_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            unix_secs("2026-11-16T01:29:59.995454+00:00"),
            Some(1_794_792_599)
        );
        assert_eq!(unix_secs("2026-11-16T02:29:59+01:00"), Some(1_794_792_599));
        assert_eq!(unix_secs("2000-02-29T12:00:00-05:30"), Some(951_845_400));
        assert_eq!(unix_secs("not a time"), None);
    }

    #[test]
    fn the_usage_answer_becomes_the_five_hour_and_weekly_windows() {
        let body = br#"{"five_hour":{"utilization":52.0,"resets_at":"2026-11-16T01:29:59.995454+00:00","limit_dollars":null},"seven_day":{"utilization":50.0,"resets_at":null},"seven_day_opus":null}"#;
        assert_eq!(
            windows(body, 1_000),
            Some(vec![
                Window {
                    minutes: FIVE_HOURS,
                    used: 5_200,
                    resets_at: 1_794_792_599,
                },
                Window {
                    minutes: ONE_WEEK,
                    used: 5_000,
                    resets_at: 1_000 + u64::from(ONE_WEEK) * 60,
                },
            ])
        );
        assert_eq!(windows(br#"{"error":"nope"}"#, 1_000), None);
    }

    #[test]
    fn an_expired_or_missing_sign_in_is_never_used() {
        let stored = |json: &str| usable_token(json.as_bytes(), 10_000);
        assert_eq!(
            stored(r#"{"claudeAiOauth":{"accessToken":"a","expiresAt":999999}}"#),
            Some("a".into())
        );
        assert_eq!(
            stored(r#"{"claudeAiOauth":{"accessToken":"a","expiresAt":20000}}"#),
            None
        );
        assert_eq!(stored(r#"{"mcpOAuth":{}}"#), None);
    }
}
