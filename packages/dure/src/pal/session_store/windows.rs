//! Win32 file operations for the session store.

use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_DISPOSITION_INFO, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileDispositionInfo,
    FileStandardInfo, GetFileInformationByHandleEx, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING, ReadFile, SetFileInformationByHandle,
};
use windows::core::PCWSTR;

/// The failure of the call that just failed, as an `io::Error` that names its kind.
///
/// Callers distinguish "the name is free" from a real fault, which a rendered Win32 message
/// cannot answer, so the code is preserved rather than the text.
fn last_error() -> io::Error {
    // SAFETY: GetLastError is called immediately after the failed API.
    let code = unsafe { GetLastError() };
    io::Error::from_raw_os_error(code.0.cast_signed())
}

/// Replace `dest` with `tmp`, committing the rename to disk before returning.
pub(crate) fn move_file_replace(tmp: &Path, dest: &Path) -> io::Result<()> {
    let src: Vec<u16> = tmp.as_os_str().encode_wide().chain([0]).collect();
    let dst: Vec<u16> = dest.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `src` and `dst` are NUL-terminated paths in the same directory.
    unsafe {
        MoveFileExW(
            PCWSTR(src.as_ptr()),
            PCWSTR(dst.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    // Carries the Win32 error as the source rather than folding its rendering
    // into a string. Ref: docs/error-handling.md.
    .map_err(io::Error::other)
}

/// A record file held open for reading and for deletion of that exact file.
///
/// The point of holding it is that a name can be reused while a caller is deciding what to do
/// with what it read. Reading and deleting through one handle addresses the file the decision
/// was made about, so a record that replaced it under the same name is never the one removed.
/// Ref: docs/implementation.md, "Session store".
pub(crate) struct RecordFile {
    handle: HANDLE,
}

impl RecordFile {
    /// Opens an existing record file, or reports `NotFound` if the name is free.
    ///
    /// Sharing stays fully permissive so holding the file open blocks nobody: concurrent
    /// readers, publishers, and deleters all proceed, and a deletion by someone else merely
    /// unlinks the name while this handle keeps addressing the file it opened.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated path. The returned handle is owned by `Self`.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_GENERIC_READ.0 | DELETE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|_error| last_error())?;
        Ok(Self { handle })
    }

    /// Reads the whole file.
    ///
    /// A record is a small JSON object written in one shot, so it is read into a buffer that
    /// holds any record this program writes and a partial read simply yields fewer bytes, which
    /// the caller already treats as content it cannot trust.
    pub(crate) fn read(&self) -> io::Result<Vec<u8>> {
        // Ample for a record of a few short strings and integers.
        let mut buf = vec![0_u8; 8192];
        let mut transferred = 0_u32;
        // SAFETY: `self.handle` is a live read handle and `buf` is exclusive here.
        unsafe {
            ReadFile(
                self.handle,
                Some(&mut buf),
                Some(&raw mut transferred),
                None,
            )
        }
        .map_err(|_error| last_error())?;
        buf.truncate(transferred as usize);
        Ok(buf)
    }

    /// Deletes the file this handle addresses, whatever its name now refers to.
    ///
    /// A file somebody else already unlinked counts as deleted: the outcome asked for has
    /// happened, and the file is distinguishable from one this process may not delete, so the
    /// two are not conflated.
    pub(crate) fn delete(&self) -> io::Result<()> {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        let size = u32::try_from(size_of::<FILE_DISPOSITION_INFO>())
            .expect("FILE_DISPOSITION_INFO fits in u32");
        // SAFETY: `self.handle` was opened with DELETE access and `disposition` is a valid
        // FILE_DISPOSITION_INFO of the length given.
        let set = unsafe {
            SetFileInformationByHandle(
                self.handle,
                FileDispositionInfo,
                (&raw const disposition).cast(),
                size,
            )
        };
        if set.is_ok() {
            return Ok(());
        }
        let error = last_error();
        if self.is_going_away().unwrap_or(false) {
            return Ok(());
        }
        Err(error)
    }

    /// Whether the file this handle addresses is already on its way out.
    ///
    /// Windows refuses a second deletion of the same file, so this is what separates losing
    /// that race from being unable to delete the file at all.
    fn is_going_away(&self) -> io::Result<bool> {
        let mut info = FILE_STANDARD_INFO::default();
        let size =
            u32::try_from(size_of::<FILE_STANDARD_INFO>()).expect("FILE_STANDARD_INFO fits in u32");
        // SAFETY: `self.handle` is a live read handle and `info` is a valid FILE_STANDARD_INFO
        // of the length given, exclusive for this call.
        unsafe {
            GetFileInformationByHandleEx(
                self.handle,
                FileStandardInfo,
                (&raw mut info).cast(),
                size,
            )
        }
        .map_err(|_error| last_error())?;
        Ok(info.DeletePending || info.NumberOfLinks == 0)
    }
}

impl Drop for RecordFile {
    fn drop(&mut self) {
        // SAFETY: `self.handle` is the CreateFileW handle this value owns and never reuses.
        _ = unsafe { CloseHandle(self.handle) };
    }
}
