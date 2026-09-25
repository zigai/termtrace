use super::{Error, Result};
use serde::de::DeserializeOwned;
use std::{
    fs,
    io::Read as _,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use termtrace_model::artifact::visual::{CaptureKind, CaptureOutcome};

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    return serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    });
}

pub(super) fn external_error(program: &str, error: impl ToString) -> Error {
    return Error::ExternalCommand {
        operation: program.to_owned(),
        reason: error.to_string(),
    };
}

pub(super) fn check_cancelled(cancelled: &AtomicBool, operation: &str) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(external_error(operation, "capture cancelled"));
    }
    return Ok(());
}

pub(super) fn status_error_with_logs(program: &str, status: ExitStatus, staging: &Path) -> Error {
    let mut stderr = String::new();
    let _ = fs::File::open(staging.join(format!("{program}.stderr.log")))
        .and_then(|file| return file.take(2048).read_to_string(&mut stderr));

    return Error::ExternalCommand {
        operation: program.to_owned(),
        reason: format!("exited with {status}: {}", stderr.trim()),
    };
}

pub(super) fn command_output(program: &'static str, args: &[&str]) -> Result<Vec<u8>> {
    return command_output_in_profile(program, args, None);
}

pub(super) fn command_output_in_profile(
    program: &'static str,
    args: &[&str],
    profile: Option<&Path>,
) -> Result<Vec<u8>> {
    let mut command = Command::new(program);
    command.args(args);

    if let Some(root) = profile {
        command.env("XDG_CONFIG_HOME", root);
    }

    return super::process::probe_output(program, &mut command, Duration::from_secs(15));
}

pub(super) fn secure_permissions(path: &Path) -> Result<()> {
    let mut permissions = fs::metadata(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?
        .permissions();
    permissions.set_mode(0o600);

    return fs::set_permissions(path, permissions).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    });
}

pub(super) fn unix_now_ns() -> Result<u128> {
    return SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| return duration.as_nanos())
        .map_err(|_| {
            return Error::InvalidReplay {
                reason: "system clock is before Unix epoch".to_owned(),
            };
        });
}

pub(super) fn outcome(directory: PathBuf, kind: &CaptureKind) -> CaptureOutcome {
    return CaptureOutcome {
        video: matches!(kind, CaptureKind::Video).then(|| return directory.join("session.mkv")),
        screenshot: directory.join("final.png"),
        directory,
    };
}
