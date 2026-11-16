use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(1);
const GRAPHICS: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[>q";
pub const CELL_SIZE: &[u8] = b"\x1b[16t";
pub const COLOURS: &[u8] = concat!(
    "\x1b]10;?\x1b\\",
    "\x1b]11;?\x1b\\",
    "\x1b]4;1;?\x1b\\",
    "\x1b]4;2;?\x1b\\",
    "\x1b]4;3;?\x1b\\",
    "\x1b]4;4;?\x1b\\",
    "\x1b]4;5;?\x1b\\",
    "\x1b[c",
)
.as_bytes();
pub const MODES: &[u8] = b"\x1b[?1004h\x1b[?2031h\x1b[?2004h\x1b[?1000h\x1b[?1006h";
pub const RESET_MODES: &[u8] = b"\x1b[?1006l\x1b[?1000l\x1b[?2004l\x1b[?2031l\x1b[?1004l";

pub fn probe(out: &mut impl Write) -> io::Result<Vec<u8>> {
    out.write_all(GRAPHICS)?;
    out.write_all(COLOURS)?;
    out.flush()?;
    read_reply(Instant::now() + TIMEOUT)
}

fn read_reply(deadline: Instant) -> io::Result<Vec<u8>> {
    let stdin = io::stdin();
    let fd = stdin.as_raw_fd();
    let mut reply = Vec::new();
    let mut chunk = [0u8; 256];
    while !answered(&reply) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis() as libc::c_int) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            break;
        }
        let read = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if read <= 0 {
            break;
        }
        reply.extend_from_slice(&chunk[..read as usize]);
    }
    Ok(reply)
}

fn answered(reply: &[u8]) -> bool {
    answer_end(reply).is_some()
}

pub fn unanswered(reply: &[u8]) -> &[u8] {
    answer_end(reply).map_or(reply, |end| &reply[end..])
}

fn answer_end(reply: &[u8]) -> Option<usize> {
    let start = reply.windows(3).position(|window| window == b"\x1b[?")?;
    let c = reply[start..].iter().position(|&byte| byte == b'c')?;
    Some(start + c + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_ends_at_the_device_attributes_answer() {
        assert!(answered(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c"));
        assert!(!answered(b"\x1b_Gi=31;OK\x1b\\"));
        assert_eq!(unanswered(b"\x1b]10;rgb:1/2/3\x1b\\\x1b[?62cj"), b"j");
        assert_eq!(unanswered(b"\x1b]10;rgb:1a1a"), b"\x1b]10;rgb:1a1a");
    }
}
