use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_LINE_BYTES: usize = 16 << 20;

pub trait Lines {
    fn restart(&mut self);
    fn line(&mut self, line: &[u8]);
}

pub struct Tail {
    path: PathBuf,
    file_id: Option<(u64, u64)>,
    offset: u64,
    pending: Vec<u8>,
    skipping_line: bool,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            file_id: None,
            offset: 0,
            pending: Vec::new(),
            skipping_line: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn advance(&mut self, budget: &mut u64, lines: &mut impl Lines) -> io::Result<()> {
        let metadata = fs::metadata(&self.path)?;
        let file_id = Some((metadata.dev(), metadata.ino()));
        if metadata.len() < self.offset || (self.file_id.is_some() && self.file_id != file_id) {
            *self = Tail::new(self.path.clone());
            lines.restart();
        }
        self.file_id = file_id;
        if metadata.len() == self.offset || *budget == 0 {
            return Ok(());
        }
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut chunk = vec![0; CHUNK_BYTES];
        let mut remaining = (metadata.len() - self.offset).min(*budget);
        while remaining > 0 {
            let wanted = remaining.min(CHUNK_BYTES as u64) as usize;
            let read = file.read(&mut chunk[..wanted])?;
            if read == 0 {
                break;
            }
            self.offset += read as u64;
            remaining -= read as u64;
            *budget -= read as u64;
            self.take(&chunk[..read], lines);
        }
        Ok(())
    }

    fn take(&mut self, bytes: &[u8], lines: &mut impl Lines) {
        let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            self.hold(bytes);
            return;
        };
        let first_end = bytes.iter().position(|byte| *byte == b'\n').unwrap_or(end);
        let first_too_long = self.pending.len() + first_end > MAX_LINE_BYTES;
        let mut complete = std::mem::take(&mut self.pending);
        complete.extend_from_slice(&bytes[..end]);
        let mut split = complete.split(|byte| *byte == b'\n');
        if std::mem::take(&mut self.skipping_line) || first_too_long {
            split.next();
        }
        for line in split {
            lines.line(line);
        }
        self.hold(&bytes[end + 1..]);
    }

    fn hold(&mut self, bytes: &[u8]) {
        if self.skipping_line {
            return;
        }
        if self.pending.len() + bytes.len() > MAX_LINE_BYTES {
            self.pending = Vec::new();
            self.skipping_line = true;
            return;
        }
        self.pending.extend_from_slice(bytes);
    }
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub fn unreadable(path: &Path, read: io::Result<()>) -> Option<String> {
    match read {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Some(format!("can't read {}: {error}", path.display()))
        }
        _ => None,
    }
}

#[cfg(test)]
pub mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;

    use super::*;

    #[test]
    fn a_missing_transcript_is_quiet_and_any_other_read_error_is_reported() {
        let path = Path::new("/x/s.jsonl");
        assert_eq!(unreadable(path, Ok(())), None);
        let gone = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(unreadable(path, Err(gone)), None);
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(
            unreadable(path, Err(denied))
                .is_some_and(|notice| notice.starts_with("can't read /x/s.jsonl"))
        );
    }

    pub fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("magi-tail-{}-{name}.jsonl", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    pub fn append(path: &Path, text: &str) {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    #[derive(Default)]
    struct Collect {
        lines: Vec<String>,
        restarts: usize,
    }

    impl Lines for Collect {
        fn restart(&mut self) {
            self.lines.clear();
            self.restarts += 1;
        }

        fn line(&mut self, line: &[u8]) {
            self.lines.push(String::from_utf8_lossy(line).into_owned());
        }
    }

    fn read_all(tail: &mut Tail, collect: &mut Collect) {
        tail.advance(&mut u64::MAX.clone(), collect).unwrap();
    }

    #[test]
    fn a_half_written_line_waits_for_its_end() {
        let path = temp("partial");
        append(&path, "one\ntw");
        let mut tail = Tail::new(path.clone());
        let mut collect = Collect::default();
        read_all(&mut tail, &mut collect);
        assert_eq!(collect.lines, vec!["one"]);
        append(&path, "o\n");
        read_all(&mut tail, &mut collect);
        assert_eq!(collect.lines, vec!["one", "two"]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_replaced_file_restarts_from_the_beginning() {
        let path = temp("replaced");
        append(&path, "old\n");
        let mut tail = Tail::new(path.clone());
        let mut collect = Collect::default();
        read_all(&mut tail, &mut collect);
        let replacement = temp("replacement");
        append(&replacement, "new\nnewer\n");
        fs::rename(&replacement, &path).unwrap();
        read_all(&mut tail, &mut collect);
        assert_eq!(
            (collect.lines.clone(), collect.restarts),
            (vec!["new".into(), "newer".into()], 1)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn the_byte_budget_spreads_a_long_catch_up_over_several_polls() {
        let path = temp("budget");
        append(&path, &"0123456789\n".repeat(100));
        let mut tail = Tail::new(path.clone());
        let mut collect = Collect::default();
        let mut budget = 11 * 10;
        tail.advance(&mut budget, &mut collect).unwrap();
        assert_eq!((collect.lines.len(), budget), (10, 0));
        read_all(&mut tail, &mut collect);
        assert_eq!(collect.lines.len(), 100);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_line_too_long_to_hold_is_skipped_and_reading_resumes_after_it() {
        let path = temp("huge");
        append(
            &path,
            &format!("before\n{}\nafter\n", "x".repeat(MAX_LINE_BYTES + 1)),
        );
        let mut tail = Tail::new(path.clone());
        let mut collect = Collect::default();
        read_all(&mut tail, &mut collect);
        assert_eq!(collect.lines, vec!["before", "after"]);
        assert!(tail.pending.is_empty() && !tail.skipping_line);
        fs::remove_file(path).unwrap();
    }
}
