//! Windows local console PAL.

use std::io;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
    ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WINDOW_INPUT, ENABLE_WRAP_AT_EOL_OUTPUT,
    GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, INPUT_RECORD, KEY_EVENT,
    PeekConsoleInputW, ReadConsoleInputW, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    SetConsoleCtrlHandler, SetConsoleMode, WINDOW_BUFFER_SIZE_EVENT,
};
use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::local_console::{ConsoleInput, LocalConsole};
use crate::pal::pseudoconsole::WindowSize;

/// Real Windows console attached to this process.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetConsole;

/// One console `ReadFile` burst. Larger than a typical key or CSI sequence;
/// `ReadFile` may return less. Not a protocol bound.
const INPUT_READ_BUF: usize = 4096;

fn saved_modes() -> &'static Mutex<Option<(CONSOLE_MODE, CONSOLE_MODE)>> {
    static SAVED: OnceLock<Mutex<Option<(CONSOLE_MODE, CONSOLE_MODE)>>> = OnceLock::new();
    SAVED.get_or_init(|| Mutex::new(None))
}

fn std_handle(kind: STD_HANDLE) -> Result<HANDLE, PalError> {
    // SAFETY: GetStdHandle returns a process-lifetime handle that this process
    // does not own or close.
    let handle =
        unsafe { GetStdHandle(kind) }.map_err(|_error| PalError::new(PalErrorKind::Other))?;
    if handle.is_invalid() {
        return Err(PalError::new(PalErrorKind::Other));
    }
    Ok(handle)
}

fn console_mode(handle: HANDLE) -> Result<CONSOLE_MODE, PalError> {
    let mut mode = CONSOLE_MODE(0);
    // SAFETY: `handle` is a standard handle from `std_handle`; `mode` is a
    // stack value that outlives the call.
    unsafe { GetConsoleMode(handle, &raw mut mode) }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;
    Ok(mode)
}

/// Restores one standard handle's console mode, best effort.
fn restore_mode(kind: STD_HANDLE, mode: CONSOLE_MODE) -> Result<(), PalError> {
    let handle = std_handle(kind)?;
    // SAFETY: `handle` is a standard console handle; `mode` was captured from
    // it before `enter_raw_relay` changed it.
    unsafe { SetConsoleMode(handle, mode) }.map_err(|_error| PalError::new(PalErrorKind::Other))
}

fn read_window_size(output: HANDLE) -> Result<WindowSize, PalError> {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: `output` is a console handle; `info` is a stack structure that
    // outlives the call.
    unsafe { GetConsoleScreenBufferInfo(output, &raw mut info) }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;
    let width = info
        .srWindow
        .Right
        .checked_sub(info.srWindow.Left)
        .and_then(|delta| delta.checked_add(1))
        .unwrap_or(1);
    let height = info
        .srWindow
        .Bottom
        .checked_sub(info.srWindow.Top)
        .and_then(|delta| delta.checked_add(1))
        .unwrap_or(1);
    Ok(WindowSize {
        cols: u16::try_from(width.max(1)).unwrap_or(u16::MAX),
        rows: u16::try_from(height.max(1)).unwrap_or(u16::MAX),
    })
}

fn event_kind(record: &INPUT_RECORD) -> u32 {
    u32::from(record.EventType)
}

fn peek_input(handle: HANDLE) -> Result<([INPUT_RECORD; 16], usize), PalError> {
    let mut peek = [INPUT_RECORD::default(); 16];
    let mut count = 0_u32;
    // SAFETY: `handle` is stdin; `peek` is exclusive for this call.
    unsafe { PeekConsoleInputW(handle, &mut peek, &raw mut count) }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;
    Ok((peek, count as usize))
}

fn consume_records(handle: HANDLE, count: usize) -> Result<(), PalError> {
    if count == 0 {
        return Ok(());
    }
    let mut discarded = vec![INPUT_RECORD::default(); count];
    let mut read = 0_u32;
    // SAFETY: `discarded` is exclusive and large enough for `count`.
    unsafe { ReadConsoleInputW(handle, &mut discarded, &raw mut read) }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;
    Ok(())
}

/// Consumes leading `WINDOW_BUFFER_SIZE_EVENT` records so a later `ReadFile`
/// is not blocked behind them. Window changes are console input records, not
/// VT bytes, which is why attach cannot learn resizes from `ReadFile` alone.
fn take_leading_resize(handle: HANDLE) -> Result<Option<WindowSize>, PalError> {
    let (peek, count) = peek_input(handle)?;
    let leading_resizes = peek
        .iter()
        .take(count)
        .take_while(|record| event_kind(record) == WINDOW_BUFFER_SIZE_EVENT)
        .count();
    if leading_resizes == 0 {
        return Ok(None);
    }
    consume_records(handle, leading_resizes)?;
    let output = std_handle(STD_OUTPUT_HANDLE)?;
    read_window_size(output).map(Some)
}

/// Drops focus/menu/mouse records so they cannot hide a later resize or key.
fn discard_leading_noise(handle: HANDLE) -> Result<bool, PalError> {
    let (peek, count) = peek_input(handle)?;
    let leading_noise = peek
        .iter()
        .take(count)
        .take_while(|record| {
            let kind = event_kind(record);
            kind != WINDOW_BUFFER_SIZE_EVENT && kind != KEY_EVENT
        })
        .count();
    if leading_noise == 0 {
        return Ok(false);
    }
    consume_records(handle, leading_noise)?;
    Ok(true)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl LocalConsole for BuildTargetConsole {
    fn has_console(&self) -> bool {
        std_handle(STD_OUTPUT_HANDLE)
            .ok()
            .and_then(|handle| console_mode(handle).ok())
            .is_some()
    }

    fn stdin_is_terminal(&self) -> bool {
        std_handle(STD_INPUT_HANDLE)
            .ok()
            .and_then(|handle| console_mode(handle).ok())
            .is_some()
    }

    fn disable_ctrl_c_handler(&self) -> Result<(), PalError> {
        // SAFETY: a null handler with Add=TRUE tells Windows to ignore control
        // signals in this process so Ctrl+C is delivered as console input.
        unsafe { SetConsoleCtrlHandler(None, true) }
            .map_err(|_error| PalError::new(PalErrorKind::Other))
    }

    fn enter_raw_relay(&self) -> Result<(), PalError> {
        let input = std_handle(STD_INPUT_HANDLE)?;
        let output = std_handle(STD_OUTPUT_HANDLE)?;
        let in_mode = console_mode(input)?;
        let out_mode = console_mode(output)?;
        {
            let mut saved = saved_modes()
                .lock()
                .expect("saved console modes are only copied, never held across a panic");
            if saved.is_none() {
                *saved = Some((in_mode, out_mode));
            }
        }
        // Disable cooked input so keystrokes reach the app immediately. Enable
        // VT input for CSI sequences and window-input so resizes appear as
        // `WINDOW_BUFFER_SIZE_EVENT` records rather than being dropped.
        let raw_in = CONSOLE_MODE(
            (in_mode.0 & !(ENABLE_ECHO_INPUT.0 | ENABLE_LINE_INPUT.0 | ENABLE_PROCESSED_INPUT.0))
                | ENABLE_VIRTUAL_TERMINAL_INPUT.0
                | ENABLE_WINDOW_INPUT.0,
        );
        // VT processing plus wrap so the local host renders the same sequences
        // the app writes through ConPTY.
        let raw_out = CONSOLE_MODE(
            out_mode.0
                | ENABLE_VIRTUAL_TERMINAL_PROCESSING.0
                | ENABLE_PROCESSED_OUTPUT.0
                | ENABLE_WRAP_AT_EOL_OUTPUT.0,
        );
        // SAFETY: `input` is the process stdin console handle; `raw_in` is a
        // combination of documented console mode flags.
        unsafe { SetConsoleMode(input, raw_in) }
            .map_err(|_error| PalError::new(PalErrorKind::Other))?;
        // SAFETY: `output` is the process stdout console handle; `raw_out` is a
        // combination of documented console mode flags.
        unsafe { SetConsoleMode(output, raw_out) }
            .map_err(|_error| PalError::new(PalErrorKind::Other))?;
        Ok(())
    }

    fn leave_raw_relay(&self) -> Result<(), PalError> {
        let saved = saved_modes()
            .lock()
            .expect("saved console modes are only copied, never held across a panic")
            .take();
        // Every restoration is attempted even when an earlier one fails, and the
        // Ctrl+C request is undone even when no modes were ever captured:
        // leaving the console half-raw is worse than losing a later error.
        let input = saved.map_or(Ok(()), |(in_mode, _out_mode)| {
            restore_mode(STD_INPUT_HANDLE, in_mode)
        });
        let output = saved.map_or(Ok(()), |(_in_mode, out_mode)| {
            restore_mode(STD_OUTPUT_HANDLE, out_mode)
        });
        // SAFETY: Add=FALSE undoes the ignore-Ctrl+C request from attach.
        let ctrl_c = unsafe { SetConsoleCtrlHandler(None, false) }
            .map_err(|_error| PalError::new(PalErrorKind::Other));
        input.and(output).and(ctrl_c)
    }

    fn window_size(&self) -> Result<WindowSize, PalError> {
        read_window_size(std_handle(STD_OUTPUT_HANDLE)?)
    }

    fn read_input(&self) -> Result<ConsoleInput, PalError> {
        let handle = std_handle(STD_INPUT_HANDLE)?;
        loop {
            if let Some(size) = take_leading_resize(handle)? {
                return Ok(ConsoleInput::Resize(size));
            }
            if discard_leading_noise(handle)? {
                continue;
            }
            // SAFETY: `handle` is the console input handle; the wait returns
            // when any input record (keys or window size) is available.
            let wait = unsafe { WaitForSingleObject(handle, INFINITE) };
            if wait != WAIT_OBJECT_0 {
                return Err(PalError::new(PalErrorKind::Other));
            }
            if let Some(size) = take_leading_resize(handle)? {
                return Ok(ConsoleInput::Resize(size));
            }
            if discard_leading_noise(handle)? {
                continue;
            }
            let mut buf = vec![0_u8; INPUT_READ_BUF];
            let mut transferred = 0_u32;
            // SAFETY: `handle` is stdin; `buf` is exclusive for this call.
            unsafe {
                ReadFile(
                    handle,
                    Some(buf.as_mut_slice()),
                    Some(&raw mut transferred),
                    None,
                )
            }
            .map_err(|_error| PalError::new(PalErrorKind::Disconnected))?;
            if transferred == 0 {
                return Err(PalError::new(PalErrorKind::Disconnected));
            }
            buf.truncate(transferred as usize);
            return Ok(ConsoleInput::Bytes(buf));
        }
    }

    fn write_output(&self, data: &[u8]) -> Result<(), PalError> {
        let handle = std_handle(STD_OUTPUT_HANDLE)?;
        let mut remaining = data;
        while !remaining.is_empty() {
            let mut transferred = 0_u32;
            // SAFETY: `handle` is stdout; `remaining` is exclusive for this call.
            unsafe { WriteFile(handle, Some(remaining), Some(&raw mut transferred), None) }
                .map_err(|_error| PalError::new(PalErrorKind::Other))?;
            if transferred == 0 {
                return Err(PalError::new(PalErrorKind::Other));
            }
            remaining = remaining
                .get(transferred as usize..)
                .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
        }
        Ok(())
    }

    fn read_prompt_line(&self) -> Result<String, PalError> {
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(PalError::from_io)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}
