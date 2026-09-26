//! artifacts and bundles CLI integration tests.

use crate::support::*;
use resvg::tiny_skia::{IntRect, Pixmap};

#[test]
fn save_atomically_publishes_one_state_with_manifested_files() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("save-session");
    let destination = temporary.path().join("saved-state");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf 'A\\033[1mA\\033[22;3mA\\033[1mA\\033[0m AgÉgj ─│█ 界\\r\\n\\033[58;2;255;0;255m\\033[4:1m1 \\033[4:2m2 \\033[4:3m3 \\033[4:4m4 \\033[4:5m5\\033[0m \\033[9mstrike\\033[0m \\033[53moverline\\033[0m'; printf '\\033[?25l'",
        ])
        .assert()
        .success();

    let mut save = Command::cargo_bin("termtrace").expect("binary should build");
    save.args(["artifact", "save"])
        .arg(&session)
        .args(["--out"])
        .arg(&destination)
        .args(["--format", "txt,json,deep-json,vt,html,svg,png"])
        .assert()
        .success()
        .stdout(predicate::str::contains("save-manifest.json"));

    assert_eq!(
        fs_err::metadata(&destination)
            .expect("saved directory metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let manifest: serde_json::Value = serde_json::from_slice(
        &fs_err::read(destination.join("save-manifest.json")).expect("save manifest"),
    )
    .expect("manifest JSON");
    assert_eq!(manifest["format"], "termtrace-save");
    assert_eq!(
        manifest["sensitivity"],
        "terminal_screen_may_contain_secrets"
    );
    assert_eq!(manifest["files"].as_array().map(Vec::len), Some(7));

    let manifested_files = manifest["files"].as_array().expect("manifested files");
    assert_eq!(
        manifested_files
            .iter()
            .find(|file| return file["path"] == "screen.svg")
            .map(|file| return &file["renderer"]),
        Some(&serde_json::Value::String(
            "termtrace-semantic-svg-v4".to_owned()
        ))
    );
    assert_eq!(
        manifested_files
            .iter()
            .find(|file| return file["path"] == "screen.png")
            .map(|file| return &file["renderer"]),
        Some(&serde_json::Value::String(
            "termtrace-semantic-png-resvg-v4".to_owned()
        ))
    );
    assert!(fs_err::read_to_string(destination.join("screen.txt"))
        .expect("screen text")
        .contains("AAAA"));
    assert!(fs_err::read(destination.join("screen.vt"))
        .expect("saved VT")
        .contains(&0x1b));
    assert!(fs_err::read_to_string(destination.join("screen.html"))
        .expect("saved HTML")
        .contains(">A<"));

    let svg = fs_err::read_to_string(destination.join("screen.svg")).expect("saved SVG");
    let document = roxmltree::Document::parse(&svg).expect("valid saved SVG");
    assert_eq!(
        document.root_element().attribute("data-truth-layer"),
        Some("semantic_rendering")
    );

    let png = fs_err::read(destination.join("screen.png")).expect("saved PNG");
    let image = Pixmap::decode_png(&png).expect("decode saved PNG");
    assert_eq!((image.width(), image.height()), (1920, 1080),);

    let cell = |column: i32| {
        return image
            .clone_rect(IntRect::from_xywh(column * 12, 0, 12, 24).expect("cell rectangle"))
            .expect("fixture cell inside exported image")
            .data()
            .to_vec();
    };
    let styles = (0..4).map(cell).collect::<Vec<_>>();

    for (index, style) in styles.iter().enumerate() {
        assert_ne!(style, &cell(6), "style {index} must render an A, not blank");

        for other in styles.iter().skip(index + 1) {
            assert_ne!(
                style, other,
                "bold and italic faces must remain visually distinct"
            );
        }
    }

    assert_ne!(
        cell(4),
        cell(6),
        "first fallback symbol must have visible ink"
    );
    assert_ne!(
        cell(5),
        cell(6),
        "second fallback symbol must have visible ink"
    );
    assert_ne!(
        cell(4),
        cell(5),
        "fallback symbols must not collapse to one missing-glyph box"
    );

    let image = &image;
    let underline_masks = (0..5)
        .map(|style| {
            return (19..24)
                .flat_map(|y| {
                    return (0..12).map(move |x| {
                        let pixel = image
                            .pixel(style * 24 + x, 24 + y)
                            .expect("underline pixel inside exported image");

                        return pixel.red().min(pixel.blue()).saturating_sub(pixel.green());
                    });
                })
                .collect::<Vec<_>>();
        })
        .collect::<Vec<_>>();

    for (index, mask) in underline_masks.iter().enumerate() {
        assert!(
            mask.iter().any(|ink| return *ink > 100),
            "underline {index} must be magenta"
        );

        for other in underline_masks.iter().skip(index + 1) {
            assert_ne!(
                mask, other,
                "underline styles must remain visually distinct"
            );
        }
    }

    for (start, end, y) in [(120, 192, 38), (204, 300, 25)] {
        assert!(
            (start..end).all(|x| {
                let pixel = image
                    .pixel(x, y)
                    .expect("decoration pixel inside exported image");
                return pixel.red() > 50 && pixel.green() > 50 && pixel.blue() > 50;
            }),
            "strike and overline must span their decorated text"
        );
    }

    for file in manifest["files"].as_array().expect("manifested files") {
        let path = destination.join(file["path"].as_str().expect("relative path"));
        let bytes = fs_err::read(path).expect("saved member");
        assert_eq!(file["blake3"], blake3::hash(&bytes).to_hex().to_string());
    }

    let screenshot = temporary.path().join("semantic-screenshot");
    let mut screenshot_command = Command::cargo_bin("termtrace").expect("binary should build");
    screenshot_command
        .args(["screenshot"])
        .arg(&session)
        .args(["--out"])
        .arg(&screenshot)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""truth_layer":"semantic_rendering""#,
        ));

    assert!(screenshot.join("screen.png").is_file());

    let final_state = inspect_session(&session, StateSelector::Final)
        .expect("final screenshot state")
        .record
        .state_id;

    let batch = temporary.path().join("semantic-screenshot-batch");
    let mut batch_command = Command::cargo_bin("termtrace").expect("binary should build");
    batch_command
        .args(["screenshot"])
        .arg(&session)
        .args(["--out"])
        .arg(&batch)
        .args(["--state", "0", "--state", &final_state.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("semantic_screenshot_batch"));

    let batch_manifest: serde_json::Value = serde_json::from_slice(
        &fs_err::read(batch.join("batch-manifest.json")).expect("batch manifest"),
    )
    .expect("batch manifest JSON");
    assert_eq!(
        batch_manifest["entries"][0]["manifest"]["semantic_image_resources"],
        manifest["semantic_image_resources"]
    );
    assert_eq!(batch_manifest["entries"].as_array().map(Vec::len), Some(2));
    assert!(batch.join("state-0000/screen.png").is_file());
    assert!(batch.join("state-0001/screen.png").is_file());

    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .args(["artifact", "save"])
        .arg(&session)
        .args(["--out"])
        .arg(&destination)
        .args(["--format", "txt"])
        .assert()
        .failure();

    assert!(destination.join("semantic-state.json").is_file());
}

#[test]
fn screenshots_serialize_non_utf8_output_paths_without_panicking() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("non-utf8-screenshot-session");

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf SCREENSHOT"])
        .assert()
        .success();

    let filename = OsString::from_vec(b"screenshot-\xff.png".to_vec());
    let destination = temporary.path().join(filename);
    let output = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["screenshot"])
        .arg(&session)
        .arg("--out")
        .arg(&destination)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let response: serde_json::Value =
        serde_json::from_slice(&output).expect("non-UTF-8 screenshot response JSON");
    assert_eq!(response["kind"], "semantic_screenshot");
    assert_eq!(
        response["directory_display"],
        destination.display().to_string()
    );
    assert!(destination.join("screen.png").is_file());
}

#[test]
fn state_hash_selectors_reach_every_exact_state_workflow() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("hash-selector-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf HASH-SELECTOR"])
        .assert()
        .success();

    let initial = inspect_session(&session, StateSelector::StateId(0)).expect("initial state");
    let final_state = inspect_session(&session, StateSelector::Final).expect("final state");
    let final_hash = final_state.record.state_hash;
    let mut inspect = Command::cargo_bin("termtrace").expect("binary should build");
    inspect
        .args(["analyze", "inspect"])
        .arg("--session")
        .arg(&session)
        .args(["--state-hash", &final_hash, "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&final_hash));

    let mut show = Command::cargo_bin("termtrace").expect("binary should build");
    show.arg("screen")
        .arg(&session)
        .args(["--state-hash", &final_hash, "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&final_hash));

    let mut diff = Command::cargo_bin("termtrace").expect("binary should build");
    diff.args(["analyze", "diff"])
        .arg(&session)
        .args([
            "--from-state-hash",
            &initial.record.state_hash,
            "--to-state-hash",
            &final_hash,
            "--format",
            "json-compact",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""changed":true"#));

    let mut explain = Command::cargo_bin("termtrace").expect("binary should build");
    explain
        .args(["analyze", "explain"])
        .arg(&session)
        .args(["--state-hash", &final_hash, "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&final_hash));

    let save = temporary.path().join("nested/artifacts/hash-save");
    let mut save_command = Command::cargo_bin("termtrace").expect("binary should build");
    save_command
        .args(["artifact", "save"])
        .arg(&session)
        .arg("--state-hash")
        .arg(&final_hash)
        .arg("--out")
        .arg(&save)
        .args(["--format", "txt"])
        .assert()
        .success();

    assert!(save.join("screen.txt").is_file());

    let screenshot = temporary.path().join("other/nested/hash-screenshot");
    let mut screenshot_command = Command::cargo_bin("termtrace").expect("binary should build");
    screenshot_command
        .args(["screenshot"])
        .arg(&session)
        .arg("--state-hash")
        .arg(&final_hash)
        .arg("--out")
        .arg(&screenshot)
        .assert()
        .success();

    assert!(screenshot.join("screen.png").is_file());

    let mut invalid = Command::cargo_bin("termtrace").expect("binary should build");
    invalid
        .arg("screen")
        .arg(&session)
        .args(["--state-hash", &final_hash.to_uppercase()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("lowercase hexadecimal"));
}

#[test]
fn bundles_publish_sensitivity_profiles_and_verify_offline() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("bundle-session");
    let bundle = temporary.path().join("forensic-bundle");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf 'BUNDLE'"])
        .assert()
        .success();

    let mut dry_run = Command::cargo_bin("termtrace").expect("binary should build");
    dry_run
        .args(["bundle"])
        .arg(&session)
        .args(["--profile", "debug", "--out"])
        .arg(temporary.path().join("unused"))
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("events-metadata.jsonl"))
        .stdout(predicate::str::contains("operational_metadata"))
        .stdout(predicate::str::contains(
            r#""includes_canonical_evidence":false"#,
        ));

    let mut create = Command::cargo_bin("termtrace").expect("binary should build");
    create
        .args(["bundle"])
        .arg(&session)
        .args(["--profile", "forensic", "--out"])
        .arg(&bundle)
        .assert()
        .success();

    assert_eq!(
        fs_err::metadata(&bundle)
            .expect("bundle metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(bundle.join("canonical/completion.json").is_file());

    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .args(["integrity", "verify-bundle"])
        .arg(&bundle)
        .arg("--source")
        .arg(&session)
        .args(["--profile", "forensic"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""valid":true"#));

    let mut wrong_profile = Command::cargo_bin("termtrace").expect("binary should build");
    wrong_profile
        .args(["integrity", "verify-bundle"])
        .arg(&bundle)
        .arg("--source")
        .arg(&session)
        .args(["--profile", "screen"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("caller-owned source identity"));

    let other_session = temporary.path().join("other-bundle-session");
    let mut record_other = Command::cargo_bin("termtrace").expect("binary should build");
    record_other
        .args(["record", "--no-input", "--out"])
        .arg(&other_session)
        .args(["--", "/bin/sh", "-c", "printf 'OTHER'"])
        .assert()
        .success();

    let mut wrong_source = Command::cargo_bin("termtrace").expect("binary should build");
    wrong_source
        .args(["integrity", "verify-bundle"])
        .arg(&bundle)
        .arg("--source")
        .arg(&other_session)
        .args(["--profile", "forensic"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("canonical source identity"));

    let manifest_path = bundle.join("manifest.json");
    let original_manifest = fs_err::read(&manifest_path).expect("original bundle manifest");
    let mut attacker_manifest: serde_json::Value =
        serde_json::from_slice(&original_manifest).expect("bundle manifest JSON");
    attacker_manifest["members"][0]["sensitivity"] =
        serde_json::Value::String("operational_metadata".to_owned());
    fs_err::write(
        &manifest_path,
        serde_json::to_vec_pretty(&attacker_manifest).expect("attacker manifest bytes"),
    )
    .expect("write attacker policy metadata");

    let mut reject_policy_metadata = Command::cargo_bin("termtrace").expect("binary should build");
    reject_policy_metadata
        .args(["integrity", "verify-bundle"])
        .arg(&bundle)
        .arg("--source")
        .arg(&session)
        .args(["--profile", "forensic"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("semantic metadata"));

    fs_err::write(&manifest_path, &original_manifest).expect("restore bundle manifest");

    fs_err::write(&manifest_path, vec![b' '; 1024 * 1024 + 1]).expect("write oversized manifest");

    let mut reject_large_manifest = Command::cargo_bin("termtrace").expect("binary should build");
    reject_large_manifest
        .args(["integrity", "verify-bundle"])
        .arg(&bundle)
        .arg("--source")
        .arg(&session)
        .args(["--profile", "forensic"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("1048576 byte limit"));

    fs_err::write(&manifest_path, &original_manifest).expect("restore bounded manifest");

    let archive = temporary.path().join("forensic.termtrace-bundle.tar.zst");
    let archive_copy = temporary
        .path()
        .join("forensic-copy.termtrace-bundle.tar.zst");

    for destination in [&archive, &archive_copy] {
        let mut create_archive = Command::cargo_bin("termtrace").expect("binary should build");
        create_archive
            .args(["bundle"])
            .arg(&session)
            .args(["--profile", "forensic", "--archive", "--out"])
            .arg(destination)
            .assert()
            .success()
            .stdout(predicate::str::contains(r#""kind":"bundle_archive""#));
    }

    assert_eq!(
        fs_err::read(&archive).expect("archive bytes"),
        fs_err::read(&archive_copy).expect("repeat archive bytes")
    );
    assert_eq!(
        fs_err::metadata(&archive)
            .expect("archive metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let mut verify_archive = Command::cargo_bin("termtrace").expect("binary should build");
    verify_archive
        .args(["integrity", "verify-bundle"])
        .arg(&archive)
        .arg("--source")
        .arg(&session)
        .args(["--profile", "forensic"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""valid":true"#));

    let mut corrupted = fs_err::read(&archive_copy).expect("corruptible archive");
    let middle = corrupted.len() / 2;

    if let Some(byte) = corrupted.get_mut(middle) {
        *byte ^= 0x80;
    }

    fs_err::write(&archive_copy, corrupted).expect("write corrupted archive");

    let mut reject_archive = Command::cargo_bin("termtrace").expect("binary should build");
    reject_archive
        .args(["integrity", "verify-bundle"])
        .arg(&archive_copy)
        .arg("--source")
        .arg(&session)
        .args(["--profile", "forensic"])
        .assert()
        .failure();

    fs_err::write(bundle.join("screen.txt"), b"tampered").expect("tamper bundle");

    let mut tampered = Command::cargo_bin("termtrace").expect("binary should build");
    tampered
        .args(["integrity", "verify-bundle"])
        .arg(&bundle)
        .arg("--source")
        .arg(&session)
        .args(["--profile", "forensic"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "payload members or semantic metadata",
        ));
}
