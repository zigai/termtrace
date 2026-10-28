#![allow(dead_code, reason = "each test target uses a different subset")]

use std::{
    env, fs,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_sdk::{
    events::Event,
    input::KeyChord,
    integrity::SessionCompletion,
    screen::{frame_text, ScreenPredicate},
    session::LaunchSpec,
    FrameDetail, ScreenProjectionRequest, Session, StopOutcome, StopRequest, Terminal,
};

pub const TIMEOUT: Duration = Duration::from_secs(10);

pub const ECHO: &str =
    "printf READY; while IFS= read -r line; do printf '\\r\\nGOT:%s' \"$line\"; done";

pub fn binary() -> PathBuf {
    return PathBuf::from(env::var_os("TERMTRACE_BIN").expect("TERMTRACE_BIN names the CLI"));
}

pub fn scratch(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch directory");

    return root;
}

pub fn terminal(name: &str) -> Terminal {
    return Terminal::new(binary(), scratch(name).join("evidence"));
}

pub fn shell(script: &str) -> Vec<String> {
    return vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()];
}

pub fn enter() -> KeyChord {
    return KeyChord::parse("enter")
        .expect("enter parses")
        .expect("enter is a key");
}

pub fn wait_text(session: &Session, text: &str) {
    let outcome = session
        .wait_screen(
            ScreenPredicate::Contains {
                text: text.to_owned(),
            },
            Duration::ZERO,
            TIMEOUT,
        )
        .expect("screen wait");
    assert!(
        outcome.is_satisfied(),
        "{text:?} never appeared: {:?}",
        screen_text(session)
    );
}

pub fn screen_text(session: &Session) -> String {
    let frame = session
        .frame(ScreenProjectionRequest {
            detail: FrameDetail::Text,
            region: None,
        })
        .expect("text frame");

    return frame_text(&frame);
}

pub fn complete(session: &mut Session) -> SessionCompletion {
    return session.complete(TIMEOUT).expect("session completes");
}

pub fn events(session: &Session) -> Vec<Event> {
    let mut events = Vec::new();
    let mut cursor = None;
    loop {
        let page = session
            .events()
            .read_page(cursor, 4096, false, &[])
            .expect("event page");
        events.extend(
            page.events
                .into_iter()
                .filter_map(|record| return record.event),
        );
        cursor = Some(page.cursor);
        if !page.has_more {
            return events;
        }
    }
}

pub fn stop(session: &mut Session) -> SessionCompletion {
    let outcome = session
        .stop(StopRequest::default(), Some(TIMEOUT))
        .expect("stop");
    let StopOutcome::Completed { completion } = outcome else {
        panic!("stop did not complete: {outcome:?}");
    };

    return *completion;
}

pub struct Live(Session);

impl Deref for Live {
    type Target = Session;

    fn deref(&self) -> &Session {
        return &self.0;
    }
}

impl DerefMut for Live {
    fn deref_mut(&mut self) -> &mut Session {
        return &mut self.0;
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.0.force_close();
    }
}

pub trait Launch {
    fn launch(&self, spec: LaunchSpec) -> termtrace_sdk::Result<Live>;
}

impl Launch for Terminal {
    fn launch(&self, spec: LaunchSpec) -> termtrace_sdk::Result<Live> {
        return self.start(spec).map(Live);
    }
}
