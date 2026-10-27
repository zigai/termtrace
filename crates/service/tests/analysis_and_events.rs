//! Integration tests for `OutputService`, `EventsService`, and `AnalysisService`.

mod support;

use assert_cmd::cargo::cargo_bin;
use std::{
    error::Error as StdError,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};
use support::record_session;
use tempfile::TempDir;
use termtrace_evidence::{
    event_log::{EventLog, EventLogWriter},
    session::{manifest_hash, read_manifest},
};
use termtrace_model::execution::event::Event;
use termtrace_model::terminal::observation::StatePoint;
use termtrace_service::{analysis::AnalysisService, events::EventsService, output::OutputService};

#[test]
fn test_output_service_paging_and_cursor() {
    let temp = TempDir::new().expect("tempdir");
    let session_path = temp.path().join("sample-output.termtrace");
    record_session(&session_path, "printf 'ALPHA\nBETA\nGAMMA\n'; sleep 0.05");

    // Read first page with small limit
    let page1 = OutputService::read_page(&session_path, None, 8).expect("page 1");
    assert!(!page1.bytes.is_empty());
    assert_eq!(page1.cursor.byte_offset, page1.bytes.len() as u64);
    assert_eq!(page1.cursor.session_path, session_path);

    // Read second page from cursor
    let page2 = OutputService::read_page(&session_path, Some(page1.cursor), 1024).expect("page 2");
    assert_eq!(page2.cursor.session_path, session_path);

    let all_bytes = [page1.bytes, page2.bytes].concat();
    let text = String::from_utf8_lossy(&all_bytes);
    assert!(text.contains("ALPHA") || text.contains("BETA"));
}

#[test]
fn test_events_service_paging_and_cursor() {
    let temp = TempDir::new().expect("tempdir");
    let session_path = temp.path().join("sample-events.termtrace");
    record_session(&session_path, "printf 'ALPHA\nBETA\nGAMMA\n'; sleep 0.05");

    let page1 = EventsService::read_page(&session_path, None, 2).expect("page 1");
    assert!(!page1.events.is_empty());
    assert!(page1.cursor.next_event_seq > 0);

    let page2 = EventsService::read_page(&session_path, Some(page1.cursor), 50).expect("page 2");
    assert_eq!(page2.cursor.session_path, session_path);
    let all_events = EventsService::read_events(&session_path, 100).expect("read events");
    assert!(!all_events.is_empty());
}

#[test]
fn test_analysis_service_diff_diagnose_cache() {
    let temp = TempDir::new().expect("tempdir");
    let session_path = temp.path().join("sample-analysis.termtrace");
    record_session(&session_path, "printf 'ALPHA\nBETA\nGAMMA\n'; sleep 0.05");
    // Test diff between initial and latest presented
    let (diff, _before, _after) = AnalysisService::diff(
        &session_path,
        StatePoint::StateId { state_id: 0 },
        StatePoint::LatestPresented,
    )
    .expect("diff");
    assert!(diff.changed || !diff.changed);

    // Test diagnose
    let report = AnalysisService::diagnose(&session_path, Vec::new(), 50).expect("diagnose");
    assert!(report.findings.is_empty() || !report.findings.is_empty());

    // Test cache status and prune
    let status = AnalysisService::cache_status(&session_path).expect("cache status");
    assert!(status.directory.is_dir() || status.compressed_bytes == 0);
    let prune = AnalysisService::cache_prune(&session_path).expect("cache prune");
    assert!(prune.removed_entries <= 1);
}

/// Isolated canonical-log fixture: the real recorder supplies a complete valid manifest;
/// a separate, freshly bound event file gives deterministic chunk boundaries and live tails.
fn paging_fixture(root: &Path) -> Result<(PathBuf, EventLogWriter), Box<dyn StdError>> {
    use termtrace_evidence::{
        event_log::{EventLogWriter, SessionClock},
        session::{canonical_manifest_bytes, manifest_hash, read_manifest},
    };
    use termtrace_model::execution::event::EventLogHeader;
    let session = root.join("paging.tt");
    record_session(&session, "printf seed");

    let clock = SessionClock::now()?;
    let mut manifest = read_manifest(&session)?;
    "paging.jsonl".clone_into(&mut manifest.event_log);
    manifest.started_unix_ns = clock.started_unix_ns();
    let bytes = canonical_manifest_bytes(&manifest)?;
    fs::write(session.join("session.json"), &bytes)?;
    let header = EventLogHeader::new(
        manifest.session_id,
        manifest_hash(&bytes),
        clock.started_unix_ns(),
        80,
        24,
        720,
        432,
        "xterm-ghostty",
    );

    let writer = EventLogWriter::create(session.join("paging.jsonl"), &header, clock)?;

    return Ok((session, writer));
}

fn append_output(writer: &mut EventLogWriter, bytes: &[u8]) -> Result<(), Box<dyn StdError>> {
    writer.append(Event::Output { data: bytes.into() })?;

    return Ok(());
}

fn append_marker(writer: &mut EventLogWriter, name: &str) -> Result<(), Box<dyn StdError>> {
    writer.append(Event::Marker {
        operation_id: uuid::Uuid::now_v7(),
        label: name.into(),
    })?;

    return Ok(());
}

#[test]
fn pages_have_exact_lookahead_and_lossless_bytes_across_chunk_boundaries() {
    let temp = TempDir::new().expect("tempdir");
    let (session, mut writer) = paging_fixture(temp.path()).expect("paging fixture");
    let empty = OutputService::page(&session, None, 3).expect("empty");
    assert!(empty.bytes.as_bytes().is_empty());
    assert!(!empty.has_more);

    append_output(&mut writer, b"A\0\xff").expect("fixture append");
    append_marker(&mut writer, "between").expect("fixture append");
    append_output(&mut writer, b"BC").expect("fixture append");

    let first = OutputService::page(&session, Some(empty.cursor), 3).expect("first");
    assert_eq!(first.bytes.as_bytes(), b"A\0\xff");
    assert!(
        first.has_more,
        "must look past a full chunk and interleaved marker"
    );

    let second = OutputService::page(&session, Some(first.cursor.clone()), 2).expect("second");
    assert_eq!(second.bytes.as_bytes(), b"BC");
    assert!(!second.has_more, "exact-full last page is not has_more");

    let eof = OutputService::page(&session, Some(second.cursor.clone()), 2).expect("EOF");
    assert_eq!(eof.cursor, second.cursor);
    assert!(eof.bytes.as_bytes().is_empty());
    assert!(!eof.has_more);

    let retry = OutputService::page(&session, Some(first.cursor), 2).expect("same cursor retry");
    assert_eq!(retry, second);
    let legacy = OutputService::read_page(
        &session,
        Some(termtrace_service::OutputCursor {
            session_path: session.clone(),
            byte_offset: 5,
            event_seq: None,
        }),
        2,
    )
    .expect("legacy EOF");
    assert!(legacy.bytes.is_empty());
    assert_eq!(legacy.cursor.byte_offset, 5);
}

#[test]
fn projected_pages_filter_without_losing_resume_or_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let (session, mut writer) = paging_fixture(temp.path()).expect("paging fixture");
    append_output(&mut writer, b"raw\0\xff").expect("fixture append");
    append_marker(&mut writer, "one").expect("fixture append");
    append_output(&mut writer, b"tail").expect("fixture append");
    append_marker(&mut writer, "two").expect("fixture append");

    let filter = vec!["marker".into()];
    let first = EventsService::page(&session, None, 1, true, &filter).expect("filtered first");
    assert!(first.has_more);
    assert_eq!(first.events[0].seq, 1);
    assert_eq!(
        first.cursor.next_event_seq, 3,
        "scan excluded output but retain next match"
    );
    assert!(first.events[0].event.is_none());

    let second = EventsService::page(&session, Some(first.cursor), 1, true, &filter)
        .expect("filtered second");
    assert_eq!(second.events[0].seq, 3);
    assert!(!second.has_more);
    assert_eq!(second.cursor.next_event_seq, 4);

    let bytes = EventsService::page(&session, None, 1, true, &[]).expect("metadata output");
    assert_eq!(bytes.events[0].byte_payloads[0].byte_count, 5);
    assert_eq!(bytes.events[0].prev_hash, bytes.cursor.source.header_hash);
    assert_eq!(
        bytes.events[0].byte_payloads[0].blake3,
        manifest_hash(b"raw\0\xff")
    );

    let none =
        EventsService::page(&session, None, 5, false, &["application".into()]).expect("no matches");
    assert!(none.events.is_empty());
    assert!(!none.has_more);
    assert_eq!(none.cursor.next_event_seq, 4);
}

#[test]
fn bound_pages_reject_forged_foreign_replaced_and_out_of_range_cursors() {
    let temp = TempDir::new().expect("tempdir");
    let (session, mut writer) = paging_fixture(temp.path()).expect("paging fixture");
    append_output(&mut writer, b"abcdef").expect("fixture append");

    let page = OutputService::page(&session, None, 2).expect("page");
    let mut forged = page.cursor.clone();
    forged.events.event_hash = "0".repeat(64);

    assert!(OutputService::page(&session, Some(forged), 2).is_err());

    let mut forged = page.cursor.clone();
    forged.byte_offset = 7;

    assert!(OutputService::page(&session, Some(forged), 2).is_err());

    let mut forged = page.cursor.clone();
    forged.events.source.session_path = b"/wrong".as_slice().into();

    assert!(OutputService::page(&session, Some(forged), 2).is_err());

    let mut forged = page.cursor.clone();
    forged.events.next_event_seq = 9;

    assert!(OutputService::page(&session, Some(forged), 2).is_err());
    for limit in [0, 4 * 1024 * 1024 + 1] {
        assert!(OutputService::page(&session, None, limit).is_err());
    }

    for limit in [0, 4097] {
        assert!(EventsService::page(&session, None, limit, false, &[]).is_err());
    }

    assert!(EventsService::page(&session, None, 2, false, &["invented".into()]).is_err());
    assert!(EventsService::read_page(
        &session,
        Some(termtrace_service::EventCursor {
            session_path: temp.path().into(),
            next_event_seq: 0
        }),
        2
    )
    .is_err());

    let path = session.join("paging.jsonl");
    let original = fs::read(&path).expect("log bytes");
    drop(writer);

    fs::rename(&path, session.join("old.jsonl")).expect("replace old inode");
    fs::write(&path, original).expect("same bytes new inode");

    assert!(OutputService::page(&session, Some(page.cursor), 2).is_err());
}

#[test]
fn live_partial_tail_is_explicit_and_complete_corruption_never_hides_after_a_full_page() {
    use std::io::Write as _;
    let temp = TempDir::new().expect("tempdir");
    let (session, mut writer) = paging_fixture(temp.path()).expect("paging fixture");
    append_output(&mut writer, b"abc").expect("fixture append");
    drop(writer);
    let path = session.join("paging.jsonl");
    let bytes = fs::read(&path).expect("log");
    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append")
        .write_all(b"{\"seq\":")
        .expect("partial tail");

    let page = EventsService::page(&session, None, 1, false, &[]).expect("live prefix");
    assert!(page.incomplete_tail);
    assert!(!page.has_more);

    let output = OutputService::page(&session, None, 3).expect("live bytes");
    assert!(output.incomplete_tail);
    assert_eq!(output.bytes.as_bytes(), b"abc");

    fs::write(&path, [bytes.as_slice(), b"{}\n"].concat()).expect("corrupt complete tail");

    assert!(EventsService::page(&session, None, 1, false, &[]).is_err());
    assert!(OutputService::page(&session, None, 3).is_err());
}

#[test]
fn public_client_pages_resume_across_live_completion_without_omission() {
    use std::time::Duration;
    use termtrace_service::{LaunchSpec, Session, StopRequest, Terminal};
    let temp = TempDir::new().expect("tempdir");
    let binary = cargo_bin("termtrace");
    let terminal = Terminal::new(binary, temp.path());
    let mut owner: Session = terminal
        .start(LaunchSpec::new([
            "/bin/sh",
            "-c",
            "printf 'READY'; read line",
        ]))
        .expect("launch");

    owner
        .output()
        .wait_for(b"READY", Duration::from_secs(5))
        .expect("ready");
    let first = owner.output().page(None, 2).expect("live output");
    assert_eq!(first.bytes.as_bytes(), b"RE");

    let events = owner
        .events()
        .read_page(None, 1, false, &[])
        .expect("live events");

    let mut all = events.events;
    let mut cursor = events.cursor;
    owner
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("owned stop");
    let rest = owner
        .output()
        .page(Some(first.cursor), 32)
        .expect("completed output");
    assert_eq!(
        [first.bytes.as_bytes(), rest.bytes.as_bytes()].concat(),
        b"READY"
    );

    loop {
        let page = owner
            .events()
            .read_page(Some(cursor), 1, false, &[])
            .expect("completed events");
        all.extend(page.events);
        cursor = page.cursor;

        if !page.has_more {
            break;
        }
    }

    assert!(all.len() > 1);
    for (seq, event) in all.iter().enumerate() {
        assert_eq!(event.seq, seq as u64);
    }

    let complete = EventLog::read(session_log(owner.path()).expect("session log"))
        .expect("whole canonical log");
    assert_eq!(all.len(), complete.events.len());
    assert_eq!(
        all.last().expect("last").hash,
        complete.events.last().expect("canonical last").hash
    );
}

fn session_log(path: &Path) -> Result<PathBuf, Box<dyn StdError>> {
    return Ok(path.join(read_manifest(path)?.event_log));
}
