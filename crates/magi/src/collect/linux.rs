use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::mem::MaybeUninit;
use std::path::Path;
use std::time::Instant;

use super::{CounterRate, CpuUsage, SystemInfo, procfs};
use crate::proto::{DiskIo, Filesystem, NetIo, Sample};

struct ProcFile {
    file: File,
    text: String,
}

impl ProcFile {
    fn open(path: &str) -> io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            text: String::new(),
        })
    }

    fn read(&mut self) -> io::Result<&str> {
        self.file.seek(SeekFrom::Start(0))?;
        self.text.clear();
        self.file.read_to_string(&mut self.text)?;
        Ok(&self.text)
    }
}

pub struct Collector {
    stat: ProcFile,
    meminfo: ProcFile,
    loadavg: ProcFile,
    uptime: ProcFile,
    net_dev: ProcFile,
    diskstats: ProcFile,
    mounts: ProcFile,
    cpu: CpuUsage,
    disk_read: CounterRate,
    disk_write: CounterRate,
    net_rx: CounterRate,
    net_tx: CounterRate,
    sensors: Vec<std::path::PathBuf>,
}

impl Collector {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            stat: ProcFile::open("/proc/stat")?,
            meminfo: ProcFile::open("/proc/meminfo")?,
            loadavg: ProcFile::open("/proc/loadavg")?,
            uptime: ProcFile::open("/proc/uptime")?,
            net_dev: ProcFile::open("/proc/net/dev")?,
            diskstats: ProcFile::open("/proc/diskstats")?,
            mounts: ProcFile::open("/proc/self/mounts")?,
            cpu: CpuUsage::default(),
            sensors: cpu_sensors(Path::new("/sys/class/hwmon")),
            disk_read: CounterRate::default(),
            disk_write: CounterRate::default(),
            net_rx: CounterRate::default(),
            net_tx: CounterRate::default(),
        })
    }

    pub fn info(&self) -> SystemInfo {
        let os = fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| procfs::os_release_name(&text));
        let cpu_model = fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| procfs::cpu_model(&text));
        SystemInfo {
            os: os.unwrap_or_else(|| "Linux".to_string()),
            kernel: format!("Linux {}", kernel_release()),
            cpu_model: cpu_model.unwrap_or_default(),
        }
    }

    pub fn sample(&mut self) -> io::Result<Sample> {
        let now = Instant::now();
        let stat = procfs::parse_stat(self.stat.read()?);
        let (cpu, cores) = self.cpu.update(stat.total, stat.cores);
        let (rx, tx) = procfs::parse_net_dev(self.net_dev.read()?, is_physical_interface);
        let (read, written) = procfs::parse_diskstats(self.diskstats.read()?, is_physical_disk);
        Ok(Sample {
            uptime_secs: procfs::parse_uptime(self.uptime.read()?),
            load: procfs::parse_loadavg(self.loadavg.read()?),
            cpu,
            cores,
            memory: procfs::parse_meminfo(self.meminfo.read()?),
            filesystems: procfs::parse_mounts(self.mounts.read()?)
                .iter()
                .filter_map(|mount| filesystem(&mount.path))
                .collect(),
            disk: DiskIo {
                read_per_sec: self.disk_read.per_sec(now, read),
                write_per_sec: self.disk_write.per_sec(now, written),
            },
            network: NetIo {
                rx_per_sec: self.net_rx.per_sec(now, rx),
                tx_per_sec: self.net_tx.per_sec(now, tx),
            },
            sessions: Vec::new(),
            plans: Vec::new(),
            notices: Vec::new(),
            forwards: Vec::new(),
            processes: Vec::new(),
            temperature: hottest(&self.sensors),
        })
    }
}

const CPU_SENSORS: [&str; 5] = [
    "k10temp",
    "coretemp",
    "zenpower",
    "cpu_thermal",
    "soc_thermal",
];

fn cpu_sensors(hwmon: &Path) -> Vec<std::path::PathBuf> {
    let Ok(chips) = std::fs::read_dir(hwmon) else {
        return Vec::new();
    };
    chips
        .flatten()
        .map(|chip| chip.path())
        .filter(|chip| {
            std::fs::read_to_string(chip.join("name"))
                .is_ok_and(|name| CPU_SENSORS.contains(&name.trim()))
        })
        .flat_map(|chip| std::fs::read_dir(chip).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("temp") && name.ends_with("_input"))
        })
        .collect()
}

fn hottest(sensors: &[std::path::PathBuf]) -> Option<f32> {
    sensors
        .iter()
        .filter_map(|path| millidegrees(&std::fs::read_to_string(path).ok()?))
        .reduce(f32::max)
}

fn millidegrees(text: &str) -> Option<f32> {
    let value: f32 = text.trim().parse().ok()?;
    (0.0..150_000.0).contains(&value).then_some(value / 1000.0)
}

fn is_physical_interface(name: &str) -> bool {
    Path::new("/sys/class/net")
        .join(name)
        .join("device")
        .exists()
}

fn is_physical_disk(name: &str) -> bool {
    Path::new("/sys/block").join(name).join("device").exists()
}

#[allow(
    clippy::useless_conversion,
    reason = "statvfs block counts are 32-bit on 32-bit targets"
)]
fn filesystem(path: &str) -> Option<Filesystem> {
    let c_path = CString::new(path).ok()?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    if stats.f_blocks == 0 {
        return None;
    }
    let block = u64::from(stats.f_frsize);
    let used = u64::from(stats.f_blocks).saturating_sub(u64::from(stats.f_bfree)) * block;
    Some(Filesystem {
        mount: path.to_string(),
        total: used + u64::from(stats.f_bavail) * block,
        used,
    })
}

fn kernel_release() -> String {
    let mut name = MaybeUninit::<libc::utsname>::uninit();
    if unsafe { libc::uname(name.as_mut_ptr()) } != 0 {
        return String::new();
    }
    let name = unsafe { name.assume_init() };
    unsafe { CStr::from_ptr(name.release.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hottest_cpu_sensor_is_read_and_nonsense_is_skipped() {
        let dir = std::env::temp_dir().join(format!("magi-hwmon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (chip, name, readings) in [
            ("hwmon0", "nvme", vec!["61000"]),
            ("hwmon1", "k10temp", vec!["48250", "52500", "garbage"]),
        ] {
            let chip = dir.join(chip);
            std::fs::create_dir_all(&chip).unwrap();
            std::fs::write(chip.join("name"), format!("{name}\n")).unwrap();
            for (index, reading) in readings.iter().enumerate() {
                std::fs::write(chip.join(format!("temp{}_input", index + 1)), reading).unwrap();
            }
        }
        let sensors = cpu_sensors(&dir);
        assert_eq!(sensors.len(), 3);
        assert_eq!(hottest(&sensors), Some(52.5));
        assert_eq!(millidegrees("999999"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
