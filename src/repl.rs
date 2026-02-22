use crate::constants::*;
use crate::{load_json_file, save_json_file};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
#[cfg(unix)]
use std::{
    os::fd::{AsRawFd, RawFd},
    str,
};

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct InputHistoryState {
    pub entries: Vec<String>,
}

#[derive(Debug, Serialize)]
struct InputHistoryStateRef<'a> {
    entries: &'a [String],
}

pub struct InputSession {
    pub history_path: PathBuf,
    pub entries: Vec<String>,
}

impl InputSession {
    pub fn load(history_path: PathBuf) -> io::Result<Self> {
        let state = load_json_file::<InputHistoryState>(&history_path)?;
        Ok(Self {
            history_path,
            entries: state.entries,
        })
    }

    pub fn record_entry(&mut self, entry: &str) -> io::Result<()> {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        if self.entries.last().is_some_and(|last| last == entry) {
            return Ok(());
        }

        self.entries.push(entry.to_string());
        if self.entries.len() > MAX_INPUT_HISTORY_ENTRIES {
            let overflow = self.entries.len() - MAX_INPUT_HISTORY_ENTRIES;
            self.entries.drain(0..overflow);
        }

        save_json_file(
            &self.history_path,
            &InputHistoryStateRef {
                entries: &self.entries,
            },
        )
    }
}

/// Reads one prompt input and returns `Ok(None)` on EOF (e.g. Ctrl-D).
///
/// In TTY mode on Unix, this enables a simple line editor with Codex/Claude-like
/// keybinds:
/// - `Enter`: submit
/// - `Shift+Option+Enter` / `Alt+Enter`: insert newline
/// - `Ctrl+A` / `Ctrl+E`: move to line start/end
/// - `Ctrl+B` / `Ctrl+F`, arrows: cursor movement
/// - `Alt+B` / `Alt+F`: move by word
/// - `Ctrl+U` / `Ctrl+K`: delete to start/end
/// - `Up/Down` and `Ctrl+P/Ctrl+N`: input history
pub fn parse_input(prompt: &str, input_session: &mut InputSession) -> io::Result<Option<String>> {
    let stdin = io::stdin();

    #[cfg(unix)]
    {
        if is_tty(stdin.as_raw_fd()) {
            return parse_input_interactive(prompt, input_session);
        }
    }

    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut err_writer = stderr.lock();
    let input = parse_input_from(prompt, &mut reader, &mut writer, &mut err_writer)?;
    if let Some(entry) = input.as_deref()
        && let Err(err) = input_session.record_entry(entry)
    {
        eprintln!("Warning: failed to persist input history: {err}");
    }
    Ok(input)
}

pub(crate) fn parse_input_from<R, W, E>(
    prompt: &str,
    reader: &mut R,
    writer: &mut W,
    err_writer: &mut E,
) -> io::Result<Option<String>>
where
    R: BufRead,
    W: Write,
    E: Write,
{
    let mut input = String::new();
    let mut composed = String::new();
    let esc = '\u{1b}';
    const ESCAPED_ESC_SUFFIX: &str = "^[";

    loop {
        write!(writer, "{prompt}")?;
        writer.flush()?;

        input.clear();
        let bytes_read = reader.read_line(&mut input)?;
        if bytes_read == 0 {
            if !composed.trim().is_empty() {
                return Ok(Some(composed));
            }
            writeln!(writer)?;
            return Ok(None);
        }

        let line_without_newline = input.trim_end_matches(['\r', '\n']);
        let (line, continues) = if let Some(stripped) = line_without_newline.strip_suffix(esc) {
            (stripped, true)
        } else if let Some(stripped) = line_without_newline.strip_suffix(ESCAPED_ESC_SUFFIX) {
            // Some terminal/input-method combinations send Option+Enter as literal "^["
            // followed by newline instead of a raw ESC byte.
            (stripped, true)
        } else {
            (line_without_newline, false)
        };
        composed.push_str(line);

        if continues {
            composed.push('\n');
            continue;
        }

        if composed.trim().is_empty() {
            writeln!(err_writer, "Input cannot be empty. Please try again.")?;
            composed.clear();
            continue;
        }

        let normalized = std::mem::take(&mut composed).replace('\u{1b}', "");
        return Ok(Some(normalized));
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum KeyPress {
    Eof,
    EnterCr,
    EnterLf,
    EscPrefix,
    InsertNewline,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
    CtrlA,
    CtrlB,
    CtrlD,
    CtrlE,
    CtrlF,
    CtrlK,
    CtrlN,
    CtrlP,
    CtrlU,
    Char(char),
    Unknown,
}

#[cfg(unix)]
#[derive(Default, Clone)]
struct LineBuffer {
    text: String,
    cursor: usize,
}

#[cfg(unix)]
impl LineBuffer {
    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn set_text(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }

    fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor = previous_char_boundary(&self.text, self.cursor);
    }

    fn move_right(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        self.cursor = next_char_boundary(&self.text, self.cursor);
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    fn delete_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let previous = previous_char_boundary(&self.text, self.cursor);
        self.text.drain(previous..self.cursor);
        self.cursor = previous;
    }

    fn delete_right(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let next = next_char_boundary(&self.text, self.cursor);
        self.text.drain(self.cursor..next);
    }

    fn delete_to_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.text.drain(0..self.cursor);
        self.cursor = 0;
    }

    fn delete_to_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    fn move_word_left(&mut self) {
        let mut index = self.cursor;

        while index > 0 {
            let previous = previous_char_boundary(&self.text, index);
            let ch = self.text[previous..index].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            index = previous;
        }
        while index > 0 {
            let previous = previous_char_boundary(&self.text, index);
            let ch = self.text[previous..index].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            index = previous;
        }
        self.cursor = index;
    }

    fn move_word_right(&mut self) {
        let mut index = self.cursor;
        let len = self.text.len();

        while index < len {
            let next = next_char_boundary(&self.text, index);
            let ch = self.text[index..next].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            index = next;
        }
        while index < len {
            let next = next_char_boundary(&self.text, index);
            let ch = self.text[index..next].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            index = next;
        }
        self.cursor = index;
    }
}

#[cfg(unix)]
fn parse_input_interactive(
    prompt: &str,
    input_session: &mut InputSession,
) -> io::Result<Option<String>> {
    let stdin = io::stdin();
    let fd = stdin.as_raw_fd();
    let _raw_mode_guard = RawModeGuard::new(fd)?;
    let mut reader = RawFdReader(fd);
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    parse_input_interactive_loop(prompt, input_session, &mut reader, &mut writer, fd)
}

#[cfg(unix)]
fn parse_input_interactive_loop<R, W>(
    prompt: &str,
    input_session: &mut InputSession,
    reader: &mut R,
    writer: &mut W,
    fd: RawFd,
) -> io::Result<Option<String>>
where
    R: Read,
    W: Write,
{
    let mut buffer = LineBuffer::default();
    let mut previous_cursor_row = 0usize;
    let mut history_index: Option<usize> = None;
    let mut history_draft = String::new();
    let mut ignore_next_enter_lf = false;
    let mut pending_alt_prefix = false;

    previous_cursor_row = render_buffer(writer, prompt, &buffer, previous_cursor_row)?;

    loop {
        let mut key = read_key(reader, fd)?;
        if pending_alt_prefix {
            key = match key {
                KeyPress::EnterCr | KeyPress::EnterLf => KeyPress::InsertNewline,
                KeyPress::Char('b') | KeyPress::Char('B') => KeyPress::WordLeft,
                KeyPress::Char('f') | KeyPress::Char('F') => KeyPress::WordRight,
                KeyPress::Char('[') => read_escape_sequence_tail(reader, fd, b'[')?,
                KeyPress::Char('O') => read_escape_sequence_tail(reader, fd, b'O')?,
                other => other,
            };
            pending_alt_prefix = false;
        }

        if !matches!(key, KeyPress::EnterCr | KeyPress::EnterLf) {
            ignore_next_enter_lf = false;
        }

        match key {
            KeyPress::Eof => {
                writeln!(writer, "\n")?;
                writer.flush()?;
                return Ok(None);
            }
            KeyPress::EscPrefix => {
                pending_alt_prefix = true;
                continue;
            }
            KeyPress::EnterLf if ignore_next_enter_lf => {
                ignore_next_enter_lf = false;
            }
            KeyPress::EnterCr | KeyPress::EnterLf => {
                ignore_next_enter_lf = matches!(key, KeyPress::EnterCr);
                if buffer.text.ends_with("^[") {
                    buffer.text.truncate(buffer.text.len() - 2);
                    buffer.cursor = buffer.text.len();
                    buffer.insert_char('\n');
                    previous_cursor_row =
                        render_buffer(writer, prompt, &buffer, previous_cursor_row)?;
                    continue;
                }
                if buffer.text.trim().is_empty() {
                    writeln!(writer, "\nInput cannot be empty. Please try again.\n")?;
                    buffer.clear();
                    history_index = None;
                    history_draft.clear();
                    previous_cursor_row = 0;
                } else {
                    let submitted = buffer.text.replace('\u{1b}', "");
                    writeln!(writer, "\n")?;
                    writer.flush()?;
                    if let Err(err) = input_session.record_entry(&submitted) {
                        eprintln!("Warning: failed to persist input history: {err}");
                    }
                    return Ok(Some(submitted));
                }
            }
            KeyPress::InsertNewline => {
                ignore_next_enter_lf = false;
                if history_index.is_some() {
                    history_index = None;
                    history_draft.clear();
                }
                buffer.insert_char('\n');
            }
            KeyPress::Backspace => {
                if history_index.is_some() {
                    history_index = None;
                    history_draft.clear();
                }
                buffer.delete_left();
            }
            KeyPress::Delete => {
                if history_index.is_some() {
                    history_index = None;
                    history_draft.clear();
                }
                // Some terminals only emit "Delete" for the key users expect as backspace.
                // If the cursor is at EOL, treat it like backspace for practical behavior.
                if buffer.cursor >= buffer.text.len() {
                    buffer.delete_left();
                } else {
                    buffer.delete_right();
                }
            }
            KeyPress::Left | KeyPress::CtrlB => buffer.move_left(),
            KeyPress::Right | KeyPress::CtrlF => buffer.move_right(),
            KeyPress::Home | KeyPress::CtrlA => buffer.move_home(),
            KeyPress::End | KeyPress::CtrlE => buffer.move_end(),
            KeyPress::WordLeft => buffer.move_word_left(),
            KeyPress::WordRight => buffer.move_word_right(),
            KeyPress::CtrlU => {
                if history_index.is_some() {
                    history_index = None;
                    history_draft.clear();
                }
                buffer.delete_to_start();
            }
            KeyPress::CtrlK => {
                if history_index.is_some() {
                    history_index = None;
                    history_draft.clear();
                }
                buffer.delete_to_end();
            }
            KeyPress::CtrlD => {
                if buffer.text.is_empty() {
                    writeln!(writer, "\n")?;
                    writer.flush()?;
                    return Ok(None);
                }
                if history_index.is_some() {
                    history_index = None;
                    history_draft.clear();
                }
                buffer.delete_right();
            }
            KeyPress::Up | KeyPress::CtrlP => {
                if !input_session.entries.is_empty() {
                    match history_index {
                        None => {
                            history_draft = buffer.text.clone();
                            history_index = Some(input_session.entries.len() - 1);
                        }
                        Some(index) if index > 0 => {
                            history_index = Some(index - 1);
                        }
                        _ => {}
                    }
                    if let Some(index) = history_index {
                        buffer.set_text(input_session.entries[index].clone());
                    }
                }
            }
            KeyPress::Down | KeyPress::CtrlN => {
                if let Some(index) = history_index {
                    if index + 1 < input_session.entries.len() {
                        let next = index + 1;
                        history_index = Some(next);
                        buffer.set_text(input_session.entries[next].clone());
                    } else {
                        history_index = None;
                        buffer.set_text(history_draft.clone());
                    }
                }
            }
            KeyPress::Char(ch) => {
                if history_index.is_some() {
                    history_index = None;
                    history_draft.clear();
                }
                buffer.insert_char(ch);
            }
            KeyPress::Unknown => {}
        }

        previous_cursor_row = render_buffer(writer, prompt, &buffer, previous_cursor_row)?;
    }
}

#[cfg(unix)]
fn previous_char_boundary(content: &str, index: usize) -> usize {
    content[..index]
        .char_indices()
        .last()
        .map(|(offset, _)| offset)
        .unwrap_or(0)
}

#[cfg(unix)]
fn next_char_boundary(content: &str, index: usize) -> usize {
    content[index..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| index + offset)
        .unwrap_or(content.len())
}

#[cfg(unix)]
fn render_buffer<W: Write>(
    writer: &mut W,
    prompt: &str,
    buffer: &LineBuffer,
    previous_cursor_row: usize,
) -> io::Result<usize> {
    write!(writer, "\r")?;
    if previous_cursor_row > 0 {
        write!(writer, "\x1b[{}A", previous_cursor_row)?;
    }
    write!(writer, "\x1b[J")?;
    write!(writer, "{prompt}{}", buffer.text)?;

    let rendered = format!("{prompt}{}", buffer.text);
    let mut safe_cursor = buffer.cursor.min(buffer.text.len());
    while safe_cursor > 0 && !buffer.text.is_char_boundary(safe_cursor) {
        safe_cursor -= 1;
    }
    debug_assert!(buffer.text.is_char_boundary(safe_cursor));
    let rendered_to_cursor = format!("{prompt}{}", &buffer.text[..safe_cursor]);
    let (end_row, _) = display_cursor_position(&rendered);
    let (cursor_row, cursor_col) = display_cursor_position(&rendered_to_cursor);

    if end_row > cursor_row {
        write!(writer, "\x1b[{}A", end_row - cursor_row)?;
    }
    write!(writer, "\r")?;
    if cursor_col > 0 {
        write!(writer, "\x1b[{}C", cursor_col)?;
    }
    writer.flush()?;

    Ok(cursor_row)
}

#[cfg(unix)]
pub(crate) fn display_cursor_position(content: &str) -> (usize, usize) {
    let mut row = 0usize;
    let mut col = 0usize;
    for ch in content.chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += char_display_width(ch);
        }
    }
    (row, col)
}

#[cfg(unix)]
fn char_display_width(ch: char) -> usize {
    if ch.is_control() {
        return 0;
    }
    if matches!(
        ch,
        '\u{1100}'..='\u{115F}'
            | '\u{2329}'..='\u{232A}'
            | '\u{2E80}'..='\u{A4CF}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE10}'..='\u{FE19}'
            | '\u{FE30}'..='\u{FE6F}'
            | '\u{FF00}'..='\u{FF60}'
            | '\u{FFE0}'..='\u{FFE6}'
            | '\u{1F300}'..='\u{1FAFF}'
            | '\u{20000}'..='\u{2FFFD}'
            | '\u{30000}'..='\u{3FFFD}'
    ) {
        2
    } else {
        1
    }
}

#[cfg(unix)]
fn read_key<R: Read>(reader: &mut R, fd: RawFd) -> io::Result<KeyPress> {
    let mut first = [0u8; 1];
    let bytes_read = reader.read(&mut first)?;
    if bytes_read == 0 {
        return Ok(KeyPress::Eof);
    }

    match first[0] {
        b'\r' => Ok(KeyPress::EnterCr),
        b'\n' => Ok(KeyPress::EnterLf),
        0x7f | 0x08 => Ok(KeyPress::Backspace),
        0x01 => Ok(KeyPress::CtrlA),
        0x02 => Ok(KeyPress::CtrlB),
        0x04 => Ok(KeyPress::CtrlD),
        0x05 => Ok(KeyPress::CtrlE),
        0x06 => Ok(KeyPress::CtrlF),
        0x0b => Ok(KeyPress::CtrlK),
        0x0e => Ok(KeyPress::CtrlN),
        0x10 => Ok(KeyPress::CtrlP),
        0x15 => Ok(KeyPress::CtrlU),
        0x1b => parse_escape_sequence(reader, fd),
        first_byte => decode_utf8_char(first_byte, reader, fd),
    }
}

#[cfg(unix)]
fn decode_utf8_char<R: Read>(first_byte: u8, reader: &mut R, fd: RawFd) -> io::Result<KeyPress> {
    let width = match first_byte {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Ok(KeyPress::Unknown),
    };

    let mut bytes = [0u8; 4];
    bytes[0] = first_byte;
    if width > 1 {
        for slot in bytes.iter_mut().take(width).skip(1) {
            let Some(byte) =
                read_byte_with_timeout(reader, fd, ESCAPE_SEQUENCE_FOLLOWUP_TIMEOUT_MS)?
            else {
                return Ok(KeyPress::Unknown);
            };
            if (byte & 0b1100_0000) != 0b1000_0000 {
                return Ok(KeyPress::Unknown);
            }
            *slot = byte;
        }
    }

    let decoded = str::from_utf8(&bytes[..width])
        .ok()
        .and_then(|value| value.chars().next())
        .map(KeyPress::Char)
        .unwrap_or(KeyPress::Unknown);
    Ok(decoded)
}

#[cfg(unix)]
fn read_byte_with_timeout<R: Read>(
    reader: &mut R,
    fd: RawFd,
    timeout_ms: i32,
) -> io::Result<Option<u8>> {
    if !stdin_readable(fd, timeout_ms)? {
        return Ok(None);
    }
    let mut byte = [0u8; 1];
    let bytes_read = reader.read(&mut byte)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    Ok(Some(byte[0]))
}

#[cfg(unix)]
fn parse_escape_sequence<R: Read>(reader: &mut R, fd: RawFd) -> io::Result<KeyPress> {
    if !stdin_readable(fd, ESCAPE_SEQUENCE_FIRST_BYTE_TIMEOUT_MS)? {
        return Ok(KeyPress::EscPrefix);
    }

    let mut first = [0u8; 1];
    let bytes_read = reader.read(&mut first)?;
    if bytes_read == 0 {
        return Ok(KeyPress::EscPrefix);
    }

    read_escape_sequence_tail(reader, fd, first[0])
}

#[cfg(unix)]
fn read_escape_sequence_tail<R: Read>(
    reader: &mut R,
    fd: RawFd,
    first_byte: u8,
) -> io::Result<KeyPress> {
    let mut sequence = Vec::with_capacity(16);
    sequence.push(first_byte);
    while sequence.len() < 16 && stdin_readable(fd, ESCAPE_SEQUENCE_FOLLOWUP_TIMEOUT_MS)? {
        let mut next = [0u8; 1];
        let bytes_read = reader.read(&mut next)?;
        if bytes_read == 0 {
            break;
        }
        sequence.push(next[0]);
    }
    Ok(map_escape_sequence(sequence.as_slice()))
}

#[cfg(unix)]
pub(crate) fn map_escape_sequence(sequence: &[u8]) -> KeyPress {
    match sequence {
        [b'\r'] | [b'\n'] | [b'\r', b'\n'] | [b'\n', b'\r'] => KeyPress::InsertNewline,
        // CSI-u keyboard protocol: Enter and modified Enter
        [b'[', b'1', b'3', b'u'] => KeyPress::EnterLf,
        [b'[', b'1', b'3', b';', b'2', b'u']
        | [b'[', b'1', b'3', b';', b'3', b'u']
        | [b'[', b'1', b'3', b';', b'4', b'u']
        | [b'[', b'1', b'3', b';', b'5', b'u']
        | [b'[', b'1', b'3', b';', b'6', b'u']
        | [b'[', b'1', b'3', b';', b'7', b'u']
        | [b'[', b'1', b'3', b';', b'8', b'u'] => KeyPress::InsertNewline,
        // xterm modifyOtherKeys for Enter with modifiers
        [b'[', b'2', b'7', b';', b'2', b';', b'1', b'3', b'~']
        | [b'[', b'2', b'7', b';', b'3', b';', b'1', b'3', b'~']
        | [b'[', b'2', b'7', b';', b'4', b';', b'1', b'3', b'~']
        | [b'[', b'2', b'7', b';', b'5', b';', b'1', b'3', b'~']
        | [b'[', b'2', b'7', b';', b'6', b';', b'1', b'3', b'~']
        | [b'[', b'2', b'7', b';', b'7', b';', b'1', b'3', b'~']
        | [b'[', b'2', b'7', b';', b'8', b';', b'1', b'3', b'~'] => KeyPress::InsertNewline,
        [0x7f] | [0x08] => KeyPress::Backspace,
        [b'b'] | [b'B'] => KeyPress::WordLeft,
        [b'f'] | [b'F'] => KeyPress::WordRight,
        [b'[', b'A'] => KeyPress::Up,
        [b'[', b'B'] => KeyPress::Down,
        [b'[', b'C'] => KeyPress::Right,
        [b'[', b'D'] => KeyPress::Left,
        [b'[', b'H'] | [b'O', b'H'] | [b'[', b'1', b'~'] | [b'[', b'7', b'~'] => KeyPress::Home,
        [b'[', b'F'] | [b'O', b'F'] | [b'[', b'4', b'~'] | [b'[', b'8', b'~'] => KeyPress::End,
        [b'[', b'3', b'~']
        | [b'[', b'3', b';', b'2', b'~']
        | [b'[', b'3', b';', b'3', b'~']
        | [b'[', b'3', b';', b'5', b'~'] => KeyPress::Delete,
        [b'[', b'1', b';', b'3', b'D'] | [b'[', b'1', b';', b'5', b'D'] => KeyPress::WordLeft,
        [b'[', b'1', b';', b'3', b'C'] | [b'[', b'1', b';', b'5', b'C'] => KeyPress::WordRight,
        [b'O', b'A'] => KeyPress::Up,
        [b'O', b'B'] => KeyPress::Down,
        [b'O', b'C'] => KeyPress::Right,
        [b'O', b'D'] => KeyPress::Left,
        [b'[', .., b'A'] => KeyPress::Up,
        [b'[', .., b'B'] => KeyPress::Down,
        [b'[', .., b'C'] => KeyPress::Right,
        [b'[', .., b'D'] => KeyPress::Left,
        _ => KeyPress::Unknown,
    }
}

#[cfg(unix)]
fn stdin_readable(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let status = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if status < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(status > 0 && (poll_fd.revents & libc::POLLIN) != 0)
}

#[cfg(unix)]
fn is_tty(fd: RawFd) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}

/// Unbuffered reader that reads directly from a raw file descriptor.
///
/// Unlike `StdinLock` (which wraps a `BufReader`), this reader issues a
/// `read(2)` syscall for each `Read::read` call. This is critical for the
/// escape-sequence parser: we rely on `poll(2)` to detect whether more bytes
/// are pending in the kernel buffer. A buffered reader can silently drain the
/// kernel buffer into user-space, causing `poll` to report "no data" even
/// though bytes are still available in the `BufReader` – which makes
/// multi-byte escape sequences (e.g. arrow keys: ESC [ A) appear as
/// individual characters.
#[cfg(unix)]
struct RawFdReader(RawFd);

#[cfg(unix)]
impl Read for RawFdReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.0, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

#[cfg(unix)]
struct RawModeGuard {
    fd: RawFd,
    original: libc::termios,
}

#[cfg(unix)]
impl RawModeGuard {
    fn new(fd: RawFd) -> io::Result<Self> {
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }

        let mut raw = original;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_iflag &= !(libc::IXON | libc::ICRNL);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_json_file;
    use std::fs;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn returns_trimmed_input() {
        let mut reader = Cursor::new(b"hello\n");
        let mut out = Vec::new();
        let mut err = Vec::new();

        let input = parse_input_from(">> ", &mut reader, &mut out, &mut err).unwrap();

        assert_eq!(input, Some("hello".to_string()));
        assert_eq!(String::from_utf8(out).unwrap(), ">> ");
        assert!(err.is_empty());
    }

    #[test]
    fn prompts_again_on_empty_input() {
        let mut reader = Cursor::new(b"\n  \nfinal input\n");
        let mut out = Vec::new();
        let mut err = Vec::new();

        let input = parse_input_from(">> ", &mut reader, &mut out, &mut err).unwrap();

        assert_eq!(input, Some("final input".to_string()));
        assert_eq!(String::from_utf8(out).unwrap(), ">> >> >> ");
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "Input cannot be empty. Please try again.\nInput cannot be empty. Please try again.\n"
        );
    }

    #[test]
    fn returns_none_on_eof() {
        let mut reader = Cursor::new(Vec::<u8>::new());
        let mut out = Vec::new();
        let mut err = Vec::new();

        let input = parse_input_from(">> ", &mut reader, &mut out, &mut err).unwrap();

        assert_eq!(input, None);
        assert_eq!(String::from_utf8(out).unwrap(), ">> \n");
        assert!(err.is_empty());
    }

    #[test]
    fn supports_multiline_input_when_line_ends_with_escape() {
        let mut reader = Cursor::new(b"first line\x1b\nsecond line\n");
        let mut out = Vec::new();
        let mut err = Vec::new();

        let input = parse_input_from(">> ", &mut reader, &mut out, &mut err).unwrap();

        assert_eq!(input, Some("first line\nsecond line".to_string()));
        assert_eq!(String::from_utf8(out).unwrap(), ">> >> ");
        assert!(err.is_empty());
    }

    #[test]
    fn input_session_record_entry_ignores_empty_and_whitespace() {
        let temp = make_temp_dir("input-history-empty");
        let history_path = temp.path().join("history.json");
        let mut session = InputSession {
            history_path: history_path.clone(),
            entries: Vec::new(),
        };

        session.record_entry("").unwrap();
        session.record_entry("   ").unwrap();

        assert!(session.entries.is_empty());
        assert!(!history_path.exists());
    }

    #[test]
    fn input_session_record_entry_skips_duplicate_consecutive() {
        let temp = make_temp_dir("input-history-dup");
        let history_path = temp.path().join("history.json");
        let mut session = InputSession {
            history_path: history_path.clone(),
            entries: vec!["same".to_string()],
        };

        session.record_entry("same").unwrap();

        assert_eq!(session.entries, vec!["same".to_string()]);
        assert!(!history_path.exists());
    }

    #[test]
    fn input_session_record_entry_trims_to_max_entries() {
        let temp = make_temp_dir("input-history-limit");
        let history_path = temp.path().join("history.json");
        let mut entries = Vec::with_capacity(MAX_INPUT_HISTORY_ENTRIES);
        for i in 0..MAX_INPUT_HISTORY_ENTRIES {
            entries.push(format!("entry-{i}"));
        }
        let mut session = InputSession {
            history_path: history_path.clone(),
            entries,
        };

        session.record_entry("entry-extra").unwrap();

        assert_eq!(session.entries.len(), MAX_INPUT_HISTORY_ENTRIES);
        assert_eq!(session.entries.first().unwrap(), "entry-1");
        assert_eq!(session.entries.last().unwrap(), "entry-extra");

        let state = load_json_file::<InputHistoryState>(&history_path).unwrap();
        assert_eq!(state.entries, session.entries);
    }

    #[cfg(unix)]
    #[test]
    fn maps_ctrl_shift_enter_escape_sequence_to_insert_newline() {
        let key = map_escape_sequence(b"[13;6u");
        assert!(matches!(key, KeyPress::InsertNewline));
    }

    #[cfg(unix)]
    #[test]
    fn maps_xterm_modify_other_keys_enter_to_insert_newline() {
        let key = map_escape_sequence(b"[27;6;13~");
        assert!(matches!(key, KeyPress::InsertNewline));
    }

    #[cfg(unix)]
    #[test]
    fn maps_crlf_escape_sequence_to_insert_newline() {
        let key = map_escape_sequence(b"\r\n");
        assert!(matches!(key, KeyPress::InsertNewline));
    }

    #[cfg(unix)]
    #[test]
    fn maps_arrow_right_escape_sequence() {
        let key = map_escape_sequence(b"[C");
        assert!(matches!(key, KeyPress::Right));
    }

    #[cfg(unix)]
    #[test]
    fn maps_arrow_left_escape_sequence() {
        let key = map_escape_sequence(b"[D");
        assert!(matches!(key, KeyPress::Left));
    }

    #[cfg(unix)]
    #[test]
    fn maps_application_cursor_left_escape_sequence() {
        let key = map_escape_sequence(b"OD");
        assert!(matches!(key, KeyPress::Left));
    }

    #[cfg(unix)]
    #[test]
    fn maps_modified_arrow_left_escape_sequence() {
        let key = map_escape_sequence(b"[1;2D");
        assert!(matches!(key, KeyPress::Left));
    }

    #[cfg(unix)]
    #[test]
    fn display_cursor_position_counts_full_width_characters() {
        let (row, col) = display_cursor_position("あい");
        assert_eq!(row, 0);
        assert_eq!(col, 4);
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn make_temp_dir(prefix: &str) -> TempDir {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("socium_{prefix}_{nanos}_{count}"));
        fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }
}
