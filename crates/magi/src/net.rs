use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::mem;
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const KEEPALIVE_IDLE_SECS: libc::c_int = 10;
const KEEPALIVE_INTERVAL_SECS: libc::c_int = 5;
const KEEPALIVE_PROBES: libc::c_int = 3;

pub trait Conn: Read + Write + Send {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl Conn for TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_write_timeout(self, timeout)
    }
}

impl Conn for UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        UnixStream::set_read_timeout(self, timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        UnixStream::set_write_timeout(self, timeout)
    }
}

pub fn read_exact_before(conn: &mut dyn Conn, buf: &mut [u8], deadline: Instant) -> io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "deadline passed"));
        }
        set_timeout_unless_peer_closed(conn, remaining)?;
        match conn.read(&mut buf[filled..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "deadline passed"));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn set_timeout_unless_peer_closed(conn: &dyn Conn, timeout: Duration) -> io::Result<()> {
    match conn.set_read_timeout(Some(timeout)) {
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
        result => result,
    }
}

pub fn local_socket() -> io::Result<PathBuf> {
    Ok(state_dir()?.join("agent.sock"))
}

pub fn status_socket() -> io::Result<PathBuf> {
    Ok(state_dir()?.join("status.sock"))
}

fn state_dir() -> io::Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
    Ok(Path::new(&home).join(".local/state/magi"))
}

pub fn bind_local(path: &Path) -> io::Result<UnixListener> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("socket path has no directory"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    if UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another agent is already listening on {}", path.display()),
        ));
    }
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub fn keepalive(stream: &TcpStream) -> io::Result<()> {
    let fd = stream.as_raw_fd();
    set_option(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1)?;
    #[cfg(target_os = "macos")]
    set_option(
        fd,
        libc::IPPROTO_TCP,
        libc::TCP_KEEPALIVE,
        KEEPALIVE_IDLE_SECS,
    )?;
    #[cfg(target_os = "linux")]
    set_option(
        fd,
        libc::IPPROTO_TCP,
        libc::TCP_KEEPIDLE,
        KEEPALIVE_IDLE_SECS,
    )?;
    set_option(
        fd,
        libc::IPPROTO_TCP,
        libc::TCP_KEEPINTVL,
        KEEPALIVE_INTERVAL_SECS,
    )?;
    set_option(fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT, KEEPALIVE_PROBES)
}

fn set_option(
    fd: libc::c_int,
    level: libc::c_int,
    name: libc::c_int,
    value: libc::c_int,
) -> io::Result<()> {
    let status = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&value as *const libc::c_int).cast(),
            mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::os::unix::fs::PermissionsExt;
    use std::thread;

    use super::*;

    fn temp_socket(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("magi-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("agent.sock")
    }

    #[test]
    fn local_socket_is_private_and_refuses_a_second_agent() {
        let path = temp_socket("private");
        let _listener = bind_local(&path).unwrap();
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            bind_local(&path).unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_stale_socket_file_is_replaced() {
        let path = temp_socket("stale");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"left behind by a crashed agent").unwrap();
        let _listener = bind_local(&path).unwrap();
        assert!(UnixStream::connect(&path).is_ok());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_trickling_peer_cannot_stretch_the_deadline() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let trickler = thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            for _ in 0..10 {
                if stream.write_all(&[0]).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(150));
            }
        });
        let (mut stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        let error = read_exact_before(
            &mut stream,
            &mut [0; 8],
            started + Duration::from_millis(400),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(700));
        drop(stream);
        trickler.join().unwrap();
    }

    #[test]
    fn a_message_from_a_peer_that_already_hung_up_is_still_read() {
        let (mut ours, mut theirs) = UnixStream::pair().unwrap();
        theirs.write_all(b"report").unwrap();
        drop(theirs);
        let mut buf = [0; 6];
        read_exact_before(&mut ours, &mut buf, Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(&buf, b"report");
    }

    #[test]
    fn keepalive_applies_to_a_live_socket() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        keepalive(&stream).unwrap();
    }
}
