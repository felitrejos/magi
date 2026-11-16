use std::ffi::CStr;
use std::time::Instant;

use crate::proto::{BasisPoints, FULL};

mod cores;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "linux", test))]
mod procfs;
#[cfg(target_os = "macos")]
mod sensors;

#[cfg(target_os = "linux")]
pub use linux::Collector;
#[cfg(target_os = "macos")]
pub use macos::Collector;

#[derive(Clone)]
pub struct SystemInfo {
    pub os: String,
    pub kernel: String,
    pub cpu_model: String,
    pub core_kinds: Vec<crate::proto::CoreKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct CpuTicks {
    pub busy: u64,
    pub total: u64,
}

#[derive(Default)]
pub struct CpuUsage {
    last: Option<(CpuTicks, Vec<CpuTicks>)>,
}

impl CpuUsage {
    pub fn update(
        &mut self,
        total: CpuTicks,
        cores: Vec<CpuTicks>,
    ) -> (BasisPoints, Vec<BasisPoints>) {
        let usage = match &self.last {
            Some((last_total, last_cores)) if last_cores.len() == cores.len() => (
                busy_share(*last_total, total),
                last_cores
                    .iter()
                    .zip(&cores)
                    .map(|(before, after)| busy_share(*before, *after))
                    .collect(),
            ),
            _ => (0, vec![0; cores.len()]),
        };
        self.last = Some((total, cores));
        usage
    }
}

fn busy_share(before: CpuTicks, after: CpuTicks) -> BasisPoints {
    let total = after.total.saturating_sub(before.total);
    if total == 0 {
        return 0;
    }
    let busy = after.busy.saturating_sub(before.busy).min(total);
    (busy * u64::from(FULL) / total) as BasisPoints
}

#[derive(Default)]
pub struct CounterRate {
    last: Option<(Instant, u64)>,
}

impl CounterRate {
    pub fn per_sec(&mut self, now: Instant, value: u64) -> u64 {
        let rate = match self.last {
            Some((then, last)) if value >= last => {
                let elapsed = now.duration_since(then).as_secs_f64();
                if elapsed > 0.0 {
                    ((value - last) as f64 / elapsed) as u64
                } else {
                    0
                }
            }
            _ => 0,
        };
        self.last = Some((now, value));
        rate
    }
}

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    let status = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if status != 0 {
        return String::new();
    }
    CStr::from_bytes_until_nul(&buf)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(all(test, target_os = "macos"))]
fn on_virtual_mac() -> bool {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "kern.hv_vmm_present"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim() == "1"
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn cpu_usage_is_zero_until_a_second_reading() {
        let mut usage = CpuUsage::default();
        let (total, cores) = usage.update(
            CpuTicks {
                busy: 50,
                total: 100,
            },
            vec![CpuTicks {
                busy: 50,
                total: 100,
            }],
        );
        assert_eq!((total, cores), (0, vec![0]));
    }

    #[test]
    fn cpu_usage_is_the_busy_share_of_elapsed_ticks() {
        let mut usage = CpuUsage::default();
        usage.update(
            CpuTicks {
                busy: 100,
                total: 1_000,
            },
            vec![
                CpuTicks {
                    busy: 0,
                    total: 500,
                },
                CpuTicks {
                    busy: 100,
                    total: 500,
                },
            ],
        );
        let (total, cores) = usage.update(
            CpuTicks {
                busy: 400,
                total: 2_000,
            },
            vec![
                CpuTicks {
                    busy: 250,
                    total: 1_000,
                },
                CpuTicks {
                    busy: 150,
                    total: 1_000,
                },
            ],
        );
        assert_eq!(total, 3_000);
        assert_eq!(cores, vec![5_000, 1_000]);
    }

    #[test]
    fn cpu_usage_restarts_when_the_core_count_changes() {
        let mut usage = CpuUsage::default();
        usage.update(CpuTicks { busy: 1, total: 2 }, vec![CpuTicks::default(); 2]);
        let (_, cores) = usage.update(
            CpuTicks { busy: 9, total: 20 },
            vec![CpuTicks::default(); 3],
        );
        assert_eq!(cores, vec![0, 0, 0]);
    }

    #[test]
    fn counter_rate_divides_by_elapsed_time_and_survives_resets() {
        let start = Instant::now();
        let mut rate = CounterRate::default();
        assert_eq!(rate.per_sec(start, 1_000), 0);
        assert_eq!(
            rate.per_sec(start + Duration::from_millis(500), 2_000),
            2_000
        );
        assert_eq!(rate.per_sec(start + Duration::from_millis(1_000), 10), 0);
        assert_eq!(rate.per_sec(start + Duration::from_millis(2_000), 110), 100);
    }
}
