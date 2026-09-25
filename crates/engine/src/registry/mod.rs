//! Secure owner-only named-session registry and managed-session retention.

use crate::{Error, Result};
use nix::{
    fcntl::{Flock, FlockArg},
    libc::O_NOFOLLOW,
    unistd::getuid,
};
use serde::de::{self, Deserialize, Deserializer};
use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions, Permissions},
    io::ErrorKind,
    os::unix::{
        ffi::OsStrExt as _,
        fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
    result::Result as StdResult,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use termtrace_control::{observe_phase, recorder_identity};
use termtrace_evidence::{artifact::write_secure, session::read_manifest};
use termtrace_model::{
    execution::session::SessionPhase,
    execution::session::{SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION},
    protocol::control::CONTROL_PROTOCOL_VERSION,
    registry::{
        AliasReservation, DetachedSessionPaths, GarbageCollectionCandidate,
        GarbageCollectionReport, RegistryEntry, RegistryListEntry, RegistrySessionResolution,
        REGISTRY_ENTRY_FORMAT, REGISTRY_ENTRY_VERSION,
    },
    value::ByteString,
};
use uuid::Uuid;

const REGISTRY_DIRECTORY: &str = "registry";
const REGISTRY_RESERVATION_FORMAT: &str = "termtrace-session-registry-reservation";
const REGISTRY_RESERVATION_VERSION: u32 = 1;

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

mod reservation;
mod retention;
mod store;

use reservation::{system_time_unix_ns, valid_reservation, validate_alias};
use store::{
    ensure_owner_directory, inspect_entry, read_registry_record_nofollow, registry_error,
    sync_directory, sync_parent_directory, validate_owner_directory,
};

#[cfg(test)]
mod tests;
