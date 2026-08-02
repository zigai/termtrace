use std::{
    fs::Permissions,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use super::{CaptureKind, CaptureOutcome};
use crate::{Error, Result};

pub(super) fn secure_permissions(path: &Path) -> Result<()> {
    return fs_err::set_permissions(path, Permissions::from_mode(0o600)).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    });
}

pub(super) fn command_output(program: &str, arguments: &[&str]) -> Result<Vec<u8>> {
    return command_output_in_profile(program, arguments, None);
}

pub(super) fn command_output_in_profile(
    program: &str,
    arguments: &[&str],
    profile_root: Option<&Path>,
) -> Result<Vec<u8>> {
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(profile_root) = profile_root {
        command.env("XDG_CONFIG_HOME", profile_root);
    }
    let output = command
        .output()
        .map_err(|error| return external_error(program, &error))?;
    if output.status.success() {
        return Ok(output.stdout);
    } else {
        return Err(status_error(program, output.status));
    }
}

pub(super) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    return serde_json::from_slice(&bytes)
        .map_err(|source| return json_error(path.to_path_buf(), source));
}

pub(super) fn unix_now_ns() -> Result<u128> {
    return SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| return duration.as_nanos())
        .map_err(|_| return Error::ClockBeforeEpoch);
}

pub(super) fn outcome(directory: PathBuf, kind: &CaptureKind) -> CaptureOutcome {
    return CaptureOutcome {
        video: matches!(kind, CaptureKind::Video).then(|| return directory.join("session.mkv")),
        screenshot: directory.join("final.png"),
        directory,
    };
}

pub(super) fn status_error(program: &str, status: ExitStatus) -> Error {
    return Error::ExternalCommand {
        program: program.to_owned(),
        reason: format!("exit status {status}"),
    };
}

pub(super) fn status_error_with_logs(program: &str, status: ExitStatus, staging: &Path) -> Error {
    return Error::ExternalCommand {
        program: program.to_owned(),
        reason: format!(
            "exit status {status}; inspect logs in {}",
            staging.display()
        ),
    };
}

pub(super) fn external_error(program: &str, error: &impl ToString) -> Error {
    return Error::ExternalCommand {
        program: program.to_owned(),
        reason: error.to_string(),
    };
}

pub(super) fn invalid_probe(reason: &str) -> Error {
    return Error::ExternalCommand {
        program: "ffprobe".to_owned(),
        reason: reason.to_owned(),
    };
}

pub(super) fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    };
}

pub(super) fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Ok(())) => return Err(primary),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}
