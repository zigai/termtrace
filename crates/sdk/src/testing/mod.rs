//! Runner-neutral helpers for ordinary Rust tests.

use std::{
    error::Error as StdError,
    ffi::OsString,
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read as _, Write as _},
    mem::replace,
    os::unix::fs::OpenOptionsExt as _,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    result::Result as StdResult,
    thread::panicking,
    time::{Duration, Instant},
};

use nix::libc::{O_CLOEXEC, O_NOFOLLOW};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use termtrace_evidence::artifact::{create_private_directory, sync_directory, write_secure};
use termtrace_model::{
    artifact::bundle::BundleProfile,
    execution::session::VerificationLevel,
    protocol::control::{ControlCommand, ControlRequest, ControlResult},
    terminal::observation::StatePoint,
    terminal::screen::{ScreenFrame, ScreenSnapshotOptions},
    terminal::state::CursorState,
};

use crate::{
    assertions::{
        assert_cursor, assert_screen_text, normalized_snapshot, AssertionError,
        AssertionFrameIdentity, ExpectationMismatch, SnapshotProfile,
    },
    client::{Screen, Session, Terminal},
    locator::{Locator, LocatorSpec},
    protocol::{exchange, ExchangeTiming},
    Error, Result,
};

const MAX_SECONDARY_FAILURE_CHARS: usize = 512;

mod evidence;
mod lifecycle;
mod snapshot;

pub use lifecycle::{
    run_terminal_test, EvidencePolicy, FinalizationReport, SecondaryFailure, TerminalTest,
    TerminalTestConfig, TestFailure, TestOutcome,
};
pub use snapshot::{assert_snapshot_file, TerminalAssertionError};
