//! Windows local console PAL.

use std::io;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Globalization::CP_UTF8;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
    ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WINDOW_INPUT, ENABLE_WRAP_AT_EOL_OUTPUT,
    GetConsoleCP, GetConsoleMode, GetConsoleOutputCP, GetConsoleScreenBufferInfo, GetStdHandle,
    INPUT_RECORD, KEY_EVENT, PeekConsoleInputW, ReadConsoleInputW, STD_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetConsoleCP, SetConsoleCtrlHandler, SetConsoleMode, SetConsoleOutputCP,
    WINDOW_BUFFER_SIZE_EVENT,
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

/// Console state the relay replaces, kept so the terminal can be handed back
/// the way it was found. Ref: docs/implementation.md, "Local console".
#[derive(Clone, Copy, Debug)]
struct SavedConsole {
    in_mode: CONSOLE_MODE,
    out_mode: CONSOLE_MODE,
    in_code_page: u32,
    out_code_page: u32,
}

fn saved_console() -> &'static Mutex<Option<SavedConsole>> {
    static SAVED: OnceLock<Mutex<Option<SavedConsole>>> = OnceLock::new();
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

/// Sets the code pages this console uses to interpret relayed bytes.
///
/// The relay carries a byte stream that a pseudoconsole produced and that a
/// pseudoconsole will consume, and those are UTF-8 in both directions. A
/// console applies its code page to the bytes crossing `WriteFile` and
/// `ReadFile`, and that code page defaults to the machine's OEM one, under
/// which every multi-byte UTF-8 sequence decodes as several unrelated glyphs.
/// Ref: docs/implementation.md, "Console encoding".
///
/// Both are attempted even when the first fails, because a console left with
/// one side converted is worse than one left wholly unconverted.
fn set_code_pages(input: u32, output: u32) -> Result<(), PalError> {
    // SAFETY: both set process-wide console state to a documented code page
    // identifier and take no pointers.
    let input = unsafe { SetConsoleCP(input) };
    // SAFETY: as above, for the output direction.
    let output = unsafe { SetConsoleOutputCP(output) };
    input
        .and(output)
        .map_err(|_error| PalError::new(PalErrorKind::Other))
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
/// Ref: docs/implementation.md, "Window size".
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
/// This is what excludes mouse reporting from pass-through.
/// Ref: docs/implementation.md, "Window size".
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
        // SAFETY: reads process-wide console state and takes no arguments.
        let in_code_page = unsafe { GetConsoleCP() };
        // SAFETY: reads process-wide console state and takes no arguments.
        let out_code_page = unsafe { GetConsoleOutputCP() };
        {
            let mut saved = saved_console()
                .lock()
                .expect("saved console state is only copied, never held across a panic");
            if saved.is_none() {
                *saved = Some(SavedConsole {
                    in_mode,
                    out_mode,
                    in_code_page,
                    out_code_page,
                });
            }
        }
        // Disable cooked input so keystrokes reach the app immediately. Enable
        // VT input for CSI sequences and window-input so resizes appear as
        // `WINDOW_BUFFER_SIZE_EVENT` records rather than being dropped.
        // Ref: docs/implementation.md, "Console modes".
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
        set_code_pages(CP_UTF8, CP_UTF8)?;
        Ok(())
    }

    fn leave_raw_relay(&self) -> Result<(), PalError> {
        let saved = saved_console()
            .lock()
            .expect("saved console state is only copied, never held across a panic")
            .take();
        // Every restoration is attempted even when an earlier one fails, and the
        // Ctrl+C request is undone even when no modes were ever captured:
        // leaving the console half-raw is worse than losing a later error.
        let input = saved.map_or(Ok(()), |saved| {
            restore_mode(STD_INPUT_HANDLE, saved.in_mode)
        });
        let output = saved.map_or(Ok(()), |saved| {
            restore_mode(STD_OUTPUT_HANDLE, saved.out_mode)
        });
        let code_pages = saved.map_or(Ok(()), |saved| {
            set_code_pages(saved.in_code_page, saved.out_code_page)
        });
        // SAFETY: Add=FALSE undoes the ignore-Ctrl+C request from attach.
        let ctrl_c = unsafe { SetConsoleCtrlHandler(None, false) }
            .map_err(|_error| PalError::new(PalErrorKind::Other));
        input.and(output).and(code_pages).and(ctrl_c)
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
