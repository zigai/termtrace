use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::de::DeserializeOwned;
use termtrace_model::artifact::visual::{CaptureKind, CaptureOutcome};

use super::{Error, Result};

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
pub(super) fn status_error_with_logs(program: &str, status: ExitStatus, staging: &Path) -> Error {
    let stderr =
        fs::read_to_string(staging.join(format!("{program}.stderr.log"))).unwrap_or_default();
    return Error::ExternalCommand {
        operation: program.to_owned(),
        reason: format!("exited with {status}: {}", stderr.trim()),
    };
}
pub(super) fn command_output(program: &str, args: &[&str]) -> Result<Vec<u8>> {
    return command_output_in_profile(program, args, None);
}
pub(super) fn command_output_in_profile(
    program: &str,
    args: &[&str],
    profile: Option<&Path>,
) -> Result<Vec<u8>> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(root) = profile {
        command.env("XDG_CONFIG_HOME", root);
    }
    let output = command
        .output()
        .map_err(|error| return external_error(program, error))?;
    if !output.status.success() {
        return Err(Error::ExternalCommand {
            operation: program.to_owned(),
            reason: format!(
                "exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    return Ok(output.stdout);
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
