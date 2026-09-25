use super::{invalid_replay, json_error};
use crate::completion;
use crate::{registry::SessionRegistry, Error, Result};
use nix::{libc::O_NOFOLLOW, unistd::geteuid};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use termtrace_evidence::{
    artifact::{secure_file, sync_directory},
    event_log::canonical_source_fingerprint,
    session::{read_manifest, validate_completed_session_with_check},
};
use termtrace_model::execution::{layout::SessionLayout, session::SessionCompletion};

/// Owner-only key authenticating the local verification cache, kept outside session evidence.
const VERIFICATION_CACHE_KEY: &str = "verification-cache.key";

pub(crate) fn check_cancelled(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|flag| return flag.load(Ordering::Relaxed)) {
        return Err(invalid_replay("semantic source query cancelled"));
    }
    return Ok(());
}

/// A completed session whose canonical validation succeeded for its current source identity.
///
/// This is the only way to establish completed-session truth. Local trust records are minted
/// only from a validated session, so a record whose identity still matches the source carries
/// proof that canonical validation succeeded for exactly those bytes.
pub struct ValidatedSession {
    root: PathBuf,
    completion: SessionCompletion,
    source: SourceIdentity,
}

impl ValidatedSession {
    /// Validate a completed session and pin the source identity the validation covered.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is incomplete, invalid, or changes during validation.
    pub fn open(session: &Path) -> Result<Self> {
        return Self::open_cancellable(session, None);
    }

    pub(crate) fn open_cancellable(session: &Path, cancelled: Option<&AtomicBool>) -> Result<Self> {
        if completion::published(session)?.is_none() {
            return Err(Error::SessionIncomplete {
                session: session.to_path_buf(),
            });
        }

        let source = SourceIdentity::capture(session)?;
        let completion = validate_completed_session_with_check(session, || {
            return check_cancelled(cancelled).map_err(|error| {
                return termtrace_evidence::Error::InvalidEvidence {
                    reason: error.to_string(),
                };
            });
        })?;

        source.check(session)?;

        return Ok(Self {
            root: session.to_path_buf(),
            completion,
            source,
        });
    }

    /// Adopt a completion the recorder validated against `candidate` and then published.
    ///
    /// # Errors
    ///
    /// Returns an error when the source changed during validation or publication.
    pub(crate) fn published(
        session: &Path,
        candidate: SourceIdentity,
        completion: SessionCompletion,
    ) -> Result<Self> {
        if candidate != SourceIdentity::capture_candidate(session)? {
            return Err(invalid_replay(
                "canonical source changed during completion validation",
            ));
        }

        let source = SourceIdentity::capture(session)?;
        if !source.0.starts_with(&candidate.0) {
            return Err(invalid_replay(
                "canonical source changed while publishing completion",
            ));
        }
        return Ok(Self {
            root: session.to_path_buf(),
            completion,
            source,
        });
    }

    /// The validated session directory.
    pub fn root(&self) -> &Path {
        return &self.root;
    }

    /// The validated completion record.
    pub const fn completion(&self) -> &SessionCompletion {
        return &self.completion;
    }

    /// Confirm the source is still the one that was validated.
    ///
    /// # Errors
    ///
    /// Returns an error when any canonical source file changed since validation.
    pub fn check(&self) -> Result<()> {
        return self.source.check(&self.root);
    }
}

/// Local-kernel identities are meaningful only under the local verifier's secret MAC.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SourceIdentity(Vec<FileIdentity>, String);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl SourceIdentity {
    pub(crate) fn capture(session: &Path) -> Result<Self> {
        let mut source = Self::capture_candidate(session)?;
        source
            .0
            .push(file_identity(&SessionLayout::new(session).completion())?);

        return Ok(source);
    }

    pub(crate) fn capture_candidate(session: &Path) -> Result<Self> {
        let layout = SessionLayout::new(session);
        let mut paths = vec![layout.event_log(), layout.manifest()];

        if read_manifest(session)?.ghostty_visual.is_some() {
            paths.push(layout.ghostty_replay_profile());
        }

        return Ok(Self(
            paths
                .iter()
                .map(|path| return file_identity(path))
                .collect::<Result<_>>()?,
            canonical_source_fingerprint(layout.event_log()).map_err(|source| {
                return Error::Filesystem {
                    path: layout.event_log(),
                    source,
                };
            })?,
        ));
    }

    pub(crate) fn check(&self, session: &Path) -> Result<()> {
        if *self != Self::capture(session)? {
            return Err(invalid_replay(
                "canonical source changed during verification or replay",
            ));
        }
        return Ok(());
    }
}

fn file_identity(path: &Path) -> Result<FileIdentity> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    let metadata = file.metadata().map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    if !metadata.is_file() {
        return Err(invalid_replay(
            "canonical cache source must be a regular file",
        ));
    }
    return Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        bytes: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    });
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Authenticated<T> {
    source: SourceIdentity,
    payload: T,
    mac: String,
}

/// Mint a local trust record; only a validated session can vouch for a payload.
pub(crate) fn encode<T: Serialize>(validated: &ValidatedSession, payload: T) -> Result<Vec<u8>> {
    validated.check()?;

    let source = &validated.source;
    let key = verification_key(&validated.root)?;
    let body = serde_json::to_vec(&(source, &payload))
        .map_err(|error| return json_error(PathBuf::from("<verified-cache>"), error))?;
    let mac = blake3::keyed_hash(&key, &body).to_hex().to_string();

    return serde_json::to_vec(&Authenticated {
        source: source.clone(),
        payload,
        mac,
    })
    .map_err(|error| return json_error(PathBuf::from("<verified-cache>"), error));
}

/// Accept a local trust record only when it was minted for the session's current source identity.
pub(crate) fn decode<T: Serialize + DeserializeOwned>(session: &Path, bytes: &[u8]) -> Result<T> {
    let authenticated: Authenticated<T> = serde_json::from_slice(bytes)
        .map_err(|error| return json_error(PathBuf::from("<verified-cache>"), error))?;

    let key = verification_key(session)?;
    let body = serde_json::to_vec(&(&authenticated.source, &authenticated.payload))
        .map_err(|error| return json_error(PathBuf::from("<verified-cache>"), error))?;
    let expected = blake3::keyed_hash(&key, &body);
    let supplied = blake3::Hash::from_hex(&authenticated.mac)
        .map_err(|_| return invalid_replay("invalid local verification-cache MAC"))?;

    if expected != supplied {
        return Err(invalid_replay(
            "local verification-cache authentication failed",
        ));
    }

    authenticated.source.check(session)?;

    return Ok(authenticated.payload);
}

fn verification_key(session: &Path) -> Result<[u8; 32]> {
    let registry = SessionRegistry::open_default()?;
    let root = registry.state_root();
    let canonical_session = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;

    let canonical_root = root.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: root.to_path_buf(),
            source,
        };
    })?;

    if canonical_root.starts_with(&canonical_session) {
        return Err(invalid_replay(
            "local verification-cache key must be outside canonical session evidence",
        ));
    }

    let path = root.join(VERIFICATION_CACHE_KEY);

    if !path.try_exists().map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })? {
        let temporary = root.join(format!(".verification-cache-key-{}", uuid::Uuid::now_v7()));
        let mut random = [0; 32];
        File::open("/dev/urandom")
            .and_then(|mut file| return file.read_exact(&mut random))
            .map_err(|source| {
                return Error::Filesystem {
                    path: PathBuf::from("/dev/urandom"),
                    source,
                };
            })?;

        let mut file = secure_file(&temporary)?;
        let result = (|| {
            file.write_all(&random)
                .and_then(|()| return file.sync_all())
                .map_err(|source| {
                    return Error::Filesystem {
                        path: temporary.clone(),
                        source,
                    };
                })?;

            match fs::hard_link(&temporary, &path) {
                Ok(()) => sync_directory(root)?,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(Error::Filesystem {
                        path: path.clone(),
                        source,
                    });
                }
            }

            return Ok(());
        })();

        fs::remove_file(&temporary).map_err(|source| {
            return Error::Filesystem {
                path: temporary,
                source,
            };
        })?;

        result?;
    }

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(&path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
    let before = file.metadata().map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;

    if !before.is_file()
        || before.mode() & 0o777 != 0o600
        || before.uid() != geteuid().as_raw()
        || before.len() != 32
    {
        return Err(invalid_replay(
            "local verification-cache key must be an owner-owned 0600 regular 32-byte file",
        ));
    }

    let mut key = [0; 32];
    file.read_exact(&mut key).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let after = file
        .metadata()
        .map_err(|source| return Error::Filesystem { path, source })?;

    if before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
        || after.len() != 32
    {
        return Err(invalid_replay(
            "local verification-cache key changed while reading",
        ));
    }
    return Ok(key);
}
