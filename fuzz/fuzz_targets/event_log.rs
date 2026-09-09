#![no_main]

use std::{
    cell::RefCell,
    fs::{self, OpenOptions, Permissions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::PathBuf,
};
use libfuzzer_sys::fuzz_target;
use tempfile::TempDir;
use termtrace_evidence::event_log::EventLog;

struct EventLogHarness {
    _temporary: TempDir,
    source: PathBuf,
    recovered: PathBuf,
}

impl EventLogHarness {
    fn new() -> Self {
        let temporary = TempDir::new().expect("create event-log fuzz directory");
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
            .expect("secure event-log fuzz directory");
        let source = temporary.path().join("events.jsonl");
        let recovered = temporary.path().join("recovered.jsonl");

        return Self {
            _temporary: temporary,
            source,
            recovered,
        };
    }

    fn exercise(&mut self, bytes: &[u8]) {
        let mut source = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&self.source)
            .expect("open event-log fuzz input");
        source.write_all(bytes).expect("write event-log fuzz input");
        drop(source);

        let _read_result = EventLog::read(&self.source);
        let _remove_result = fs::remove_file(&self.recovered);
        let recovery = EventLog::recover_prefix(&self.source, &self.recovered);
        if recovery.is_ok() {
            EventLog::read(&self.recovered).expect("recovered event log must validate");
        }
    }
}

thread_local! {
    static HARNESS: RefCell<EventLogHarness> = RefCell::new(EventLogHarness::new());
}

fuzz_target!(|bytes: &[u8]| {
    HARNESS.with(|harness| harness.borrow_mut().exercise(bytes));
});
