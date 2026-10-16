use std::{
    env,
    ffi::OsStr,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

/// Every executable regular file named `program` in `PATH`, in search order.
#[must_use]
pub fn executables_on_path(program: impl AsRef<OsStr>) -> Vec<PathBuf> {
    let program = program.as_ref();
    let Some(path) = env::var_os("PATH") else {
        return Vec::new();
    };

    return env::split_paths(&path)
        .map(|directory| return directory.join(program))
        .filter(|candidate| return is_executable(candidate))
        .collect();
}

/// The executable a shell would run for `program`, resolved through `PATH`.
#[must_use]
pub fn find_executable(program: impl AsRef<OsStr>) -> Option<PathBuf> {
    let program = program.as_ref();
    let path = env::var_os("PATH")?;

    return env::split_paths(&path)
        .map(|directory| return directory.join(program))
        .find(|candidate| return is_executable(candidate));
}

/// Whether `candidate` is a regular file with an execute permission bit.
#[must_use]
pub fn is_executable(candidate: &Path) -> bool {
    return fs::metadata(candidate).is_ok_and(|metadata| {
        return metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
    });
}
