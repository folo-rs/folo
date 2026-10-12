//! Supervisor tests, grouped by the concern each one is about.

mod attach;
mod panics;
mod preamble;
mod sequences;
mod session;
mod startup;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex, mpsc};
use std::thread;

use testing::{WatchdogPhaseReporter, with_watchdog_phases};

use super::*;
use crate::constants::{CONNECT_TIMEOUT, MAX_CLIENT_BACKLOG_BYTES, MAX_OUTPUT_CHUNK_BYTES};
use crate::durability::LauncherTie;
use crate::outbox::Outbox;
use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{AppId, ConnId, JobId, ListenerId};
use crate::pal::processes::MockProcesses;
use crate::pal::pseudoconsole::{MemoryPseudoconsole, Pseudoconsole, WindowSize};
use crate::pal::session_store::{MemorySessionStore, SessionStore};
use crate::pal::transport::{MemoryTransport, Transport};
use crate::protocol::{Message, PROTOCOL_VERSION, StartupStep, encode, payload_len_ok};
use crate::session_record::{ProcessIdentity, SessionRecord};
use crate::supervisor::record_writer::RecordWriter;
use crate::supervisor::relay::{client_loop, pty_output_loop};
use crate::supervisor::shared::{Client, FirstAttach, Shared, preamble_messages};
use crate::supervisor::startup::{DEFAULT_PTY_SIZE, InitGuard, initialize, map_startup};
use crate::{
    AppCommand, BreakawayDeniedError, PalFailedError, SessionId, StartupFailedError, StoreError,
};

mod fixtures;

use fixtures::*;
