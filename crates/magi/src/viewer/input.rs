use std::io;
use std::os::fd::AsRawFd;
use std::time::Duration;

use ratatui::crossterm::terminal;

const ESC: u8 = 0x1b;
const WAKE: Duration = Duration::from_millis(100);
const LONGEST: usize = 4096;
const PATIENCE: u8 = 10;

#[derive(Debug, PartialEq)]
pub enum Input {
    Key(Key),
    Recheck,
    Theme(Vec<u8>),
    Resize,
}

#[derive(Debug, PartialEq)]
pub enum Key {
    Quit,
    Up,
    Down,
    First,
    Last,
    Alerts,
    Open,
}

#[derive(Default)]
pub struct Parser {
    pending: Vec<u8>,
    colours: Vec<u8>,
    waited: u8,
}

impl Parser {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Input>) {
        self.waited = 0;
        self.pending.extend_from_slice(bytes);
        let mut start = 0;
        while start < self.pending.len() {
            match self.next(start, out) {
                Some(used) => start += used,
                None if self.pending.len() - start > LONGEST => start = self.pending.len(),
                None => break,
            }
        }
        self.pending.drain(..start);
    }

    pub fn idle(&mut self, out: &mut Vec<Input>) {
        if self.pending.is_empty() {
            return;
        }
        self.waited = self.waited.saturating_add(1);
        if self.pending.len() > 2 && self.waited < PATIENCE {
            return;
        }
        if self.pending == [ESC] {
            out.push(Input::Key(Key::Quit));
        }
        self.pending.clear();
    }

    fn next(&mut self, start: usize, out: &mut Vec<Input>) -> Option<usize> {
        let rest = &self.pending[start..];
        if rest[0] != ESC {
            if let Some(key) = plain_key(rest[0]) {
                out.push(Input::Key(key));
            }
            return Some(1);
        }
        match *rest.get(1)? {
            b'[' => {
                let end = 2 + rest[2..]
                    .iter()
                    .position(|byte| (0x40..=0x7e).contains(byte))?;
                if let Some(input) = csi(&mut self.colours, &rest[2..=end]) {
                    out.push(input);
                }
                Some(end + 1)
            }
            b'O' => {
                if let Some(key) = final_key(*rest.get(2)?) {
                    out.push(Input::Key(key));
                }
                Some(3)
            }
            b']' => {
                let (end, terminator) = string_end(rest, true)?;
                if terminator > 0 {
                    self.colours.extend_from_slice(&rest[..end + terminator]);
                }
                Some(end + terminator)
            }
            b'P' | b'_' | b'^' | b'X' => {
                let (end, terminator) = string_end(rest, false)?;
                Some(end + terminator)
            }
            ESC => {
                out.push(Input::Key(Key::Quit));
                Some(1)
            }
            _ => Some(2),
        }
    }
}

fn csi(colours: &mut Vec<u8>, body: &[u8]) -> Option<Input> {
    let (&last, params) = body.split_last()?;
    match (params, last) {
        ([b'?', ..], b'c') if !colours.is_empty() => Some(Input::Theme(std::mem::take(colours))),
        ([b'?', b'9', b'9', b'7', b';', ..], b'n') | ([], b'I') => Some(Input::Recheck),
        ([b'1' | b'7'], b'~') => Some(Input::Key(Key::First)),
        ([b'4' | b'8'], b'~') => Some(Input::Key(Key::Last)),
        ([b'?', ..], _) => None,
        _ => final_key(last).map(Input::Key),
    }
}

fn plain_key(byte: u8) -> Option<Key> {
    match byte {
        b'q' | 0x03 => Some(Key::Quit),
        b'k' => Some(Key::Up),
        b'j' => Some(Key::Down),
        b'g' => Some(Key::First),
        b'G' => Some(Key::Last),
        b'a' => Some(Key::Alerts),
        b'\r' | b'\n' => Some(Key::Open),
        _ => None,
    }
}

fn final_key(byte: u8) -> Option<Key> {
    match byte {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'H' => Some(Key::First),
        b'F' => Some(Key::Last),
        b'M' => Some(Key::Open),
        _ => None,
    }
}

fn string_end(sequence: &[u8], bell_ends: bool) -> Option<(usize, usize)> {
    for (index, &byte) in sequence.iter().enumerate().skip(2) {
        match byte {
            0x07 if bell_ends => return Some((index, 1)),
            ESC if sequence.get(index + 1)? == &b'\\' => return Some((index, 2)),
            0x00..=0x1f => return Some((index, 0)),
            _ => {}
        }
    }
    None
}

pub fn read(early: &[u8], mut send: impl FnMut(Input) -> bool) -> io::Result<()> {
    let fd = io::stdin().as_raw_fd();
    let mut parser = Parser::default();
    let mut size = terminal::size().ok();
    let mut chunk = [0u8; 1024];
    let mut inputs = Vec::new();
    parser.feed(early, &mut inputs);
    loop {
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, WAKE.as_millis() as libc::c_int) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            parser.idle(&mut inputs);
        } else {
            let read = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            if read < 0 {
                let error = io::Error::last_os_error();
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) {
                    continue;
                }
                return Err(error);
            }
            if read == 0 {
                return Ok(());
            }
            parser.feed(&chunk[..read as usize], &mut inputs);
        }
        let now = terminal::size().ok();
        if now != size {
            size = now;
            inputs.push(Input::Resize);
        }
        for input in inputs.drain(..) {
            if !send(input) {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(chunks: &[&[u8]]) -> Vec<Input> {
        let mut parser = Parser::default();
        let mut out = Vec::new();
        for chunk in chunks {
            parser.feed(chunk, &mut out);
        }
        parser.idle(&mut out);
        out
    }

    #[test]
    fn an_unfinished_sequence_gives_way_so_keys_still_work() {
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(b"\x1b]", &mut out);
        parser.idle(&mut out);
        parser.feed(b"q", &mut out);
        assert_eq!(out, [Input::Key(Key::Quit)]);
        let mut out = Vec::new();
        parser.feed(b"\x1b]11;rgb:0d0d", &mut out);
        for _ in 1..PATIENCE {
            parser.idle(&mut out);
        }
        parser.feed(b"/1111/1717\x1b\\\x1b[?62c", &mut out);
        assert_eq!(
            out,
            [Input::Theme(b"\x1b]11;rgb:0d0d/1111/1717\x1b\\".to_vec())]
        );
        let mut out = Vec::new();
        parser.feed(b"\x1b]10;rgb:1a1a\x03\x1b]11;rgb:0d", &mut out);
        parser.feed(b"0d\x1b[A\x1b[?62c", &mut out);
        assert_eq!(out, [Input::Key(Key::Quit), Input::Key(Key::Up)]);
        let mut out = Vec::new();
        parser.feed(b"\x1bPstuck", &mut out);
        for _ in 0..PATIENCE {
            parser.idle(&mut out);
        }
        parser.feed(b"j", &mut out);
        assert_eq!(out, [Input::Key(Key::Down)]);
    }

    #[test]
    fn keys_in_every_encoding_map_to_the_same_action() {
        assert_eq!(
            parse(&[b"jk\x1b[A\x1bOB\x1b[1;5A\x1b[H\x1b[1~\x1b[F\x1b[8~gGa\r\x1bOMq\x03"]),
            [
                Key::Down,
                Key::Up,
                Key::Up,
                Key::Down,
                Key::Up,
                Key::First,
                Key::First,
                Key::Last,
                Key::Last,
                Key::First,
                Key::Last,
                Key::Alerts,
                Key::Open,
                Key::Open,
                Key::Quit,
                Key::Quit,
            ]
            .map(Input::Key)
        );
    }

    #[test]
    fn a_lone_escape_quits_only_once_nothing_follows_it() {
        assert_eq!(parse(&[b"\x1b"]), [Input::Key(Key::Quit)]);
        assert_eq!(parse(&[b"\x1b", b"[B"]), [Input::Key(Key::Down)]);
        assert_eq!(
            parse(&[b"\x1b\x1b"]),
            [Input::Key(Key::Quit), Input::Key(Key::Quit)]
        );
    }

    #[test]
    fn colour_replies_never_turn_into_keys_and_arrive_together() {
        let replies: &[u8] =
            b"\x1b]10;rgb:e6e6/eded/f3f3\x1b\\\x1b]11;rgb:0d0d/1111/1717\x07\x1b]4;2;rgb:3f/b9/50\x1b\\";
        assert_eq!(
            parse(&[&replies[..9], &replies[9..], b"\x1b[?62;22c"]),
            [Input::Theme(replies.to_vec())]
        );
        assert_eq!(parse(&[b"\x1b[?62;22c"]), []);
    }

    #[test]
    fn theme_and_focus_signals_ask_for_a_recheck() {
        assert_eq!(
            parse(&[b"\x1b[?997;2n\x1b[I\x1b[O"]),
            [Input::Recheck, Input::Recheck]
        );
    }

    #[test]
    fn other_terminal_answers_are_swallowed_whole() {
        assert_eq!(
            parse(&[b"\x1bP>|rex 0.1\x1b\\\x1b_Gi=7;ENOENT\x1b\\\x1b[6;16;8t\x1b[?2031;2$yq"]),
            [Input::Key(Key::Quit)]
        );
    }
}
