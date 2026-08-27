//! Windows process, job, and spawn implementation.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Write;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use rand::RngExt;
use windows::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE, WAIT_OBJECT_0,
};
use windows::Win32::System::Console::HPCON;
use windows::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DETACHED_PROCESS,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetCurrentProcessId, GetExitCodeProcess, GetProcessId, GetProcessTimes, INFINITE,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcess,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{AppId, JobId};
use crate::pal::processes::{
    AppSpawn, ProcessLiveness, Processes, SupervisorSpawn, resolve_command_path,
};
use crate::pal::pseudoconsole::hpcon_for;
use crate::pal::raw_handle::RawHandle;
use crate::session_record::ProcessIdentity;

/// Real Windows process control.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetProcesses;

struct HandleTable {
    jobs: HashMap<u64, RawHandle>,
    apps: HashMap<u64, RawHandle>,
}

fn table() -> &'static Mutex<HandleTable> {
    static TABLE: OnceLock<Mutex<HandleTable>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(HandleTable {
            jobs: HashMap::new(),
            apps: HashMap::new(),
        })
    })
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn filetime_u64(time: FILETIME) -> u64 {
    let high = u64::from(time.dwHighDateTime);
    let low = u64::from(time.dwLowDateTime);
    high.checked_shl(32)
        .expect("shifting a u32 into the high half of a u64 cannot overflow")
        | low
}

fn identity_of(handle: HANDLE) -> Result<ProcessIdentity, PalError> {
    let pid = {
        // SAFETY: `handle` is a valid process handle owned by the caller.
        unsafe { GetProcessId(handle) }
    };
    if pid == 0 {
        return Err(PalError::new(PalErrorKind::InspectFailed));
    }
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all FILETIME pointers refer to stack values that outlive the call.
    unsafe {
        GetProcessTimes(
            handle,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .map_err(|_error| PalError::new(PalErrorKind::InspectFailed))?;
    Ok(ProcessIdentity {
        pid,
        creation_time: filetime_u64(creation),
    })
}

fn open_verified(identity: &ProcessIdentity) -> Result<HANDLE, PalError> {
    let desired = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE;
    // SAFETY: pid is a numeric process id; OpenProcess does not retain aliasing
    // of Rust references.
    let handle = unsafe { OpenProcess(desired, false, identity.pid) }
        .map_err(|_error| PalError::new(PalErrorKind::NotFound))?;
    match identity_of(handle) {
        Ok(actual) if actual.creation_time == identity.creation_time => Ok(handle),
        Ok(_) => {
            close(handle);
            Err(PalError::new(PalErrorKind::NotFound))
        }
        Err(error) => {
            close(handle);
            Err(error)
        }
    }
}

fn close(handle: HANDLE) {
    // SAFETY: `handle` is a process or job handle we own and never use again.
    _ = unsafe { CloseHandle(handle) };
}

fn wide(s: &str) -> Vec<u16> {
    OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl Processes for BuildTargetProcesses {
    fn current_exe(&self) -> Result<PathBuf, PalError> {
        std::env::current_exe().map_err(PalError::from_io)
    }

    fn spawn_supervisor(&self, request: &SupervisorSpawn) -> Result<ProcessIdentity, PalError> {
        let mut cmd = format!("\"{}\"", request.exe.display());
        for arg in &request.args {
            cmd.push(' ');
            cmd.push('"');
            cmd.push_str(arg);
            cmd.push('"');
        }
        let mut cmd_wide = wide(&cmd);
        let mut exe_wide = wide(&request.exe.to_string_lossy());
        let si = STARTUPINFOW {
            cb: u32::try_from(size_of::<STARTUPINFOW>()).expect("STARTUPINFOW fits in u32"),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let flags = CREATE_BREAKAWAY_FROM_JOB | DETACHED_PROCESS | CREATE_UNICODE_ENVIRONMENT;
        // SAFETY: `exe_wide` and `cmd_wide` are NUL-terminated. `si`/`pi` are
        // valid stack structures. No handle inherit list is passed because the
        // startup channel is a named pipe connected by name after spawn.
        let created = unsafe {
            CreateProcessW(
                PCWSTR(exe_wide.as_mut_ptr()),
                Some(PWSTR(cmd_wide.as_mut_ptr())),
                None,
                None,
                false,
                flags,
                None,
                None,
                &raw const si,
                &raw mut pi,
            )
        };
        created.map_err(|_error| PalError::new(PalErrorKind::BreakawayDenied))?;
        close(pi.hThread);
        let identity = identity_of(pi.hProcess)?;
        close(pi.hProcess);
        Ok(identity)
    }

    fn probe(&self, identity: &ProcessIdentity) -> ProcessLiveness {
        let handle = match open_verified(identity) {
            Ok(handle) => handle,
            Err(error) if error.kind() == PalErrorKind::NotFound => return ProcessLiveness::Dead,
            Err(_) => return ProcessLiveness::InspectFailed,
        };
        let mut code = 0_u32;
        // SAFETY: `handle` is a process handle we opened and `code` is a stack u32.
        let result = unsafe { GetExitCodeProcess(handle, &raw mut code) };
        close(handle);
        match result {
            Ok(()) if code == STILL_ACTIVE.0 as u32 => ProcessLiveness::Live,
            Ok(()) => ProcessLiveness::Dead,
            Err(_) => ProcessLiveness::InspectFailed,
        }
    }

    fn terminate(&self, identity: &ProcessIdentity) -> Result<(), PalError> {
        let handle = open_verified(identity).map_err(|error| {
            if error.kind() == PalErrorKind::NotFound {
                PalError::new(PalErrorKind::NotFound)
            } else {
                PalError::new(PalErrorKind::InspectFailed)
            }
        })?;
        // SAFETY: `handle` is a process handle opened with PROCESS_TERMINATE
        // and verified to be the recorded process.
        let result = unsafe { TerminateProcess(handle, 1) };
        close(handle);
        result.map_err(|_error| PalError::new(PalErrorKind::Other))
    }

    fn create_lifetime_job(&self) -> Result<JobId, PalError> {
        // SAFETY: a null name creates an unnamed job object.
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|_error| PalError::new(PalErrorKind::Other))?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        // SAFETY: `info` is a stack structure of the size SetInformationJobObject
        // expects for JobObjectExtendedLimitInformation.
        unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::from_mut(&mut info).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .expect("job info size fits in u32"),
            )
        }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;
        let id = next_id();
        table()
            .lock()
            .expect("handle table")
            .jobs
            .insert(id, RawHandle::from_handle(handle));
        Ok(JobId(id))
    }

    fn close_job(&self, job: JobId) {
        if let Some(handle) = table().lock().expect("handle table").jobs.remove(&job.0) {
            close(handle.as_handle());
        }
    }

    fn spawn_app(&self, request: &AppSpawn) -> Result<AppId, PalError> {
        let hpcon = hpcon_for(request.pty).ok_or_else(|| PalError::new(PalErrorKind::NotFound))?;
        let exe = resolve_command_path(
            request
                .command
                .first()
                .ok_or_else(|| PalError::new(PalErrorKind::Other))?,
            &request.launch_directory,
        );
        let mut cmd = String::new();
        for (index, arg) in request.command.iter().enumerate() {
            if index > 0 {
                cmd.push(' ');
            }
            cmd.push('"');
            cmd.push_str(arg);
            cmd.push('"');
        }
        let mut cmd_wide = wide(&cmd);
        let mut exe_wide = wide(&exe.to_string_lossy());
        let mut dir_wide = wide(&request.launch_directory.to_string_lossy());

        let job = table()
            .lock()
            .expect("handle table")
            .jobs
            .get(&request.job.0)
            .copied()
            .ok_or_else(|| PalError::new(PalErrorKind::NotFound))?
            .as_handle();

        // Two attributes: the pseudoconsole and the lifetime job. The job is
        // applied at creation so the app cannot run outside it. CREATE_SUSPENDED
        // plus a later AssignProcessToJobObject delays console initialization
        // and can leave the child with pipes instead of a console.
        let attribute_count = 2;
        let mut attr_size: usize = 0;
        // SAFETY: querying the required size; the null list pointer is allowed
        // when discovering the buffer length.
        _ = unsafe {
            InitializeProcThreadAttributeList(None, attribute_count, None, &raw mut attr_size)
        };
        let mut attr_buf = vec![0_u8; attr_size];
        let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr().cast());
        // SAFETY: `attr_buf` is the size reported by the previous call and lives
        // for the rest of this function.
        unsafe {
            InitializeProcThreadAttributeList(
                Some(attr_list),
                attribute_count,
                None,
                &raw mut attr_size,
            )
        }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;

        // PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE stores `lpValue` as the HPCON
        // itself. The Microsoft sample passes `hPC`, not `&hPC`.
        // Ref: https://learn.microsoft.com/windows/console/creating-a-pseudoconsole-session
        let hpcon_value = hpcon.0 as *const core::ffi::c_void;
        // SAFETY: the attribute list was initialized for two attributes. `hpcon`
        // is a live HPCON owned by the pseudoconsole PAL for `request.pty`.
        unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                usize::try_from(PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE)
                    .expect("PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE fits in usize"),
                Some(hpcon_value),
                size_of::<HPCON>(),
                None,
                None,
            )
        }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;

        let mut job_list = [job];
        // SAFETY: the list still has a free slot. `job_list` holds the lifetime
        // job handle and lives until CreateProcessW returns.
        unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                usize::try_from(PROC_THREAD_ATTRIBUTE_JOB_LIST)
                    .expect("PROC_THREAD_ATTRIBUTE_JOB_LIST fits in usize"),
                Some(std::ptr::from_mut(&mut job_list).cast()),
                size_of::<HANDLE>(),
                None,
                None,
            )
        }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;

        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb =
            u32::try_from(size_of::<STARTUPINFOEXW>()).expect("STARTUPINFOEXW fits in u32");
        // Parent stdio is often redirected (SSH, `cargo test`). CreateProcess
        // copies those handle values into the child unless STARTF_USESTDHANDLES
        // overrides them. Invalid std handles let the pseudoconsole attribute
        // install console handles instead of the parent's pipes.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        si.lpAttributeList = attr_list;
        let mut pi = PROCESS_INFORMATION::default();
        let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT;
        // SAFETY: wide strings are NUL-terminated. Attribute list is initialized
        // with the pseudoconsole and job. Std handles are invalid so they are
        // not the parent's redirected pipes.
        let created = unsafe {
            CreateProcessW(
                PCWSTR(exe_wide.as_mut_ptr()),
                Some(PWSTR(cmd_wide.as_mut_ptr())),
                None,
                None,
                false,
                flags,
                None,
                PCWSTR(dir_wide.as_mut_ptr()),
                std::ptr::from_ref(&si.StartupInfo),
                &raw mut pi,
            )
        };
        // SAFETY: the list was initialized above and is no longer referenced by
        // the kernel after CreateProcessW returns.
        unsafe {
            DeleteProcThreadAttributeList(attr_list);
        }
        created.map_err(|_error| PalError::new(PalErrorKind::Other))?;

        close(pi.hThread);
        let id = next_id();
        table()
            .lock()
            .expect("handle table")
            .apps
            .insert(id, RawHandle::from_handle(pi.hProcess));
        Ok(AppId(id))
    }

    fn wait_app(&self, app: AppId) -> Result<i32, PalError> {
        let handle = table()
            .lock()
            .expect("handle table")
            .apps
            .get(&app.0)
            .copied()
            .ok_or_else(|| PalError::new(PalErrorKind::NotFound))?
            .as_handle();
        // SAFETY: `handle` is a process handle stored in the table.
        let wait = unsafe { WaitForSingleObject(handle, INFINITE) };
        if wait != WAIT_OBJECT_0 {
            return Err(PalError::new(PalErrorKind::Other));
        }
        let mut code = 0_u32;
        // SAFETY: the process has exited; `code` is a stack u32.
        unsafe { GetExitCodeProcess(handle, &raw mut code) }
            .map_err(|_error| PalError::new(PalErrorKind::Other))?;
        // Windows process statuses are `u32`; NTSTATUS-style failure codes use
        // the high bit and do not fit in a non-negative `i32`.
        Ok(code.cast_signed())
    }

    fn current_identity(&self) -> Result<ProcessIdentity, PalError> {
        // SAFETY: GetCurrentProcess returns a pseudo-handle that does not need
        // to be closed and is valid for the calling process.
        let handle = unsafe { GetCurrentProcess() };
        let mut identity = identity_of(handle)?;
        identity.pid = {
            // SAFETY: no preconditions.
            unsafe { GetCurrentProcessId() }
        };
        Ok(identity)
    }

    fn random_nonce(&self) -> String {
        let bytes: [u8; 16] = rand::rng().random();
        let mut nonce = String::with_capacity(32);
        for byte in bytes {
            write!(nonce, "{byte:02x}").expect("writing to String cannot fail");
        }
        nonce
    }
}
