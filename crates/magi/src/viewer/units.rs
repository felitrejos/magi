use std::time::Duration;

const BINARY_UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];

pub fn bytes(value: u64) -> String {
    let mut scaled = value as f64;
    let mut unit = 0;
    while scaled >= 1024.0 && unit < BINARY_UNITS.len() - 1 {
        scaled /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{value}B"),
        _ if scaled < 10.0 => format!("{scaled:.1}{}", BINARY_UNITS[unit]),
        _ => format!("{scaled:.0}{}", BINARY_UNITS[unit]),
    }
}

pub fn count(value: u64) -> String {
    match value {
        0..1_000 => value.to_string(),
        1_000..1_000_000 => compact(value as f64 / 1e3, "K"),
        1_000_000..1_000_000_000 => compact(value as f64 / 1e6, "M"),
        _ => compact(value as f64 / 1e9, "B"),
    }
}

fn compact(value: f64, unit: &str) -> String {
    if value < 10.0 {
        format!("{value:.1}{unit}")
    } else {
        format!("{value:.0}{unit}")
    }
}

pub fn dollars(micros: u64) -> String {
    format!("${:.2}", micros as f64 / 1e6)
}

pub fn model(id: &str) -> String {
    let id = id
        .split('[')
        .next()
        .unwrap_or(id)
        .trim_start_matches("claude-");
    let mut parts = id.split('-');
    let family = parts.next().unwrap_or_default();
    let version: Vec<&str> = parts
        .take_while(|part| part.len() <= 2 && part.chars().all(|c| c.is_ascii_digit()))
        .collect();
    if version.is_empty() {
        family.to_string()
    } else {
        format!("{family} {}", version.join("."))
    }
}

pub fn rate(per_sec: u64) -> String {
    format!("{}/s", bytes(per_sec))
}

pub fn percent(fraction: f32) -> String {
    let percent = if fraction.is_finite() {
        (fraction * 100.0).clamp(0.0, 999.0)
    } else {
        0.0
    };
    format!("{percent:>3.0}%")
}

pub fn duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_compact_binary_units() {
        assert_eq!(bytes(0), "0B");
        assert_eq!(bytes(1023), "1023B");
        assert_eq!(bytes(1536), "1.5K");
        assert_eq!(bytes(12 * 1024 * 1024), "12M");
        assert_eq!(bytes(u64::MAX), "16384P");
        assert_eq!(rate(2048), "2.0K/s");
    }

    #[test]
    fn counts_models_and_dollars_read_naturally() {
        assert_eq!(count(999), "999");
        assert_eq!(count(1_200), "1.2K");
        assert_eq!(count(775_000), "775K");
        assert_eq!(count(149_160_864), "149M");
        assert_eq!(dollars(51_380_000), "$51.38");
        assert_eq!(model("claude-opus-5-5"), "opus 5.5");
        assert_eq!(model("claude-opus-5-5[1m]"), "opus 5.5");
        assert_eq!(model("claude-haiku-4-5-20251001"), "haiku 4.5");
        assert_eq!(model("claude-fable-5"), "fable 5");
        assert_eq!(model(""), "");
    }

    #[test]
    fn percent_is_right_aligned_and_bounded() {
        assert_eq!(percent(0.0), "  0%");
        assert_eq!(percent(0.423), " 42%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(f32::INFINITY), "  0%");
    }

    #[test]
    fn durations_show_the_two_largest_units() {
        assert_eq!(duration(Duration::from_secs(42)), "42s");
        assert_eq!(duration(Duration::from_secs(600)), "10m");
        assert_eq!(duration(Duration::from_secs(3 * 3600 + 120)), "3h 2m");
        assert_eq!(
            duration(Duration::from_secs(4 * 86_400 + 5 * 3600)),
            "4d 5h"
        );
    }
}
