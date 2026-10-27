//! Shared immutable assertion and safe snapshot comparison vectors.
#[cfg(test)]
mod tests {
    use nix::{
        fcntl::{Flock, FlockArg},
        sys::stat::Mode,
        unistd::mkfifo,
    };
    use std::{fs, os::unix::fs::symlink, path::Path};
    use termtrace_model::terminal::screen::{
        ScreenFrame, ScreenFrameContent, ScreenRegion, ScreenSnapshotOptions,
    };
    use termtrace_service::{
        assertions::{
            assert_count, assert_cursor, assert_exists, assert_match_style, assert_match_text,
            assert_neutral_region, assert_no_presented_transient, assert_no_wrapped_rows,
            assert_not_exists, assert_screen_text, masked_snapshot, normalized_snapshot,
            AssertionError, ExpectationMismatchError, MaskedSnapshotOptions, SnapshotProfile,
        },
        locator::{LocatorSpec, ScreenFrameQuery, StyleFilter},
        testing::{assert_masked_snapshot_file, assert_snapshot_file, TerminalAssertionError},
    };

    fn fixture() -> (ScreenFrame, serde_json::Value) {
        let frame = serde_json::from_str(include_str!(
            "../../../fixtures/conformance/locators/parity-frame.json"
        ))
        .expect("frame");

        let corpus = serde_json::from_str(include_str!(
            "../../../fixtures/conformance/testing/assertions.json"
        ))
        .expect("corpus");

        return (frame, corpus);
    }

    fn region(top: u16, left: u16, right: u16, bottom: u16) -> ScreenRegion {
        return ScreenRegion {
            top,
            left,
            right,
            bottom,
        };
    }

    fn mismatch(corpus: &serde_json::Value, name: &str, error: ExpectationMismatchError) {
        assert_eq!(
            serde_json::to_value(error).expect("diagnostic"),
            *corpus
                .get("mismatches")
                .and_then(|v| return v.get(name))
                .expect("expected mismatch")
        );
    }

    fn only_mismatch(error: AssertionError) -> ExpectationMismatchError {
        let AssertionError::Mismatch(error) = error else {
            panic!("unexpected operational error: {error}");
        };

        return error;
    }

    #[test]
    fn shared_assertion_corpus() {
        let (mut frame, corpus) = fixture();
        for (profile, name) in [
            (SnapshotProfile::Text, "text"),
            (SnapshotProfile::Spans, "spans"),
            (SnapshotProfile::Cells, "cells"),
        ] {
            assert_eq!(
                normalized_snapshot(
                    &frame,
                    profile,
                    ScreenSnapshotOptions {
                        include_cursor: true,
                        include_modes: true
                    }
                )
                .expect("normalize"),
                corpus
                    .get("normalized")
                    .and_then(|v| return v.get(name))
                    .and_then(serde_json::Value::as_str)
                    .expect("normalized vector")
            );
        }

        let options: MaskedSnapshotOptions =
            serde_json::from_value(corpus.get("mask_options").cloned().expect("mask options"))
                .expect("mask options");
        assert_eq!(
            masked_snapshot(&frame, &options).expect("masked"),
            corpus
                .get("masked")
                .and_then(serde_json::Value::as_str)
                .expect("masked vector")
        );

        let found = frame
            .query(LocatorSpec::text("Save").expect("spec"))
            .expect("query");
        let none = frame
            .query(LocatorSpec::text("ABSENT").expect("spec"))
            .expect("query");

        let selected = found.first().expect("first");
        assert_screen_text(&frame, "Save").expect("text");

        assert_exists(&found).expect("exists");

        assert_not_exists(&none).expect("absent");

        assert_count(&found, 2).expect("count");

        assert_match_text(selected, "Save").expect("match");
        assert_match_style(selected, &StyleFilter::default()).expect("style");

        assert_cursor(&frame, &frame.cursor).expect("unknown cursor facts preserved");
        assert_no_presented_transient(&frame).expect("presented");
        mismatch(
            &corpus,
            "screen_text",
            only_mismatch(assert_screen_text(&frame, "ABSENT").expect_err("mismatch")),
        );

        mismatch(
            &corpus,
            "exists",
            assert_exists(&none).expect_err("mismatch"),
        );

        mismatch(
            &corpus,
            "not_exists",
            assert_not_exists(&found).expect_err("mismatch"),
        );

        mismatch(
            &corpus,
            "count",
            assert_count(&found, 1).expect_err("mismatch"),
        );

        mismatch(
            &corpus,
            "match_text",
            assert_match_text(selected, "OTHER").expect_err("mismatch"),
        );

        mismatch(
            &corpus,
            "match_style",
            only_mismatch(
                assert_match_style(
                    selected,
                    &StyleFilter {
                        bold: Some(true),
                        ..StyleFilter::default()
                    },
                )
                .expect_err("mismatch"),
            ),
        );

        let mut cursor = frame.cursor.clone();
        cursor.row = Some(0);
        mismatch(
            &corpus,
            "cursor",
            assert_cursor(&frame, &cursor).expect_err("mismatch"),
        );

        mismatch(
            &corpus,
            "no_wrapped_rows",
            only_mismatch(assert_no_wrapped_rows(&frame).expect_err("mismatch")),
        );

        mismatch(
            &corpus,
            "neutral_region",
            only_mismatch(assert_neutral_region(&frame, region(0, 0, 1, 1)).expect_err("mismatch")),
        );

        frame.presented = false;
        mismatch(
            &corpus,
            "no_presented_transient",
            assert_no_presented_transient(&frame).expect_err("mismatch"),
        );

        frame.presented = true;
        frame.modes.synchronized_output = true;

        assert!(assert_no_presented_transient(&frame).is_err());

        frame.data = ScreenFrameContent::None;

        assert!(matches!(
            assert_screen_text(&frame, "Save"),
            Err(AssertionError::Query(_))
        ));
        assert!(matches!(
            assert_no_wrapped_rows(&frame),
            Err(AssertionError::Query(_))
        ));
        assert!(matches!(
            assert_neutral_region(&frame, region(0, 0, 1, 1)),
            Err(AssertionError::Query(_))
        ));
    }

    #[test]
    fn masked_union_and_wide_boundaries_are_exact_and_immutable() {
        let (frame, corpus) = fixture();
        let original = serde_json::to_string(&frame).expect("frame");
        let mut options: MaskedSnapshotOptions =
            serde_json::from_value(corpus.get("mask_options").cloned().expect("mask"))
                .expect("options");

        let expected = masked_snapshot(&frame, &options).expect("masked");
        options.mask_regions = vec![region(1, 2, 6, 2), region(1, 0, 4, 2), region(1, 0, 6, 2)];

        assert_eq!(masked_snapshot(&frame, &options).expect("union"), expected);

        let mut changed = frame.clone();
        if let ScreenFrameContent::Spans { rows } = &mut changed.data {
            let row = rows.get_mut(1).expect("row");
            row.text = "IGNORED".to_owned();
            let cell = row
                .spans
                .first_mut()
                .expect("span")
                .cells
                .first_mut()
                .expect("cell");
            cell.grapheme = "語".to_owned();
        }

        assert_eq!(
            masked_snapshot(&changed, &options).expect("masked change"),
            expected
        );

        if let ScreenFrameContent::Spans { rows } = &mut changed.data {
            rows.first_mut()
                .expect("row")
                .spans
                .first_mut()
                .expect("span")
                .cells
                .first_mut()
                .expect("cell")
                .grapheme = "X".to_owned();
        }

        assert_ne!(
            masked_snapshot(&changed, &options).expect("adjacent change"),
            expected
        );

        for invalid in [
            region(1, 0, 1, 2),
            region(3, 15, 16, 4),
            region(0, 0, 0, 1),
            region(0, 0, 17, 1),
        ] {
            options.mask_regions = vec![invalid];

            assert!(matches!(
                masked_snapshot(&frame, &options),
                Err(AssertionError::Query(_))
            ));
        }

        options.mask_regions = vec![region(3, 15, 16, 4), region(4, 0, 2, 5)];
        masked_snapshot(&frame, &options).expect("whole wrapped wide grapheme");
        options.profile = SnapshotProfile::Text;

        assert!(masked_snapshot(&frame, &options).is_err());
        assert_eq!(serde_json::to_string(&frame).expect("frame"), original);
    }

    #[test]
    fn snapshots_are_explicit_bounded_atomic_and_nofollow() {
        let (frame, corpus) = fixture();
        let temp = tempfile::tempdir().expect("temporary");
        let path = temp.path().join("nested/baseline.json");
        let compare = |path: &Path, update| {
            return assert_snapshot_file(
                &frame,
                path,
                SnapshotProfile::Text,
                ScreenSnapshotOptions::default(),
                update,
            );
        };

        assert!(matches!(
            compare(&path, false),
            Err(TerminalAssertionError::Assertion(AssertionError::Mismatch(
                _
            )))
        ));

        compare(&path, true).expect("explicit update");
        compare(&path, false).expect("match");

        let expected = format!(
            "{}\n",
            normalized_snapshot(
                &frame,
                SnapshotProfile::Text,
                ScreenSnapshotOptions::default()
            )
            .expect("normalize")
        );

        assert_eq!(fs::read_to_string(&path).expect("baseline"), expected);

        let link = temp.path().join("link");
        symlink(&path, &link).expect("symlink");
        for update in [false, true] {
            assert!(matches!(
                compare(&link, update),
                Err(TerminalAssertionError::Operation(_))
            ));
        }

        let root_link = temp.path().join("linked-root");
        symlink(path.parent().expect("parent"), &root_link).expect("symlink parent");

        assert!(compare(&root_link.join("new.json"), true).is_err());
        assert!(compare(temp.path(), true).is_err());

        let notdir = temp.path().join("file-root");
        fs::write(&notdir, "x").expect("file");

        assert!(compare(&notdir.join("x"), true).is_err());
        assert!(compare(&temp.path().join("nested/../escape"), true).is_err());

        let fifo = temp.path().join("fifo");
        mkfifo(&fifo, Mode::S_IRUSR).expect("fifo");
        for update in [false, true] {
            assert!(compare(&fifo, update).is_err());
        }

        let mut huge = frame.clone();
        if let ScreenFrameContent::Spans { rows } = &mut huge.data {
            rows.first_mut().expect("row").text = "x".repeat(4 * 1024 * 1024 + 1);
        }

        let untouched = temp.path().join("not-created/snapshot.json");
        assert!(assert_snapshot_file(
            &huge,
            &untouched,
            SnapshotProfile::Text,
            ScreenSnapshotOptions::default(),
            true
        )
        .is_err());
        assert!(!untouched.parent().expect("parent").exists());

        let options: MaskedSnapshotOptions =
            serde_json::from_value(corpus.get("mask_options").cloned().expect("options"))
                .expect("options");

        let masked = temp.path().join("masked.json");
        assert_masked_snapshot_file(&frame, &masked, &options, true).expect("masked publish");
        assert_masked_snapshot_file(&frame, &masked, &options, false).expect("masked compare");
        let mut bad = options;
        bad.mask_regions = vec![region(1, 0, 1, 2)];

        assert!(assert_masked_snapshot_file(&frame, &path, &bad, true).is_err());
        assert_eq!(fs::read_to_string(&path).expect("unchanged"), expected);

        fs::write(&path, vec![b'x'; 4 * 1024 * 1024 + 1]).expect("large");

        assert!(matches!(
            compare(&path, false),
            Err(TerminalAssertionError::Operation(_))
        ));

        fs::write(&path, "stale").expect("stale");

        assert!(matches!(
            compare(&path, false),
            Err(TerminalAssertionError::Assertion(AssertionError::Mismatch(
                _
            )))
        ));

        // Held directory inode lock rejects competing publishers without altering the existing file.
        let lock = Flock::lock(
            fs::File::open(path.parent().expect("parent")).expect("directory"),
            FlockArg::LockExclusiveNonblock,
        )
        .expect("lock");

        assert!(compare(&path, true).is_err());
        assert_eq!(fs::read_to_string(&path).expect("unchanged"), "stale");

        drop(lock);

        assert!(!fs::read_dir(path.parent().expect("parent"))
            .expect("files")
            .any(|e| return e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".snapshot-")));
    }
}
