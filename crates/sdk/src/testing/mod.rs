//! Runner-neutral helpers for ordinary Rust tests.

use std::{
    error::Error as StdError,
    ffi::OsString,
    fmt,
    fs::{File, OpenOptions},
    io::{self, BufRead as _, BufReader, Read as _, Write as _},
    mem::replace,
    net::Shutdown,
    os::unix::{fs::OpenOptionsExt as _, net::UnixStream},
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
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
    protocol::control::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest, ControlResponse, ControlResult,
        MAX_CONTROL_MESSAGE_BYTES,
    },
    terminal::observation::StatePoint,
    terminal::screen::{ScreenFrame, ScreenSnapshotOptions},
    terminal::state::CursorState,
};

use crate::{
    Error, Result,
    assertions::{
        AssertionError, AssertionFrameIdentity, ExpectationMismatch, SnapshotProfile,
        assert_cursor, assert_screen_text, normalized_snapshot,
    },
    client::{Screen, Session, Terminal},
    locator::{Locator, LocatorSpec},
    protocol::connect_session_control,
};

const MAX_SECONDARY_FAILURE_CHARS: usize = 512;

mod evidence;
mod lifecycle;
mod snapshot;

pub use lifecycle::{
    EvidencePolicy, FinalizationReport, SecondaryFailure, TerminalTest, TerminalTestConfig,
    TestFailure, TestOutcome, run_terminal_test,
};
pub use snapshot::{TerminalAssertionError, assert_snapshot_file};
