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
        })
    }
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
