//! Keys, from a script or from a terminal.
//!
//! # One event type, two sources
//!
//! The state machine in [`super`] never learns where a key came from. A
//! scripted `tab` and a typed 0x09 are the same [`Event`], which is the whole
//! reason a headless run and a keyboard run can be compared at all: if the
//! script were interpreted anywhere other than here, "the same keys" would
//! stop meaning the same thing.
//!
//! # Raw mode, and the one external program
//!
//! An interactive run needs the terminal to hand over each key as it is
//! pressed rather than a line at a time, and to stop echoing. That is
//! `stty raw -echo`, and `stty` is the only external program this half of
//! Catalyst runs. No shell is involved: the program is named directly and its
//! arguments are fixed words, never anything a user typed.
//!
//! `stty -g` does double duty. It prints the current settings, which is what
//! gets put back on the way out, and it *fails* when standard input is not a
//! terminal -- which is how this build answers "is there a terminal here?"
//! without a dependency and without `unsafe`.
//!
//! # Why a reader thread
//!
//! Escape is ambiguous: pressed alone it is 0x1b, and it is also the first
//! byte of every arrow key. Telling them apart means asking "is another byte
//! coming *soon*", which a blocking read cannot express. A thread that feeds
//! bytes into a channel can: the decoder waits with a deadline for the second
//! byte, and a key that does not arrive within it was a lone escape.

use crate::cli::Refused;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// The only external program this build runs for the terminal front end.
const TERMINAL_SETTINGS: &str = "stty";

/// How long a second byte has to arrive before an escape counts as a lone
/// one. Long enough for a terminal to deliver the rest of an arrow key,
/// short enough that a person pressing escape does not notice a pause.
const ESCAPE_GRACE: Duration = Duration::from_millis(50);

/// One key, whatever produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Text(String),
    Enter,
    Tab,
    BackTab,
    Up,
    Down,
    Backspace,
    Esc,
    CtrlS,
    CtrlC,
}

/// Read a key script: one event per line, exactly the vocabulary of
/// `docs/interface.md` §4.
///
/// `text:` keeps everything after the colon verbatim, spaces included, because
/// a unit may be ` N s/m` and trimming it would be this reader deciding what
/// the user meant. An event this build does not know is refused rather than
/// skipped: a silently ignored key makes a script that half ran look like a
/// script that ran.
pub fn parse_script(text: &str) -> Result<Vec<Event>, Refused> {
    let mut events = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("text:") {
            events.push(Event::Text(rest.to_owned()));
            continue;
        }
        events.push(match line.trim() {
            "enter" => Event::Enter,
            "tab" => Event::Tab,
            "backtab" => Event::BackTab,
            "up" => Event::Up,
            "down" => Event::Down,
            "backspace" => Event::Backspace,
            "esc" => Event::Esc,
            "ctrl-s" => Event::CtrlS,
            "ctrl-c" => Event::CtrlC,
            _ => {
                return Err(Refused::new(
                    "catalyst.syntax",
                    format!(
                        "line {} of the key script is not an event this build knows",
                        index + 1
                    ),
                    "use one event per line: text:<characters>, enter, tab, backtab, up, \
                     down, backspace, esc, ctrl-s or ctrl-c",
                ))
            }
        });
    }
    Ok(events)
}

/// The terminal, put into raw mode and owed a restoration.
pub struct RawTerminal {
    saved: String,
}

impl RawTerminal {
    /// Capture the current settings and hand the terminal over key by key.
    /// Refuses when standard input is not a terminal, which is the same
    /// question `stty -g` answers by failing.
    pub fn enter() -> Result<RawTerminal, Refused> {
        let saved = settings()?;
        apply(&["raw", "-echo"])?;
        Ok(RawTerminal { saved })
    }

    /// Put back exactly what was there. Best effort on purpose: this runs on
    /// the way out, including the way out of a refusal, and a failure to
    /// restore must not replace the answer the user was about to read.
    pub fn restore(&self) {
        let _ = apply(&[self.saved.as_str()]);
    }
}

fn settings() -> Result<String, Refused> {
    let output = Command::new(TERMINAL_SETTINGS)
        .arg("-g")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(done) if done.status.success() => {
            Ok(String::from_utf8_lossy(&done.stdout).trim().to_owned())
        }
        _ => Err(no_terminal()),
    }
}

fn apply(arguments: &[&str]) -> Result<(), Refused> {
    let status = Command::new(TERMINAL_SETTINGS)
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(done) if done.success() => Ok(()),
        _ => Err(no_terminal()),
    }
}

/// The terminal's own size, rows and columns, from `stty size` -- the same
/// program the raw mode already depends on, asked one more question. `None`
/// when standard input is not a terminal or the answer is not two numbers,
/// and a zero in either place counts as no answer, which is what a terminal
/// that has not been sized yet reports.
pub fn terminal_size() -> Option<(usize, usize)> {
    let output = Command::new(TERMINAL_SETTINGS)
        .arg("size")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    let rows: usize = parts.next()?.parse().ok()?;
    let columns: usize = parts.next()?.parse().ok()?;
    if rows == 0 || columns == 0 || parts.next().is_some() {
        return None;
    }
    Some((columns, rows))
}

pub fn no_terminal() -> Refused {
    Refused::new(
        "catalyst.not_a_terminal",
        "the interactive flow needs a terminal on standard input, and this run has none",
        "run `catalyst tui` from a terminal, or drive it with `--keys FILE`, adding \
         --headless --transcript FILE for the plain-text renderer",
    )
}

/// Start reading standard input a byte at a time, into a channel.
pub fn keyboard() -> Receiver<u8> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        let mut byte = [0u8; 1];
        while matches!(input.read(&mut byte), Ok(1)) {
            if sender.send(byte[0]).is_err() {
                break;
            }
        }
    });
    receiver
}

/// The next key, or `None` at end of input.
pub fn next_event(bytes: &Receiver<u8>) -> Option<Event> {
    loop {
        let first = bytes.recv().ok()?;
        let event = match first {
            0x03 => Some(Event::CtrlC),
            0x13 => Some(Event::CtrlS),
            b'\r' | b'\n' => Some(Event::Enter),
            0x09 => Some(Event::Tab),
            0x7f | 0x08 => Some(Event::Backspace),
            0x1b => Some(escape(bytes)),
            byte if byte >= 0x20 => character(byte, bytes).map(Event::Text),
            // Every other control byte is a key this flow has no use for.
            // Ignoring it is right; reporting it would be noise on a screen
            // the user is reading for something else.
            _ => None,
        };
        if let Some(event) = event {
            return Some(event);
        }
    }
}

/// An escape, which is either a key on its own or the start of a sequence.
///
/// The sequence form is `ESC [ … final`, where the final byte is the one in
/// 0x40..=0x7e. Reading up to it keeps the parameters of a sequence this
/// build ignores from arriving later as if they had been typed.
fn escape(bytes: &Receiver<u8>) -> Event {
    match bytes.recv_timeout(ESCAPE_GRACE) {
        Err(_) => Event::Esc,
        Ok(b'[') => {
            let mut final_byte = None;
            while let Ok(byte) = bytes.recv_timeout(ESCAPE_GRACE) {
                if (0x40..=0x7e).contains(&byte) {
                    final_byte = Some(byte);
                    break;
                }
            }
            match final_byte {
                Some(b'A') => Event::Up,
                Some(b'B') => Event::Down,
                Some(b'Z') => Event::BackTab,
                _ => Event::Esc,
            }
        }
        Ok(_) => Event::Esc,
    }
}

/// One character, gathering the continuation bytes of a multi-byte one.
fn character(first: u8, bytes: &Receiver<u8>) -> Option<String> {
    let extra = match first {
        b if b < 0x80 => 0,
        b if b >> 5 == 0b110 => 1,
        b if b >> 4 == 0b1110 => 2,
        b if b >> 3 == 0b11110 => 3,
        _ => return None,
    };
    let mut buffer = vec![first];
    for _ in 0..extra {
        buffer.push(bytes.recv_timeout(ESCAPE_GRACE).ok()?);
    }
    String::from_utf8(buffer).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_of_the_vocabulary_reads_back() {
        let script = "text:func f(x) = x\nenter\ntab\nbacktab\nup\ndown\nbackspace\nesc\n\
                      ctrl-s\nctrl-c\n";
        assert_eq!(
            parse_script(script).expect("a valid script"),
            vec![
                Event::Text("func f(x) = x".to_owned()),
                Event::Enter,
                Event::Tab,
                Event::BackTab,
                Event::Up,
                Event::Down,
                Event::Backspace,
                Event::Esc,
                Event::CtrlS,
                Event::CtrlC,
            ]
        );
    }

    #[test]
    fn text_keeps_what_follows_the_colon_including_nothing_at_all() {
        assert_eq!(
            parse_script("text:\ntext: N/m \n").expect("a valid script"),
            vec![Event::Text(String::new()), Event::Text(" N/m ".to_owned())]
        );
    }

    #[test]
    fn an_unknown_event_is_refused_with_the_line_and_the_vocabulary() {
        let refusal = parse_script("enter\nwiggle\n").expect_err("refused");
        assert_eq!(refusal.code, "catalyst.syntax");
        assert!(refusal.detail.contains("line 2"), "{}", refusal.detail);
        assert!(refusal.remedy.contains("backtab"), "{}", refusal.remedy);
    }
}
