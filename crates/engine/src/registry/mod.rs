//! Secure owner-only named-session registry and managed-session retention.

use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions, Permissions},
    io::{ErrorKind, Read as _},
    os::linux::net::SocketAddrExt as _,
    os::unix::{
        ffi::OsStrExt as _,
        fs::{FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
        net::{SocketAddr, UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use nix::{
    fcntl::{Flock, FlockArg},
    libc::O_NOFOLLOW,
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::getuid,
};
use termtrace_evidence::{
    artifact::write_secure,
    event_log::EventLog,
    session::{read_completion, read_manifest, validate_completed_log},
};
use termtrace_model::{
    execution::session::{SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION},
    protocol::control::CONTROL_PROTOCOL_VERSION,
    registry::{
        AliasReservation, DetachedSessionPaths, GarbageCollectionCandidate,
        GarbageCollectionReport, REGISTRY_ENTRY_FORMAT, REGISTRY_ENTRY_VERSION, RegistryEntry,
        RegistryLifecycle, RegistryListEntry, RegistrySessionResolution,
    },
    value::ByteString,
};
use uuid::Uuid;

use crate::{Error, Result};

const REGISTRY_DIRECTORY: &str = "registry";
const REGISTRY_RESERVATION_FORMAT: &str = "termtrace-session-registry-reservation";
const REGISTRY_RESERVATION_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
enum RegistryRecord {
    Entry(RegistryEntry),
    Reservation(AliasReservation),
}

/// Secure storage boundary for named session aliases.
#[derive(Clone, Debug)]
pub struct SessionRegistry {
    state_root: PathBuf,
    directory: PathBuf,
}

mod liveness;
mod reservation;
mod retention;
mod store;

use reservation::{system_time_unix_ns, valid_reservation, validate_alias};
use store::{
    ensure_owner_directory, read_registry_record_nofollow, registry_error, sync_directory,
    sync_parent_directory, validate_owner_directory,
};

#[cfg(test)]
mod tests;
