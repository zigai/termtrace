use super::*;
use nix::libc::ELOOP;
use std::{fs, os::unix::fs::symlink, path::PathBuf};
use tempfile::TempDir;

fn session(id: &str) -> TmuxEvent {
    return TmuxEvent::SessionCreated {
        session_id: id.to_owned(),
    };
}

fn journal(directory: &TempDir) -> PathBuf {
    return directory.path().join(JOURNAL_NAME);
}

fn append_events(
    path: &Path,
    boundary: &TmuxJournalBoundary,
    events: Vec<TmuxEvent>,
) -> TmuxJournalBoundary {
    let (_, next) = append(path, boundary, events, 7).expect("journal append");
    return next;
}

fn committed(path: &Path, boundary: &TmuxJournalBoundary) -> Result<Vec<TmuxEventRecord>> {
    let mut records = Vec::new();
    visit(path, boundary, |record| {
        records.push(record.clone());
        return Ok(());
    })?;
    return Ok(records);
}

fn record(sequence: u64, id: &str) -> TmuxEventRecord {
    return TmuxEventRecord {
        sequence,
        observed_unix_ns: 7,
        event: session(id),
    };
}

fn frame(previous_hash: &str, event: TmuxEventRecord) -> (Vec<u8>, String) {
    let hash = record_hash(previous_hash, &event).expect("record hash");
    let mut bytes = serde_json::to_vec(&Record {
        previous_hash: previous_hash.to_owned(),
        event,
        hash: hash.clone(),
    })
    .expect("record encoding");
    bytes.push(b'\n');
    return (bytes, hash);
}

fn boundary_of(bytes: &[u8], sequence: u64, hash: &str) -> TmuxJournalBoundary {
    return TmuxJournalBoundary {
        bytes: u64::try_from(bytes.len()).expect("journal length"),
        sequence,
        hash: hash.to_owned(),
        ..TmuxJournalBoundary::empty()
    };
}

fn invalid_reason<T>(result: Result<T>) -> String {
    return match result {
        Err(Error::Invalid { reason, .. }) => reason,
        Err(other) => panic!("expected an invalid-journal error, got {other:?}"),
        Ok(_) => panic!("expected an invalid-journal error, got success"),
    };
}

fn filesystem_error<T>(result: Result<T>) -> io::Error {
    return match result {
        Err(Error::Filesystem { source, .. }) => source,
        Err(other) => panic!("expected a filesystem error, got {other:?}"),
        Ok(_) => panic!("expected a filesystem error, got success"),
    };
}

fn len(path: &Path) -> u64 {
    return fs::metadata(path).expect("journal metadata").len();
}

#[test]
fn appends_continue_the_sequence_and_visit_returns_every_committed_event() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);

    let (first_written, first) =
        append(&path, &TmuxJournalBoundary::empty(), vec![session("$1")], 7).expect("append");
    let (second_written, second) =
        append(&path, &first, vec![session("$2"), session("$3")], 7).expect("append");

    assert_eq!(
        committed(&path, &second).expect("valid journal"),
        vec![record(1, "$1"), record(2, "$2"), record(3, "$3")]
    );
    assert_eq!(second.sequence, 3);
    assert_eq!(second.bytes, len(&path));
    let positions: Vec<(u64, u64)> = first_written
        .iter()
        .chain(&second_written)
        .map(|written| return (written.offset, written.offset + written.length))
        .collect();
    assert_eq!(positions.first().map(|position| return position.0), Some(0));
    assert!(positions
        .windows(2)
        .all(|pair| return pair[0].1 == pair[1].0));
    assert_eq!(
        positions.last().map(|position| return position.1),
        Some(second.bytes)
    );
}

#[test]
fn append_after_a_crash_discards_the_uncommitted_suffix_and_keeps_the_chain() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let first = append_events(&path, &TmuxJournalBoundary::empty(), vec![session("$1")]);
    let mut contents = fs::read(&path).expect("journal");
    contents.extend_from_slice(format!("{{\"previous_hash\":\"{}", "torn".repeat(256)).as_bytes());
    fs::write(&path, &contents).expect("crash suffix");

    let second = append_events(&path, &first, vec![session("$2")]);

    assert_eq!(len(&path), second.bytes);
    assert_eq!(
        committed(&path, &second).expect("valid journal"),
        vec![record(1, "$1"), record(2, "$2")]
    );
}

#[test]
fn visit_reads_only_the_committed_prefix() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let first = append_events(&path, &TmuxJournalBoundary::empty(), vec![session("$1")]);
    append_events(&path, &first, vec![session("$2")]);
    let mut contents = fs::read(&path).expect("journal");
    contents.extend_from_slice(b"not json");
    fs::write(&path, &contents).expect("uncommitted suffix");

    assert_eq!(
        committed(&path, &first).expect("valid prefix"),
        vec![record(1, "$1")]
    );
}

#[test]
fn visit_rejects_an_event_changed_after_it_was_committed() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let boundary = append_events(
        &path,
        &TmuxJournalBoundary::empty(),
        vec![session("$1"), session("$2")],
    );
    let contents = String::from_utf8(fs::read(&path).expect("journal")).expect("utf-8 journal");
    let tampered = contents.replacen("\"$1\"", "\"$9\"", 1);
    assert_ne!(tampered, contents);
    fs::write(&path, tampered).expect("tampered journal");

    assert_eq!(
        invalid_reason(committed(&path, &boundary)),
        "tmux lifecycle journal chain differs"
    );
}

#[test]
fn visit_rejects_sequences_that_skip_repeat_or_do_not_start_at_one() {
    let cases: [&[u64]; 3] = [&[2], &[1, 3], &[1, 1]];
    for sequences in cases {
        let directory = TempDir::new().expect("temporary directory");
        let path = journal(&directory);
        let mut contents = Vec::new();
        let mut hash = String::new();
        for &sequence in sequences {
            let (bytes, next) = frame(&hash, record(sequence, "$1"));
            contents.extend_from_slice(&bytes);
            hash = next;
        }
        fs::write(&path, &contents).expect("journal");
        let last = *sequences.last().expect("sequence");

        assert_eq!(
            invalid_reason(committed(&path, &boundary_of(&contents, last, &hash))),
            "tmux lifecycle journal chain differs",
            "sequences {sequences:?}"
        );
    }
}

#[test]
fn visit_rejects_a_record_that_does_not_link_to_its_predecessor() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let (first, first_hash) = frame("", record(1, "$1"));
    let second_event = record(2, "$2");
    let second_hash = record_hash(&first_hash, &second_event).expect("record hash");
    let mut second = serde_json::to_vec(&Record {
        previous_hash: "unrelated".to_owned(),
        event: second_event,
        hash: second_hash.clone(),
    })
    .expect("record encoding");
    second.push(b'\n');
    let contents = [first, second].concat();
    fs::write(&path, &contents).expect("journal");

    assert_eq!(
        invalid_reason(committed(&path, &boundary_of(&contents, 2, &second_hash))),
        "tmux lifecycle journal chain differs"
    );
}

#[test]
fn visit_rejects_a_boundary_whose_sequence_or_hash_differs_from_the_journal() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let boundary = append_events(
        &path,
        &TmuxJournalBoundary::empty(),
        vec![session("$1"), session("$2")],
    );
    let wrong_sequence = TmuxJournalBoundary {
        sequence: 3,
        ..boundary.clone()
    };
    let wrong_hash = TmuxJournalBoundary {
        hash: "0".repeat(boundary.hash.len()),
        ..boundary
    };

    for wrong in [wrong_sequence, wrong_hash] {
        assert_eq!(
            invalid_reason(committed(&path, &wrong)),
            "tmux lifecycle journal boundary differs",
            "boundary {wrong:?}"
        );
    }
}

#[test]
fn visit_rejects_a_committed_record_without_its_final_newline() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let (mut bytes, hash) = frame("", record(1, "$1"));
    bytes.pop();
    fs::write(&path, &bytes).expect("journal");

    assert_eq!(
        invalid_reason(committed(&path, &boundary_of(&bytes, 1, &hash))),
        "tmux lifecycle journal has an invalid frame"
    );
}

#[test]
fn frames_up_to_the_size_limit_round_trip_and_larger_ones_are_rejected() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let (short, _) = frame("", record(1, ""));
    let limit = usize::try_from(MAX_RECORD_BYTES).expect("frame limit");
    let largest = "x".repeat(limit - short.len());
    let (written, boundary) = append(
        &path,
        &TmuxJournalBoundary::empty(),
        vec![session(&largest)],
        7,
    )
    .expect("record at the frame limit");
    assert_eq!(boundary.bytes, MAX_RECORD_BYTES);
    assert_eq!(written.len(), 1);
    assert_eq!(
        committed(&path, &boundary).expect("valid journal"),
        vec![record(1, &largest)]
    );

    let oversized = format!("{largest}x");
    assert_eq!(
        invalid_reason(append(&path, &boundary, vec![session(&oversized)], 7)),
        "tmux lifecycle record exceeds its frame limit"
    );

    let (bytes, hash) = frame("", record(1, &oversized));
    fs::write(&path, &bytes).expect("oversized journal");
    assert_eq!(
        invalid_reason(committed(&path, &boundary_of(&bytes, 1, &hash))),
        "tmux lifecycle journal has an invalid frame"
    );
}

#[test]
fn a_journal_shorter_than_its_boundary_is_rejected_and_left_unchanged() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let boundary = append_events(&path, &TmuxJournalBoundary::empty(), vec![session("$1")]);
    let mut contents = fs::read(&path).expect("journal");
    contents.pop();
    fs::write(&path, &contents).expect("short journal");

    assert_eq!(
        invalid_reason(committed(&path, &boundary)),
        "tmux lifecycle journal committed prefix is missing"
    );
    assert_eq!(
        invalid_reason(append(&path, &boundary, vec![session("$2")], 7)),
        "tmux lifecycle journal is not a complete regular file"
    );
    assert_eq!(fs::read(&path).expect("journal"), contents);
}

#[test]
fn a_missing_journal_is_empty_only_when_nothing_was_committed() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let committed_boundary = TmuxJournalBoundary {
        bytes: 1,
        sequence: 1,
        hash: "0".to_owned(),
        ..TmuxJournalBoundary::empty()
    };

    assert_eq!(
        committed(&path, &TmuxJournalBoundary::empty()).expect("empty journal"),
        Vec::new()
    );
    assert_eq!(
        filesystem_error(committed(&path, &committed_boundary)).kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn every_entry_point_rejects_an_unsupported_journal_version() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let boundary = append_events(&path, &TmuxJournalBoundary::empty(), vec![session("$1")]);
    let contents = fs::read(&path).expect("journal");
    let future = TmuxJournalBoundary {
        version: TMUX_JOURNAL_VERSION + 1,
        ..boundary
    };
    let expected = format!(
        "unsupported tmux lifecycle journal version {}",
        TMUX_JOURNAL_VERSION + 1
    );
    let copy = directory.path().join("copy.jsonl");

    assert_eq!(invalid_reason(committed(&path, &future)), expected);
    assert_eq!(
        invalid_reason(append(&path, &future, vec![session("$2")], 7)),
        expected
    );
    assert_eq!(
        invalid_reason(copy_committed(&path, &copy, &future)),
        expected
    );
    assert_eq!(fs::read(&path).expect("journal"), contents);
    assert!(!copy.exists());
}

#[test]
fn copy_committed_copies_exactly_the_committed_prefix() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let first = append_events(&path, &TmuxJournalBoundary::empty(), vec![session("$1")]);
    append_events(&path, &first, vec![session("$2")]);
    let copy = directory.path().join("copy.jsonl");

    copy_committed(&path, &copy, &first).expect("copy");

    let source = fs::read(&path).expect("journal");
    let prefix = usize::try_from(first.bytes).expect("prefix length");
    assert_eq!(fs::read(&copy).expect("copy"), source[..prefix]);
    assert_eq!(
        committed(&copy, &first).expect("valid copy"),
        vec![record(1, "$1")]
    );
}

#[test]
fn copy_committed_never_replaces_an_existing_destination() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let boundary = append_events(&path, &TmuxJournalBoundary::empty(), vec![session("$1")]);
    let copy = directory.path().join("copy.jsonl");
    fs::write(&copy, "existing").expect("existing destination");

    assert_eq!(
        filesystem_error(copy_committed(&path, &copy, &boundary)).kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&copy).expect("destination"), b"existing");
}

#[test]
fn copy_committed_refuses_a_tampered_source_without_creating_the_copy() {
    let directory = TempDir::new().expect("temporary directory");
    let path = journal(&directory);
    let boundary = append_events(&path, &TmuxJournalBoundary::empty(), vec![session("$1")]);
    let contents = String::from_utf8(fs::read(&path).expect("journal")).expect("utf-8 journal");
    fs::write(&path, contents.replacen("\"$1\"", "\"$9\"", 1)).expect("tampered journal");
    let copy = directory.path().join("copy.jsonl");

    assert_eq!(
        invalid_reason(copy_committed(&path, &copy, &boundary)),
        "tmux lifecycle journal chain differs"
    );
    assert!(!copy.exists());
}

#[test]
fn every_entry_point_refuses_a_symlinked_journal() {
    let directory = TempDir::new().expect("temporary directory");
    let target = directory.path().join("target.jsonl");
    let boundary = append_events(&target, &TmuxJournalBoundary::empty(), vec![session("$1")]);
    let contents = fs::read(&target).expect("journal");
    let link = journal(&directory);
    symlink(&target, &link).expect("journal symlink");
    let copy = directory.path().join("copy.jsonl");

    let refusals = [
        filesystem_error(committed(&link, &boundary)),
        filesystem_error(append(&link, &boundary, vec![session("$2")], 7)),
        filesystem_error(copy_committed(&link, &copy, &boundary)),
    ];

    for refusal in refusals {
        assert_eq!(refusal.raw_os_error(), Some(ELOOP), "{refusal}");
    }
    assert_eq!(fs::read(&target).expect("journal"), contents);
    assert!(!copy.exists());
}
