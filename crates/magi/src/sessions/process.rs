use std::path::PathBuf;

#[cfg(target_os = "macos")]
pub fn open_files(name: &str) -> Vec<(i32, PathBuf)> {
    macos::pids()
        .into_iter()
        .filter(|pid| macos::name(*pid).as_deref() == Some(name))
        .flat_map(|pid| {
            macos::open_files(pid)
                .into_iter()
                .map(move |path| (pid, path))
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub fn open_files(name: &str) -> Vec<(i32, PathBuf)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .is_ok_and(|comm| comm.trim_end() == name)
        })
        .filter_map(|pid| Some((pid, std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?)))
        .flat_map(|(pid, fds)| {
            fds.flatten()
                .filter_map(|fd| std::fs::read_link(fd.path()).ok())
                .filter(|path| path.is_absolute())
                .map(move |path| (pid, path))
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::CStr;
    use std::mem::{MaybeUninit, size_of};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    const PROC_PIDFDVNODEPATHINFO: libc::c_int = 2;

    #[repr(C)]
    struct ProcFileInfo {
        open_flags: u32,
        status: u32,
        offset: libc::off_t,
        kind: i32,
        guard_flags: u32,
    }

    #[repr(C)]
    struct VnodeFdInfoWithPath {
        file: ProcFileInfo,
        vnode: libc::vnode_info_path,
    }

    pub fn pids() -> Vec<i32> {
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0i32; count as usize + 64];
        let bytes = (pids.len() * size_of::<i32>()) as libc::c_int;
        let found = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(found.max(0) as usize);
        pids
    }

    pub fn name(pid: i32) -> Option<String> {
        let mut buf = [0u8; 256];
        let len = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        (len > 0).then(|| String::from_utf8_lossy(&buf[..len as usize]).into_owned())
    }

    pub fn open_files(pid: i32) -> Vec<PathBuf> {
        let bytes =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if bytes <= 0 {
            return Vec::new();
        }
        let mut fds: Vec<libc::proc_fdinfo> =
            Vec::with_capacity(bytes as usize / size_of::<libc::proc_fdinfo>() + 16);
        let capacity = (fds.capacity() * size_of::<libc::proc_fdinfo>()) as libc::c_int;
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast(),
                capacity,
            )
        };
        if written <= 0 {
            return Vec::new();
        }
        unsafe { fds.set_len(written as usize / size_of::<libc::proc_fdinfo>()) };
        fds.iter()
            .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_VNODE as u32)
            .filter_map(|fd| path(pid, fd.proc_fd))
            .collect()
    }

    fn path(pid: i32, fd: i32) -> Option<PathBuf> {
        let mut info = MaybeUninit::<VnodeFdInfoWithPath>::zeroed();
        let size = size_of::<VnodeFdInfoWithPath>() as libc::c_int;
        let written = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDVNODEPATHINFO,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if written != size {
            return None;
        }
        let info = unsafe { info.assume_init() };
        let raw: [u8; 1024] = unsafe { std::mem::transmute(info.vnode.vip_path) };
        let path = CStr::from_bytes_until_nul(&raw).ok()?;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    #[test]
    #[ignore = "helper: finds_a_named_process_and_the_files_it_holds_open runs it as magiprobe"]
    fn probe_holds_its_output_open() {
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[test]
    fn finds_a_named_process_and_the_files_it_holds_open() {
        let exe = std::env::current_exe().unwrap();
        let dir = exe.with_file_name(format!("magi-process-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("magiprobe");
        std::fs::hard_link(&exe, &program).unwrap();
        let held = dir.join("held.jsonl");
        let mut child = std::process::Command::new(&program)
            .args([
                "sessions::process::tests::probe_holds_its_output_open",
                "--exact",
                "--ignored",
            ])
            .stdout(File::create(&held).unwrap())
            .spawn()
            .unwrap();
        let held = std::fs::canonicalize(&held).unwrap();
        let child_pid = child.id() as i32;
        let holds = |files: &[(i32, PathBuf)]| {
            files.iter().any(|(pid, file)| {
                *pid == child_pid && std::fs::canonicalize(file).is_ok_and(|file| file == held)
            })
        };
        let mut found = Vec::new();
        for _ in 0..50 {
            found = open_files("magiprobe");
            if holds(&found) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(holds(&found), "{found:?}");
        assert!(!holds(&open_files("magiprobe-absent")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_process_scan"]
    fn bench_process_scan() {
        let runs = 50;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            open_files("codex");
        }
        println!("process scan: {:?} per scan", started.elapsed() / runs);
    }
}
