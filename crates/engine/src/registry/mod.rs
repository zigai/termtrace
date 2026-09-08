//! Secure owner-only named-session registry and managed-session retention.

use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions, Permissions},
    io::ErrorKind,
    os::linux::net::SocketAddrExt as _,
    os::unix::{
        ffi::OsStrExt as _,
        fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
        net::{SocketAddr, UnixStream},
    },
    path::{Path, PathBuf},
    result::Result as StdResult,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::de::{self, Deserialize, Deserializer};

use nix::{
    fcntl::{Flock, FlockArg},
    libc::O_NOFOLLOW,
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::getuid,
};
use termtrace_evidence::{
    artifact::write_secure,
    session::{read_completion, read_manifest},
};
use termtrace_model::{
    execution::session::{SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION},
    protocol::control::CONTROL_PROTOCOL_VERSION,
    registry::{
        AliasReservation, DetachedSessionPaths, GarbageCollectionCandidate,
        GarbageCollectionReport, RegistryEntry, RegistryLifecycle, RegistryListEntry,
        RegistrySessionResolution, REGISTRY_ENTRY_FORMAT, REGISTRY_ENTRY_VERSION,
    },
    value::ByteString,
};
use uuid::Uuid;

use crate::{Error, Result};

const REGISTRY_DIRECTORY: &str = "registry";
const REGISTRY_RESERVATION_FORMAT: &str = "termtrace-session-registry-reservation";
const REGISTRY_RESERVATION_VERSION: u32 = 1;

pub use liveness::recorder_identity;

#[derive(Clone, Debug, Eq, PartialEq)]
enum RegistryRecord {
    Reservation(AliasReservation),
    Entry(RegistryEntry),
}

impl<'de> Deserialize<'de> for RegistryRecord {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("owner_token").is_some() {
            return serde_json::from_value(value)
                .map(Self::Reservation)
                .map_err(de::Error::custom);
        }

        return serde_json::from_value(value)
            .map(Self::Entry)
            .map_err(de::Error::custom);
    }
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
