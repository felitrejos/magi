use std::ffi::{CStr, CString};
use std::io;
use std::mem::{self, MaybeUninit};
use std::ptr;
use std::slice;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use core_foundation::base::{CFType, TCFType, kCFAllocatorDefault};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use io_kit_sys::types::io_iterator_t;
use io_kit_sys::{
    IOIteratorNext, IOObjectRelease, IORegistryEntryCreateCFProperty, IOServiceGetMatchingServices,
    IOServiceMatching, kIOMasterPortDefault,
};

use super::{CounterRate, CpuTicks, CpuUsage, SystemInfo};
use crate::proto::{DiskIo, Filesystem, Memory, NetIo, Sample};

pub struct Collector {
    host: libc::mach_port_t,
    page_size: u64,
    memory_total: u64,
    route_buf: Vec<u8>,
    ticks: TickCounters,
    cpu: CpuUsage,
    disk_read: CounterRate,
    disk_write: CounterRate,
    net_rx: CounterRate,
    net_tx: CounterRate,
    sensors: Option<super::sensors::Sensors>,
}

impl Collector {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            host: unsafe { mach2::mach_init::mach_host_self() },
            page_size: unsafe { mach2::vm_page_size::vm_kernel_page_size } as u64,
            memory_total: sysctl_value::<u64>("hw.memsize")?,
            route_buf: Vec::new(),
            ticks: TickCounters::default(),
            cpu: CpuUsage::default(),
            disk_read: CounterRate::default(),
            disk_write: CounterRate::default(),
            net_rx: CounterRate::default(),
            net_tx: CounterRate::default(),
            sensors: super::sensors::Sensors::new(),
        })
    }

    pub fn info(&self) -> SystemInfo {
        SystemInfo {
            os: format!(
                "macOS {}",
                sysctl_string("kern.osproductversion").unwrap_or_default()
            ),
            kernel: format!(
                "Darwin {}",
                sysctl_string("kern.osrelease").unwrap_or_default()
            ),
            cpu_model: sysctl_string("machdep.cpu.brand_string").unwrap_or_default(),
        }
    }

    pub fn sample(&mut self) -> io::Result<Sample> {
        let now = Instant::now();
        let (total, cores) = self.cpu_ticks()?;
        let (cpu, cores) = self.cpu.update(total, cores);
        let (read, written) = disk_bytes()?;
        let (rx, tx) = self.network_bytes()?;
        Ok(Sample {
            uptime_secs: uptime_secs()?,
            load: load_average(),
            cpu,
            cores,
            memory: self.memory()?,
            filesystems: filesystems(),
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
            temperature: self
                .sensors
                .as_ref()
                .and_then(super::sensors::Sensors::hottest_die),
        })
    }

    fn cpu_ticks(&mut self) -> io::Result<(CpuTicks, Vec<CpuTicks>)> {
        let mut count: libc::natural_t = 0;
        let mut info: libc::processor_info_array_t = ptr::null_mut();
        let mut info_count: libc::mach_msg_type_number_t = 0;
        let status = unsafe {
            libc::host_processor_info(
                self.host,
                libc::PROCESSOR_CPU_LOAD_INFO,
                &mut count,
                &mut info,
                &mut info_count,
            )
        };
        if status != libc::KERN_SUCCESS {
            return Err(io::Error::other(format!(
                "host_processor_info failed with {status}"
            )));
        }
        let loads = unsafe {
            slice::from_raw_parts(info as *const libc::processor_cpu_load_info, count as usize)
        };
        let cores = self.ticks.advance(loads.iter().map(|load| load.cpu_ticks));
        unsafe {
            libc::vm_deallocate(
                mach2::traps::mach_task_self(),
                info as libc::vm_address_t,
                info_count as libc::vm_size_t * mem::size_of::<libc::integer_t>(),
            )
        };
        let total = cores
            .iter()
            .fold(CpuTicks::default(), |sum, core| CpuTicks {
                busy: sum.busy + core.busy,
                total: sum.total + core.total,
            });
        Ok((total, cores))
    }

    fn memory(&self) -> io::Result<Memory> {
        let mut stats = MaybeUninit::<libc::vm_statistics64>::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let status = unsafe {
            libc::host_statistics64(
                self.host,
                libc::HOST_VM_INFO64,
                stats.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if status != libc::KERN_SUCCESS {
            return Err(io::Error::other(format!(
                "host_statistics64 failed with {status}"
            )));
        }
        let stats = unsafe { stats.assume_init() };
        let pages = |count: libc::natural_t| u64::from(count) * self.page_size;
        let app = pages(stats.internal_page_count).saturating_sub(pages(stats.purgeable_count));
        let used = app + pages(stats.wire_count) + pages(stats.compressor_page_count);
        let swap = sysctl_value::<libc::xsw_usage>("vm.swapusage")?;
        Ok(Memory {
            total: self.memory_total,
            used: used.min(self.memory_total),
            swap_total: swap.xsu_total,
            swap_used: swap.xsu_used,
        })
    }

    fn network_bytes(&mut self) -> io::Result<(u64, u64)> {
        let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
        let mut len = 0;
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                ptr::null_mut(),
                &mut len,
                ptr::null_mut(),
                0,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        len += len / 4;
        self.route_buf.resize(len, 0);
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                self.route_buf.as_mut_ptr().cast(),
                &mut len,
                ptr::null_mut(),
                0,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(sum_interface_bytes(&self.route_buf[..len]))
    }
}

type RawTicks = [libc::c_uint; libc::CPU_STATE_MAX as usize];

#[derive(Default)]
struct TickCounters {
    last: Vec<RawTicks>,
    totals: Vec<[u64; libc::CPU_STATE_MAX as usize]>,
}

impl TickCounters {
    fn advance(&mut self, raw: impl Iterator<Item = RawTicks>) -> Vec<CpuTicks> {
        let raw: Vec<RawTicks> = raw.collect();
        if raw.len() != self.last.len() {
            self.totals = raw.iter().map(|ticks| ticks.map(u64::from)).collect();
        } else {
            for ((total, now), before) in self.totals.iter_mut().zip(&raw).zip(&self.last) {
                for state in 0..total.len() {
                    total[state] += u64::from(now[state].wrapping_sub(before[state]));
                }
            }
        }
        self.last = raw;
        self.totals.iter().map(core_ticks).collect()
    }
}

fn core_ticks(ticks: &[u64; libc::CPU_STATE_MAX as usize]) -> CpuTicks {
    let tick = |state: libc::c_int| ticks[state as usize];
    let busy =
        tick(libc::CPU_STATE_USER) + tick(libc::CPU_STATE_SYSTEM) + tick(libc::CPU_STATE_NICE);
    CpuTicks {
        busy,
        total: busy + tick(libc::CPU_STATE_IDLE),
    }
}

fn sum_interface_bytes(messages: &[u8]) -> (u64, u64) {
    let mut rx = 0;
    let mut tx = 0;
    let mut offset = 0;
    while offset + mem::size_of::<libc::if_msghdr>() <= messages.len() {
        let header: libc::if_msghdr =
            unsafe { ptr::read_unaligned(messages[offset..].as_ptr().cast()) };
        let message_len = usize::from(header.ifm_msglen);
        if message_len == 0 {
            break;
        }
        if i32::from(header.ifm_type) == libc::RTM_IFINFO2
            && offset + mem::size_of::<libc::if_msghdr2>() <= messages.len()
        {
            let interface: libc::if_msghdr2 =
                unsafe { ptr::read_unaligned(messages[offset..].as_ptr().cast()) };
            let flags = interface.ifm_flags;
            let counted = flags & libc::IFF_UP != 0
                && flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) == 0;
            if counted {
                let data = interface.ifm_data;
                rx += data.ifi_ibytes;
                tx += data.ifi_obytes;
            }
        }
        offset += message_len;
    }
    (rx, tx)
}

fn disk_bytes() -> io::Result<(u64, u64)> {
    let mut iterator: io_iterator_t = 0;
    let status = unsafe {
        IOServiceGetMatchingServices(
            kIOMasterPortDefault,
            IOServiceMatching(c"IOBlockStorageDriver".as_ptr()),
            &mut iterator,
        )
    };
    if status != libc::KERN_SUCCESS {
        return Err(io::Error::other(format!(
            "IOServiceGetMatchingServices failed with {status}"
        )));
    }
    let statistics_key = CFString::from_static_string("Statistics");
    let read_key = CFString::from_static_string("Bytes (Read)");
    let write_key = CFString::from_static_string("Bytes (Write)");
    let mut read = 0;
    let mut written = 0;
    loop {
        let service = unsafe { IOIteratorNext(iterator) };
        if service == 0 {
            break;
        }
        let properties = unsafe {
            IORegistryEntryCreateCFProperty(
                service,
                statistics_key.as_concrete_TypeRef(),
                kCFAllocatorDefault,
                0,
            )
        };
        unsafe { IOObjectRelease(service) };
        if properties.is_null() {
            continue;
        }
        let statistics: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_create_rule(properties as CFDictionaryRef) };
        read += counter(&statistics, &read_key);
        written += counter(&statistics, &write_key);
    }
    unsafe { IOObjectRelease(iterator) };
    Ok((read, written))
}

fn counter(statistics: &CFDictionary<CFString, CFType>, key: &CFString) -> u64 {
    statistics
        .find(key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_i64())
        .map_or(0, |value| value.max(0) as u64)
}

fn filesystems() -> Vec<Filesystem> {
    let mounts = mounted_filesystems();
    mounts
        .iter()
        .filter(|fs| {
            fs.f_flags & libc::MNT_LOCAL as u32 != 0
                && fs.f_flags & libc::MNT_DONTBROWSE as u32 == 0
                && fs.f_blocks > 0
        })
        .map(|fs| {
            let block = u64::from(fs.f_bsize);
            let used = fs.f_blocks.saturating_sub(fs.f_bfree) * block;
            Filesystem {
                mount: unsafe { CStr::from_ptr(fs.f_mntonname.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
                total: used + fs.f_bavail * block,
                used,
            }
        })
        .collect()
}

fn mounted_filesystems() -> Vec<libc::statfs> {
    let count = unsafe { libc::getfsstat(ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count <= 0 {
        return Vec::new();
    }
    let capacity = count as usize + 8;
    let mut mounts: Vec<libc::statfs> = Vec::with_capacity(capacity);
    let bytes = (capacity * mem::size_of::<libc::statfs>()) as libc::c_int;
    let filled = unsafe { libc::getfsstat(mounts.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    if filled <= 0 {
        return Vec::new();
    }
    unsafe { mounts.set_len((filled as usize).min(capacity)) };
    mounts
}

fn uptime_secs() -> io::Result<u64> {
    let boot = sysctl_value::<libc::timeval>("kern.boottime")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;
    Ok(now.as_secs().saturating_sub(boot.tv_sec as u64))
}

fn load_average() -> [f32; 3] {
    let mut load = [0f64; 3];
    if unsafe { libc::getloadavg(load.as_mut_ptr(), 3) } != 3 {
        return [0.0; 3];
    }
    load.map(|value| value as f32)
}

fn sysctl_value<T: Copy>(name: &str) -> io::Result<T> {
    let name = CString::new(name).map_err(io::Error::other)?;
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = mem::size_of::<T>();
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if size != mem::size_of::<T>() {
        return Err(io::Error::other(format!(
            "sysctl {name:?} returned {size} bytes"
        )));
    }
    Ok(unsafe { value.assume_init() })
}

fn sysctl_string(name: &str) -> io::Result<String> {
    let name = CString::new(name).map_err(io::Error::other)?;
    let mut size = 0;
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut buf = vec![0u8; size];
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(size);
    Ok(CStr::from_bytes_until_nul(&buf)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn tick_counters_survive_a_u32_wrap() {
        let mut counters = TickCounters::default();
        let near_max = u32::MAX - 10;
        let before = counters.advance([[near_max, 0, 100, 0]].into_iter())[0];
        let after = counters.advance([[5, 0, 105, 0]].into_iter())[0];
        assert_eq!(after.busy - before.busy, 16);
        assert_eq!(after.total - before.total, 21);
    }

    #[test]
    fn samples_this_mac() {
        let mut collector = Collector::new().unwrap();
        collector.sample().unwrap();
        thread::sleep(Duration::from_millis(200));
        let sample = collector.sample().unwrap();

        assert!(!sample.cores.is_empty());
        assert!(sample.cpu <= crate::proto::FULL);
        assert!(
            sample.memory.total > 0
                && sample.memory.used > 0
                && sample.memory.used <= sample.memory.total
        );
        assert!(sample.uptime_secs > 0);
        assert!(
            sample.filesystems.iter().any(|fs| fs.mount == "/"),
            "mounts: {:?}",
            sample.filesystems
        );
        assert!(collector.info().cpu_model.len() > 3);
    }
}
