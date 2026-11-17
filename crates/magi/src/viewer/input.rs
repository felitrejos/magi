use std::io;
use std::os::fd::AsRawFd;
use std::time::Duration;

use ratatui::crossterm::terminal;

const ESC: u8 = 0x1b;
const WAKE: Duration = Duration::from_millis(100);
const LONGEST: usize = 4096;
const PATIENCE: u8 = 10;
const PASTE_START: &[u8] = b"200~";
const PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, PartialEq)]
pub enum Input {
    Key(Key),
    Recheck,
    Theme(Vec<u8>),
    Resize,
    Cells { width: u32, height: u32 },
    Closed,
    Wheel { up: bool },
}

#[derive(Debug, PartialEq)]
pub enum Key {
    Char(char),
    Paste(String),
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    First,
    Last,
    Enter,
    Newline,
    Escape,
    Backspace,
    Tab,
    BackTab,
    Interrupt,
}

#[derive(Default)]
pub struct Parser {
    pending: Vec<u8>,
    colours: Vec<u8>,
    waited: u8,
    pasting: bool,
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
        if self.pending.is_empty() && !self.pasting {
            return;
        }
        self.waited = self.waited.saturating_add(1);
        if self.pasting {
            if self.waited >= PATIENCE {
                if !self.pending.is_empty() {
                    let text = String::from_utf8_lossy(&self.pending).into_owned();
                    out.push(Input::Key(Key::Paste(text)));
                }
                self.pending.clear();
                self.pasting = false;
            }
            return;
        }
        if self.pending.len() > 2 && self.waited < PATIENCE {
            return;
        }
        if self.pending == [ESC] {
            out.push(Input::Key(Key::Escape));
        }
        self.pending.clear();
    }

    fn next(&mut self, start: usize, out: &mut Vec<Input>) -> Option<usize> {
        let rest = &self.pending[start..];
        if self.pasting {
            return self.paste(start, out);
        }
        if rest[0] != ESC {
            let (key, used) = plain_key(rest)?;
            out.extend(key.map(Input::Key));
            return Some(used);
        }
        match *rest.get(1)? {
            b'[' => {
                let end = 2 + rest[2..]
                    .iter()
                    .position(|byte| (0x40..=0x7e).contains(byte))?;
                if &rest[2..=end] == PASTE_START {
                    self.pasting = true;
                } else if let Some(input) = csi(&mut self.colours, &rest[2..=end]) {
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
                out.push(Input::Key(Key::Escape));
                Some(1)
            }
            b'\r' | b'\n' => {
                out.push(Input::Key(Key::Newline));
                Some(2)
            }
            _ => Some(2),
        }
    }

    fn paste(&mut self, start: usize, out: &mut Vec<Input>) -> Option<usize> {
        let rest = &self.pending[start..];
        let (text, used) = match rest
            .windows(PASTE_END.len())
            .position(|window| window == PASTE_END)
        {
            Some(end) => {
                self.pasting = false;
                (&rest[..end], end + PASTE_END.len())
            }
            None if rest.len() > LONGEST => {
                let mut cut = rest.len() - (PASTE_END.len() - 1);
                while cut > 0 && rest[cut] & 0xc0 == 0x80 {
                    cut -= 1;
                }
                (&rest[..cut], cut)
            }
            None => return None,
        };
        out.push(Input::Key(Key::Paste(
            String::from_utf8_lossy(text).into_owned(),
        )));
        Some(used)
    }
}

fn csi(colours: &mut Vec<u8>, body: &[u8]) -> Option<Input> {
    let (&last, params) = body.split_last()?;
    match (params, last) {
        ([b'?', ..], b'c') if !colours.is_empty() => Some(Input::Theme(std::mem::take(colours))),
        ([b'?', b'9', b'9', b'7', b';', ..], b'n') | ([], b'I') => Some(Input::Recheck),
        ([b'1' | b'7'], b'~') => Some(Input::Key(Key::First)),
        ([b'4' | b'8'], b'~') => Some(Input::Key(Key::Last)),
        ([b'5'], b'~') => Some(Input::Key(Key::PageUp)),
        ([b'6'], b'~') => Some(Input::Key(Key::PageDown)),
        ([b'2', b'7', b';', .., b';', b'1', b'3'], b'~') => Some(Input::Key(Key::Newline)),
        ([b'6', b';', size @ ..], b't') => cell_size(size),
        ([b'<', mouse @ ..], b'M') => mouse_press(mouse),
        ([b'?' | b'<', ..], _) => None,
        _ => final_key(last).map(Input::Key),
    }
}

fn cell_size(params: &[u8]) -> Option<Input> {
    let (height, width) = std::str::from_utf8(params).ok()?.split_once(';')?;
    let (height, width): (u32, u32) = (height.parse().ok()?, width.parse().ok()?);
    (height > 0 && width > 0).then_some(Input::Cells { width, height })
}

fn mouse_press(params: &[u8]) -> Option<Input> {
    let button = std::str::from_utf8(params).ok()?.split(';').next()?;
    match button {
        "64" => Some(Input::Wheel { up: true }),
        "65" => Some(Input::Wheel { up: false }),
        _ => None,
    }
}

fn plain_key(rest: &[u8]) -> Option<(Option<Key>, usize)> {
    let key = match rest[0] {
        0x03 => Key::Interrupt,
        b'\r' => Key::Enter,
        b'\n' => Key::Newline,
        0x7f | 0x08 => Key::Backspace,
        b'\t' => Key::Tab,
        byte @ 0x20..=0x7e => Key::Char(char::from(byte)),
        0x00..=0x1f => return Some((None, 1)),
        lead => {
            let len = match lead {
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => return Some((None, 1)),
            };
            let bytes = rest.get(..len)?;
            return Some(match std::str::from_utf8(bytes) {
                Ok(text) => (text.chars().next().map(Key::Char), len),
                Err(_) => (None, 1),
            });
        }
    };
    Some((Some(key), 1))
}

fn final_key(byte: u8) -> Option<Key> {
    match byte {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        b'H' => Some(Key::First),
        b'F' => Some(Key::Last),
        b'M' => Some(Key::Enter),
        b'Z' => Some(Key::BackTab),
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
        assert_eq!(out, [Input::Key(Key::Char('q'))]);
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
        assert_eq!(out, [Input::Key(Key::Interrupt), Input::Key(Key::Up)]);
        let mut out = Vec::new();
        parser.feed(b"\x1bPstuck", &mut out);
        for _ in 0..PATIENCE {
            parser.idle(&mut out);
        }
        parser.feed(b"j", &mut out);
        assert_eq!(out, [Input::Key(Key::Char('j'))]);
    }

    #[test]
    fn a_cell_size_reply_becomes_its_own_input() {
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(b"\x1b[6;34;16t\x1b[6;0;16t", &mut out);
        assert_eq!(
            out,
            [Input::Cells {
                width: 16,
                height: 34
            }]
        );
    }

    #[test]
    fn keys_in_every_encoding_map_to_the_same_key() {
        assert_eq!(
            parse(&[b"\x1b[A\x1bOB\x1b[C\x1bOD\x1b[1;5A\x1b[H\x1b[1~\x1b[F\x1b[8~\r\x1bOM\x7f\x08\t\x1b[Z\x03"]),
            [
                Key::Up,
                Key::Down,
                Key::Right,
                Key::Left,
                Key::Up,
                Key::First,
                Key::First,
                Key::Last,
                Key::Last,
                Key::Enter,
                Key::Enter,
                Key::Backspace,
                Key::Backspace,
                Key::Tab,
                Key::BackTab,
                Key::Interrupt,
            ]
            .map(Input::Key)
        );
    }

    #[test]
    fn shift_enter_alt_enter_and_ctrl_j_start_a_new_line() {
        assert_eq!(
            parse(&[b"\x1b[27;2;13~\n\x1b\r\x1b\n\r\x1b[27;2;9~"]),
            [
                Key::Newline,
                Key::Newline,
                Key::Newline,
                Key::Newline,
                Key::Enter
            ]
            .map(Input::Key)
        );
    }

    #[test]
    fn letters_arrive_as_characters_even_when_split_across_reads() {
        let bytes = "qé日".as_bytes();
        assert_eq!(
            parse(&[&bytes[..2], &bytes[2..4], &bytes[4..]]),
            [Key::Char('q'), Key::Char('é'), Key::Char('日')].map(Input::Key)
        );
        assert_eq!(parse(&[b"\x01\xff"]), []);
    }

    #[test]
    fn a_paste_arrives_whole_with_its_newlines_and_escapes_as_text() {
        assert_eq!(
            parse(&[b"\x1b[200~line one\n", b"q\x1b[A\x1b[201~j"]),
            [
                Input::Key(Key::Paste("line one\nq\x1b[A".into())),
                Input::Key(Key::Char('j')),
            ]
        );
    }

    #[test]
    fn a_lone_escape_counts_only_once_nothing_follows_it() {
        assert_eq!(parse(&[b"\x1b"]), [Input::Key(Key::Escape)]);
        assert_eq!(parse(&[b"\x1b", b"[B"]), [Input::Key(Key::Down)]);
        assert_eq!(
            parse(&[b"\x1b\x1b"]),
            [Input::Key(Key::Escape), Input::Key(Key::Escape)]
        );
    }

    #[test]
    fn a_long_paste_keeps_its_end_marker_and_characters_whole_across_reads() {
        let body = "é".repeat(LONGEST);
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(b"\x1b[200~", &mut out);
        parser.feed(body.as_bytes(), &mut out);
        parser.feed(b"\x1b[20", &mut out);
        parser.feed(b"1~q", &mut out);
        let pasted: String = out
            .iter()
            .map(|input| match input {
                Input::Key(Key::Paste(text)) => text.as_str(),
                other => panic!("{other:?}"),
            })
            .take(out.len() - 1)
            .collect();
        assert_eq!(pasted, body);
        assert_eq!(out.last(), Some(&Input::Key(Key::Char('q'))));
    }

    #[test]
    fn a_paste_that_never_ends_gives_the_keys_back() {
        let mut parser = Parser::default();
        let mut out = Vec::new();
        parser.feed(b"\x1b[200~half a paste", &mut out);
        for _ in 0..PATIENCE {
            parser.idle(&mut out);
        }
        parser.feed(b"q", &mut out);
        assert_eq!(
            out,
            [
                Input::Key(Key::Paste("half a paste".into())),
                Input::Key(Key::Char('q'))
            ]
        );
    }

    #[test]
    fn the_wheel_arrives_as_input_and_clicks_and_releases_are_ignored() {
        assert_eq!(
            parse(&[b"\x1b[<0;12;5M\x1b[<0;12;5m\x1b[<64;1;1M\x1b[<65;1;1M\x1b[<2;3;3M"]),
            [Input::Wheel { up: true }, Input::Wheel { up: false }]
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
            parse(&[b"\x1bP>|rex 0.1\x1b\\\x1b_Gi=7;ENOENT\x1b\\\x1b[?2031;2$yq"]),
            [Input::Key(Key::Char('q'))]
        );
    }
}
