use super::super::validation::OWNER_DIRECTORY_MODE;
use crate::event_log::SessionClock;
use std::{fs::Permissions, os::unix::fs::PermissionsExt as _, path::Path};
use termtrace_model::execution::event::EventLogHeader;
use uuid::Uuid;

pub(super) fn header(clock: &SessionClock) -> EventLogHeader {
    return EventLogHeader::new(
        Uuid::now_v7(),
        "0000000000000000000000000000000000000000000000000000000000000000",
        clock.started_unix_ns(),
        120,
        40,
        1080,
        720,
        "xterm-ghostty",
    );
}

pub(super) fn secure_directory(path: &Path) {
    fs_err::create_dir(path).expect("create secure directory");
    fs_err::set_permissions(path, Permissions::from_mode(OWNER_DIRECTORY_MODE))
        .expect("secure directory permissions");
}
