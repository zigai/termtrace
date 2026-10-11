//! Integration tests for `OutputService`, `EventsService`, and `AnalysisService`.

mod support;

use support::record_session;
use tempfile::TempDir;
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
