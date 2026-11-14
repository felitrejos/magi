use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const REPEAT_AFTER: Duration = Duration::from_secs(60);

pub fn info(message: impl AsRef<str>) {
    eprintln!("{} {}", clock(), message.as_ref());
}

#[derive(Default)]
pub struct Throttle {
    last: Mutex<Option<(String, Instant)>>,
}

impl Throttle {
    pub fn info(&self, message: String) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((text, at)) = last.as_ref()
            && *text == message
            && at.elapsed() < REPEAT_AFTER
        {
            return;
        }
        info(&message);
        *last = Some((message, Instant::now()));
    }
}

pub fn clock() -> String {
    local_time(now_secs(), c"%H:%M:%S")
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| now.as_secs())
}

pub fn local_time(secs: u64, format: &CStr) -> String {
    let secs = secs as libc::time_t;
    let mut local = MaybeUninit::<libc::tm>::zeroed();
    if unsafe { libc::localtime_r(&secs, local.as_mut_ptr()) }.is_null() {
        return String::from("--:--");
    }
    let local = unsafe { local.assume_init() };
    let mut buf = [0u8; 64];
    let len =
        unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &local) };
    String::from_utf8_lossy(&buf[..len]).into_owned()
}
