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
use termtrace_evidence::bundle::verify_archive;

struct BundleHarness {
    _temporary: TempDir,
    archive: PathBuf,
}

impl BundleHarness {
    fn new() -> Self {
        let temporary = TempDir::new().expect("create bundle fuzz directory");
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
            .expect("secure bundle fuzz directory");
        let archive = temporary.path().join("candidate.tar.zst");
        return Self {
            _temporary: temporary,
            archive,
        };
    }

    fn exercise(&mut self, bytes: &[u8]) {
        let mut archive = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&self.archive)
            .expect("open bundle fuzz input");
        archive.write_all(bytes).expect("write bundle fuzz input");
        drop(archive);
        let _verification = verify_archive(&self.archive, None);
    }
}

thread_local! {
    static HARNESS: RefCell<BundleHarness> = RefCell::new(BundleHarness::new());
}

fuzz_target!(|bytes: &[u8]| {
    HARNESS.with(|harness| harness.borrow_mut().exercise(bytes));
});
