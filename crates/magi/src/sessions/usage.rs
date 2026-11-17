use std::collections::HashMap;
use std::time::Instant;

use crate::proto::Usage;

const MAX_DEPTH: usize = 64;
const TOP_BY_CPU: usize = 16;
const TOP_BY_MEMORY: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Process {
    pub name: String,
    pub parent: i32,
    pub cpu_ns: u64,
    pub memory: u64,
}

#[derive(Default)]
pub struct Measured {
    pub usage: HashMap<i32, Usage>,
    pub parents: HashMap<i32, i32>,
    pub top: Vec<Busy>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Busy {
    pub pid: i32,
    pub name: String,
    pub cpu: Option<u32>,
    pub memory: u64,
    pub root: Option<i32>,
}

#[derive(Default)]
pub struct Meter {
    previous: Option<(Instant, HashMap<i32, u64>)>,
}

impl Meter {
    pub fn measure(&mut self, roots: &[i32], now: Instant) -> Measured {
        let processes = snapshot();
        let usage = usage(&processes, roots, self.previous.as_ref(), now);
        let parents = parents(&processes, roots);
        let top = busiest(&processes, roots, self.previous.as_ref(), now);
        self.previous = Some((
            now,
            processes
                .iter()
                .map(|(pid, process)| (*pid, process.cpu_ns))
                .collect(),
        ));
        Measured {
            usage,
            parents,
            top,
        }
    }
}

fn busiest(
    processes: &HashMap<i32, Process>,
    roots: &[i32],
    previous: Option<&(Instant, HashMap<i32, u64>)>,
    now: Instant,
) -> Vec<Busy> {
    let elapsed_ns = previous
        .map(|(at, _)| now.duration_since(*at).as_nanos())
        .filter(|elapsed| *elapsed > 0);
    let tracked: HashMap<i32, (u64, u64)> = roots.iter().map(|root| (*root, (0, 0))).collect();
    let mut all: Vec<Busy> = processes
        .iter()
        .map(|(pid, process)| {
            let cpu = previous.zip(elapsed_ns).map(|((_, before), elapsed)| {
                let spent = process
                    .cpu_ns
                    .saturating_sub(before.get(pid).copied().unwrap_or(process.cpu_ns));
                u32::try_from(u128::from(spent) * 1000 / elapsed).unwrap_or(u32::MAX)
            });
            Busy {
                pid: *pid,
                name: process.name.clone(),
                cpu,
                memory: process.memory,
                root: owner(*pid, processes, &tracked),
            }
        })
        .collect();
    all.sort_by_key(|busy| std::cmp::Reverse((busy.cpu.unwrap_or(0), busy.memory)));
    let mut top: Vec<Busy> = all.iter().take(TOP_BY_CPU).cloned().collect();
    all.sort_by_key(|busy| std::cmp::Reverse(busy.memory));
    for busy in all {
        if top.len() >= TOP_BY_CPU + TOP_BY_MEMORY {
            break;
        }
        if !top.iter().any(|known| known.pid == busy.pid) {
            top.push(busy);
        }
    }
    top
}

fn parents(processes: &HashMap<i32, Process>, roots: &[i32]) -> HashMap<i32, i32> {
    let tracked: HashMap<i32, (u64, u64)> = roots.iter().map(|root| (*root, (0, 0))).collect();
    roots
        .iter()
        .filter_map(|root| {
            let parent = processes.get(root)?.parent;
            let owner = owner(parent, processes, &tracked)?;
            (owner != *root).then_some((*root, owner))
        })
        .collect()
}

fn usage(
    processes: &HashMap<i32, Process>,
    roots: &[i32],
    previous: Option<&(Instant, HashMap<i32, u64>)>,
    now: Instant,
) -> HashMap<i32, Usage> {
    let elapsed_ns = previous
        .map(|(at, _)| now.duration_since(*at).as_nanos())
        .filter(|elapsed| *elapsed > 0);
    let mut totals: HashMap<i32, (u64, u64)> = roots
        .iter()
        .filter(|root| processes.contains_key(root))
        .map(|root| (*root, (0, 0)))
        .collect();
    for (pid, process) in processes {
        let Some(root) = owner(*pid, processes, &totals) else {
            continue;
        };
        let spent = previous.map_or(0, |(_, before)| {
            process
                .cpu_ns
                .saturating_sub(before.get(pid).copied().unwrap_or(0))
        });
        let total = totals.get_mut(&root).expect("owner is a tracked root");
        total.0 += spent;
        total.1 += process.memory;
    }
    totals
        .into_iter()
        .map(|(root, (spent, memory))| {
            let cpu = elapsed_ns.map(|elapsed| {
                u32::try_from(u128::from(spent) * 1000 / elapsed).unwrap_or(u32::MAX)
            });
            (root, Usage { cpu, memory })
        })
        .collect()
}

fn owner(
    pid: i32,
    processes: &HashMap<i32, Process>,
    roots: &HashMap<i32, (u64, u64)>,
) -> Option<i32> {
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        if roots.contains_key(&current) {
            return Some(current);
        }
        let parent = processes.get(&current)?.parent;
        if parent <= 1 || parent == current {
            return None;
        }
        current = parent;
    }
    None
}

#[cfg(target_os = "linux")]
fn snapshot() -> HashMap<i32, Process> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return HashMap::new();
    };
    let tick_ns = 1_000_000_000
        / u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
            .unwrap_or(100)
            .max(1);
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(4096);
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            Some((pid, parse_stat(&stat, tick_ns, page)?))
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn parse_stat(stat: &str, tick_ns: u64, page: u64) -> Option<Process> {
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    let number = |index: usize| fields.get(index)?.parse::<u64>().ok();
    let name = stat.get(stat.find('(')? + 1..stat.rfind(')')?)?.to_string();
    Some(Process {
        name,
        parent: fields.get(1)?.parse().ok()?,
        cpu_ns: (number(11)? + number(12)?) * tick_ns,
        memory: number(21)? * page,
    })
}

#[cfg(target_os = "macos")]
fn c_name(chars: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(target_os = "macos")]
fn snapshot() -> HashMap<i32, Process> {
    use std::mem::{MaybeUninit, size_of};
    use std::sync::OnceLock;

    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> libc::c_int;
    }
    static TIMEBASE: OnceLock<(u64, u64)> = OnceLock::new();
    let (numer, denom) = *TIMEBASE.get_or_init(|| {
        let mut info = Timebase { numer: 0, denom: 0 };
        if unsafe { mach_timebase_info(&mut info) } != 0 || info.denom == 0 {
            return (1, 1);
        }
        (u64::from(info.numer), u64::from(info.denom))
    });
    super::process::pids()
        .into_iter()
        .filter_map(|pid| {
            let mut info = MaybeUninit::<libc::proc_taskallinfo>::zeroed();
            let size = size_of::<libc::proc_taskallinfo>() as libc::c_int;
            let read = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTASKALLINFO,
                    0,
                    info.as_mut_ptr().cast(),
                    size,
                )
            };
            if read != size {
                return None;
            }
            let info = unsafe { info.assume_init() };
            let ticks = info.ptinfo.pti_total_user + info.ptinfo.pti_total_system;
            let name = c_name(&info.pbsd.pbi_name);
            Some((
                pid,
                Process {
                    name: if name.is_empty() {
                        c_name(&info.pbsd.pbi_comm)
                    } else {
                        name
                    },
                    parent: i32::try_from(info.pbsd.pbi_ppid).ok()?,
                    cpu_ns: u64::try_from(
                        u128::from(ticks) * u128::from(numer) / u128::from(denom),
                    )
                    .unwrap_or(u64::MAX),
                    memory: info.ptinfo.pti_resident_size,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn process(parent: i32, cpu_ns: u64, memory: u64) -> Process {
        Process {
            name: format!("p{parent}"),
            parent,
            cpu_ns,
            memory,
        }
    }

    #[test]
    fn a_session_counts_every_process_it_started_and_nested_agents_count_for_themselves() {
        let processes = HashMap::from([
            (100, process(1, 0, 10)),
            (101, process(100, 0, 20)),
            (102, process(101, 0, 30)),
            (200, process(101, 0, 40)),
            (201, process(200, 0, 50)),
            (300, process(1, 0, 60)),
        ]);
        let usage = usage(&processes, &[100, 200, 999], None, Instant::now());
        assert_eq!(usage[&100].memory, 60);
        assert_eq!(usage[&200].memory, 90);
        assert!(!usage.contains_key(&999));
        assert_eq!(usage[&100].cpu, None);
    }

    #[test]
    fn cpu_is_time_spent_since_the_last_look_divided_by_the_time_between() {
        let then = Instant::now();
        let before = HashMap::from([
            (100, 1_000_000_000),
            (101, 500_000_000),
            (102, 9_000_000_000),
        ]);
        let processes = HashMap::from([
            (100, process(1, 1_500_000_000, 0)),
            (101, process(100, 1_500_000_000, 0)),
            (103, process(100, 250_000_000, 0)),
        ]);
        let usage = usage(
            &processes,
            &[100],
            Some(&(then, before)),
            then + Duration::from_secs(1),
        );
        assert_eq!(usage[&100].cpu, Some(1750));
    }

    #[test]
    fn a_session_started_inside_another_names_it_as_its_parent() {
        let processes = HashMap::from([
            (10, process(1, 0, 0)),
            (11, process(10, 0, 0)),
            (12, process(11, 0, 0)),
            (20, process(1, 0, 0)),
        ]);
        assert_eq!(
            parents(&processes, &[10, 12, 20]),
            HashMap::from([(12, 10)])
        );
    }

    #[test]
    fn a_parent_loop_never_hangs() {
        let processes = HashMap::from([(5, process(6, 0, 1)), (6, process(5, 0, 1))]);
        assert!(usage(&processes, &[7], None, Instant::now()).is_empty());
    }

    #[test]
    fn linux_stat_reads_parent_cpu_and_memory_past_a_tricky_name() {
        let stat = "4242 (node (worker) 1) S 4100 4242 4100 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 11 0 9000 123456789 3000 18446744073709551615";
        assert_eq!(
            parse_stat(stat, 10_000_000, 4096),
            Some(Process {
                name: "node (worker) 1".into(),
                ..process(4100, 3_000_000_000, 3000 * 4096)
            })
        );
        assert_eq!(parse_stat("1 (x) S", 10_000_000, 4096), None);
    }

    #[test]
    fn this_process_is_measured_in_real_nanoseconds() {
        let me = std::process::id() as i32;
        let cpu_now = || snapshot()[&me].cpu_ns;
        let rusage = || {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
            unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
            let usage = unsafe { usage.assume_init() };
            let ns = |time: libc::timeval| {
                time.tv_sec as u64 * 1_000_000_000 + time.tv_usec as u64 * 1000
            };
            ns(usage.ru_utime) + ns(usage.ru_stime)
        };
        let (ours, theirs) = (cpu_now(), rusage());
        let started = Instant::now();
        let mut spin = 0u64;
        while started.elapsed() < Duration::from_millis(300) {
            spin = std::hint::black_box(spin.wrapping_add(1));
        }
        let ours = cpu_now() - ours;
        let theirs = rusage() - theirs;
        let ratio = ours as f64 / theirs as f64;
        assert!((0.7..1.3).contains(&ratio), "ours {ours} theirs {theirs}");
        let me = &snapshot()[&me];
        assert_eq!(me.parent, unsafe { libc::getppid() });
        assert!(me.memory > 1 << 20);
    }

    #[test]
    #[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_usage"]
    fn bench_usage() {
        let processes = snapshot().len();
        let roots: Vec<i32> = vec![std::process::id() as i32];
        let mut meter = Meter::default();
        meter.measure(&roots, Instant::now());
        let runs = 100;
        let started = Instant::now();
        for _ in 0..runs {
            meter.measure(&roots, Instant::now());
        }
        println!(
            "usage of one session among {processes} processes: {:?} per sample",
            started.elapsed() / runs
        );
    }
}
