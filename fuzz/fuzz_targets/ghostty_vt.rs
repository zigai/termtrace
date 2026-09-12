#![no_main]

use libfuzzer_sys::fuzz_target;
use termtrace_terminal::GhosttyVt;

fuzz_target!(|bytes: &[u8]| {
    let mut vt = GhosttyVt::new(80, 24, 1000).expect("valid fixed terminal geometry");
    vt.write(bytes);
    let _snapshot = vt
        .snapshot()
        .expect("snapshot after arbitrary terminal output");
    let _responses = vt.take_responses();
});
