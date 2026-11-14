use crate::collect::CpuTicks;
use crate::proto::Memory;

pub struct Stat {
    pub total: CpuTicks,
    pub cores: Vec<CpuTicks>,
}

pub fn parse_stat(text: &str) -> Stat {
    let mut total = CpuTicks::default();
    let mut cores = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let Some(label) = fields.next() else { continue };
        if !label.starts_with("cpu") {
            continue;
        }
        let ticks = cpu_ticks(fields);
        if label == "cpu" {
            total = ticks;
        } else {
            cores.push(ticks);
        }
    }
    Stat { total, cores }
}

fn cpu_ticks<'a>(fields: impl Iterator<Item = &'a str>) -> CpuTicks {
    let values: Vec<u64> = fields
        .take(8)
        .map(|field| field.parse().unwrap_or(0))
        .collect();
    let field = |index: usize| values.get(index).copied().unwrap_or(0);
    let idle = field(3) + field(4);
    let busy = field(0) + field(1) + field(2) + field(5) + field(6) + field(7);
    CpuTicks {
        busy,
        total: busy + idle,
    }
}

pub fn parse_meminfo(text: &str) -> Memory {
    let mut total = 0;
    let mut available = 0;
    let mut swap_total = 0;
    let mut swap_free = 0;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let kib: u64 = rest
            .split_ascii_whitespace()
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        match key {
            "MemTotal" => total = kib * 1024,
            "MemAvailable" => available = kib * 1024,
            "SwapTotal" => swap_total = kib * 1024,
            "SwapFree" => swap_free = kib * 1024,
            _ => {}
        }
    }
    Memory {
        total,
        used: total.saturating_sub(available),
        swap_total,
        swap_used: swap_total.saturating_sub(swap_free),
    }
}

pub fn parse_loadavg(text: &str) -> [f32; 3] {
    let mut load = [0.0; 3];
    for (slot, field) in load.iter_mut().zip(text.split_ascii_whitespace()) {
        *slot = field.parse().unwrap_or(0.0);
    }
    load
}

pub fn parse_uptime(text: &str) -> u64 {
    text.split_ascii_whitespace()
        .next()
        .and_then(|secs| secs.parse::<f64>().ok())
        .map_or(0, |secs| secs as u64)
}

pub fn parse_net_dev(text: &str, counts: impl Fn(&str) -> bool) -> (u64, u64) {
    let mut rx = 0;
    let mut tx = 0;
    for line in text.lines().skip(2) {
        let Some((name, stats)) = line.split_once(':') else {
            continue;
        };
        if !counts(name.trim()) {
            continue;
        }
        let fields: Vec<u64> = stats
            .split_ascii_whitespace()
            .map(|field| field.parse().unwrap_or(0))
            .collect();
        rx += fields.first().copied().unwrap_or(0);
        tx += fields.get(8).copied().unwrap_or(0);
    }
    (rx, tx)
}

const SECTOR_BYTES: u64 = 512;

pub fn parse_diskstats(text: &str, counts: impl Fn(&str) -> bool) -> (u64, u64) {
    let mut read = 0;
    let mut written = 0;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        if fields.len() < 10 || !counts(fields[2]) {
            continue;
        }
        read += fields[5].parse::<u64>().unwrap_or(0) * SECTOR_BYTES;
        written += fields[9].parse::<u64>().unwrap_or(0) * SECTOR_BYTES;
    }
    (read, written)
}

pub struct Mount {
    pub device: String,
    pub path: String,
}

pub fn parse_mounts(text: &str) -> Vec<Mount> {
    let mut mounts: Vec<Mount> = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let (Some(device), Some(path)) = (fields.next(), fields.next()) else {
            continue;
        };
        if !device.starts_with("/dev/") || device.starts_with("/dev/loop") {
            continue;
        }
        if mounts.iter().any(|mount| mount.device == device) {
            continue;
        }
        mounts.push(Mount {
            device: device.to_string(),
            path: unescape_mount_path(path),
        });
    }
    mounts
}

fn unescape_mount_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|digits| bytes[i] == b'\\' && digits.iter().all(|d| (b'0'..=b'7').contains(d)));
        match octal {
            Some(digits) => {
                out.push(
                    digits
                        .iter()
                        .fold(0u8, |value, digit| value.wrapping_mul(8) + (digit - b'0')),
                );
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn os_release_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim_matches('"').to_string())
}

pub fn cpu_model(cpuinfo: &str) -> Option<String> {
    cpuinfo.lines().find_map(|line| {
        line.split_once(':')
            .filter(|(key, _)| key.trim() == "model name")
            .map(|(_, value)| value.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  4705 356 584 3699176 23060 0 277 12 0 0
cpu0 1393 280 260 914727 11187 0 191 5 0 0
cpu1 3312 76 324 2784449 11873 0 86 7 0 0
intr 1462898 32 0 0
ctxt 3020120
";

    #[test]
    fn stat_splits_aggregate_and_per_core_ticks() {
        let stat = parse_stat(STAT);
        assert_eq!(
            stat.total,
            CpuTicks {
                busy: 4705 + 356 + 584 + 277 + 12,
                total: 4705 + 356 + 584 + 277 + 12 + 3699176 + 23060
            }
        );
        assert_eq!(stat.cores.len(), 2);
        assert_eq!(stat.cores[1].busy, 3312 + 76 + 324 + 86 + 7);
    }

    #[test]
    fn meminfo_uses_available_memory_for_used() {
        let memory = parse_meminfo(
            "MemTotal:       32768000 kB\nMemFree:         1000000 kB\nMemAvailable:   20000000 kB\nSwapTotal:       2097148 kB\nSwapFree:        2000000 kB\n",
        );
        assert_eq!(memory.total, 32_768_000 * 1024);
        assert_eq!(memory.used, 12_768_000 * 1024);
        assert_eq!(memory.swap_used, 97_148 * 1024);
    }

    #[test]
    fn loadavg_and_uptime_read_leading_fields() {
        assert_eq!(
            parse_loadavg("0.42 1.05 2.50 3/812 12345\n"),
            [0.42, 1.05, 2.5]
        );
        assert_eq!(parse_uptime("356457.12 1400000.55\n"), 356_457);
    }

    #[test]
    fn net_dev_sums_only_counted_interfaces() {
        let text = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 9000      10    0    0    0     0          0         0     9000      10    0    0    0     0       0          0
  eth0: 1000      10    0    0    0     0          0         0     2000      20    0    0    0     0       0          0
tailscale0: 500   5    0    0    0     0          0         0      700       7    0    0    0     0       0          0
";
        assert_eq!(parse_net_dev(text, |name| name == "eth0"), (1000, 2000));
    }

    #[test]
    fn diskstats_converts_sectors_to_bytes() {
        let text = "   8       0 sda 1000 0 2048 0 500 0 4096 0 0 0 0
   8       1 sda1 900 0 1024 0 400 0 2048 0 0 0 0
";
        assert_eq!(
            parse_diskstats(text, |name| name == "sda"),
            (2048 * 512, 4096 * 512)
        );
    }

    #[test]
    fn mounts_keep_real_devices_once_and_unescape_paths() {
        let text = "sysfs /sys sysfs rw 0 0
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
/dev/loop3 /snap/core/1 squashfs ro 0 0
/dev/sdb1 /mnt/media\\040drive ext4 rw 0 0
/dev/nvme0n1p2 /var/lib/docker ext4 rw 0 0
";
        let mounts = parse_mounts(text);
        let paths: Vec<&str> = mounts.iter().map(|mount| mount.path.as_str()).collect();
        assert_eq!(paths, vec!["/", "/mnt/media drive"]);
    }

    #[test]
    fn os_release_and_cpuinfo_names() {
        assert_eq!(
            os_release_name("NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n").as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert_eq!(
            cpu_model("processor\t: 0\nmodel name\t: AMD Ryzen 7 5700U with Radeon Graphics\n")
                .as_deref(),
            Some("AMD Ryzen 7 5700U with Radeon Graphics")
        );
    }
}
