//! Secure owner-only named-session registry and managed-session retention.

use crate::{Error, Result};
use nix::{libc::O_NOFOLLOW, unistd::getuid};
use serde::de::{self, Deserialize, Deserializer};
use serde_json::value::RawValue;
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
    execution::session::SESSION_MANIFEST_FORMAT,
    protocol::control::CONTROL_PROTOCOL_VERSION,
    registry::{
        AliasReservation, DetachedSessionPaths, GarbageCollectionCandidate,
        GarbageCollectionReport, RegistryEntry, RegistryListEntry, RegistrySessionResolution,
        REGISTRY_ENTRY_FORMAT, REGISTRY_ENTRY_VERSION,
    },
    value::ByteString,
};
use termtrace_process::ExclusiveLock;
use uuid::Uuid;

/// Environment variable that overrides the platform user state directory.
pub const STATE_DIR_ENV: &str = "TERMTRACE_STATE_DIR";
const REGISTRY_DIRECTORY: &str = "registry";
const SESSIONS_DIRECTORY: &str = "sessions";
const SUPERVISOR_DIRECTORY: &str = "supervisor";
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
        // Buffer exact JSON tokens only at this format-dispatch boundary. Materializing a
        // Value here would round numeric u128 timestamps before the typed reader sees them.
        // raw_value does not change any canonical serializer or the global Number domain.
        let raw = Box::<RawValue>::deserialize(deserializer)?;

        #[derive(serde::Deserialize)]
        struct Identity {
            format: String,
            version: u32,
        }

        let identity: Identity = serde_json::from_str(raw.get()).map_err(de::Error::custom)?;

        match (identity.format.as_str(), identity.version) {
            (REGISTRY_RESERVATION_FORMAT, REGISTRY_RESERVATION_VERSION) => {
                return serde_json::from_str(raw.get())
                    .map(Self::Reservation)
                    .map_err(de::Error::custom);
            }
            (REGISTRY_ENTRY_FORMAT, REGISTRY_ENTRY_VERSION) => {
                return serde_json::from_str(raw.get())
                    .map(Self::Entry)
                    .map_err(de::Error::custom);
            }
            _ => {
                return Err(de::Error::custom(
                    "unsupported registry record format/version",
                ))
            }
        }
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

use reservation::{is_valid, system_time_unix_ns, validate_alias};
use store::{
    ensure_owner_directory, inspect_entry, read_registry_record_nofollow, registry_error,
    sync_directory, sync_parent_directory,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod lossless_record_tests {
    use super::*;
    use std::io::Cursor;

    const ENTRY: &str = r#"{"format":"termtrace-session-registry-entry","version":1,"alias":"sample","canonical_session":"L3RtcC//","session_id":"00000000-0000-0000-0000-000000000000","recorder_pid":null,"recorder_process_start_ticks":null,"control_protocol_version":19,"registered_unix_ns":340282366920938463463374607431768211455}"#;

    #[test]
    fn typed_reparse_keeps_u128_numeric_tokens_and_exact_canonical_hashes() {
        let RegistryRecord::Entry(entry) = serde_json::from_reader(Cursor::new(ENTRY)).unwrap()
        else {
            panic!("entry");
        };

        assert_eq!(entry.registered_unix_ns, u128::MAX);
        assert_eq!(entry.canonical_session.as_bytes(), b"/tmp/\xff");

        let encoded = serde_json::to_string(&entry).unwrap();
        assert_eq!(encoded, ENTRY);
        assert_eq!(
            blake3::hash(encoded.as_bytes()),
            blake3::hash(ENTRY.as_bytes())
        );

        // Ordinary Value serialization/hash semantics must not change with raw_value enabled.
        let value =
            serde_json::json!({"app": {"counter": 9_007_199_254_740_993_u64}, "bytes": "AP8="});

        let canonical = r#"{"app":{"counter":9007199254740993},"bytes":"AP8="}"#;

        assert_eq!(serde_json::to_string(&value).unwrap(), canonical);
        assert_eq!(
            blake3::hash(&serde_json::to_vec(&value).unwrap()),
            blake3::hash(canonical.as_bytes())
        );
    }

    #[test]
    fn current_reservations_and_legacy_optional_process_fields_remain_readable() {
        let legacy = ENTRY.replace(
            "\"recorder_pid\":null,\"recorder_process_start_ticks\":null,",
            "",
        );

        let RegistryRecord::Entry(entry) = serde_json::from_str(&legacy).unwrap() else {
            panic!("entry");
        };

        assert_eq!(entry.recorder_pid, None);
        assert_eq!(entry.recorder_process_start_ticks, None);
        assert_eq!(serde_json::to_string(&entry).unwrap(), ENTRY);

        let text = r#"{"format":"termtrace-session-registry-reservation","version":1,"alias":"sample","owner_token":"00000000-0000-0000-0000-000000000000","created_unix_ns":340282366920938463463374607431768211455,"lease_expires_unix_ns":340282366920938463463374607431768211455,"pending_session":"L3RtcC//"}"#;
        let RegistryRecord::Reservation(record) =
            serde_json::from_reader(Cursor::new(text)).unwrap()
        else {
            panic!("reservation");
        };

        assert_eq!(record.created_unix_ns, u128::MAX);
        assert_eq!(record.lease_expires_unix_ns, u128::MAX);
        assert_eq!(serde_json::to_string(&record).unwrap(), text);
    }

    #[test]
    fn rejects_unknown_versions_formats_overflow_and_duplicate_identity() {
        for text in [
            ENTRY.replace("\"version\":1", "\"version\":2"),
            ENTRY.replace("termtrace-session-registry-entry", "unknown"),
            ENTRY.replace(
                "340282366920938463463374607431768211455",
                "340282366920938463463374607431768211456",
            ),
            ENTRY.replace("\"version\":1", "\"version\":1,\"version\":1"),
        ] {
            assert!(
                serde_json::from_str::<RegistryRecord>(&text).is_err(),
                "{text}"
            );
        }
    }
}
