//! artifacts and bundles CLI integration tests.

use crate::support::*;

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
            "printf 'A\\033[1mA\\033[22;3mA\\033[1mA\\033[0m AgÉgj ─│█ 界\\r\\n\\033[58;2;255;0;255m\\033[4:1m1 \\033[4:2m2 \\033[4:3m3 \\033[4:4m4 \\033[4:5m5\\033[0m \\033[9mstrike\\033[0m \\033[53moverline\\033[0m'; printf '\\033[2 q\\033[1;1H'",
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
    assert_eq!(manifest["version"], 4);
    assert_eq!(manifest["files"].as_array().map(Vec::len), Some(7));
    let image_resource = &manifest["semantic_image_resources"][0];
    let fonts = image_resource["fonts"].as_array().expect("font resources");
    assert_eq!(fonts.len(), 5);
    let roles: Vec<&str> = fonts
        .iter()
        .map(|font| return font["role"].as_str().expect("font role"))
        .collect();
    assert_eq!(
        roles,
        [
            "regular",
            "bold",
            "italic",
            "bold-italic",
            "symbols-fallback"
        ]
    );
    let families: Vec<&str> = fonts
        .iter()
        .map(|font| return font["family"].as_str().expect("font family"))
        .collect();
    assert_eq!(
        families,
        [
            "Termtrace JetBrains Mono",
            "Termtrace JetBrains Mono",
            "Termtrace JetBrains Mono",
            "Termtrace JetBrains Mono",
            "Termtrace Symbols Nerd Font"
        ]
    );
    for font in fonts {
        assert!(
            font["repository_source"]
                .as_str()
                .is_some_and(|source| return source.starts_with("crates/render/assets/fonts/"))
        );
        assert!(
            font["decompressed_size"]
                .as_u64()
                .is_some_and(|size| return size > 0)
        );
        assert_eq!(font["decompressed_blake3"].as_str().map(str::len), Some(64));
    }
    assert_eq!(
        image_resource["font_set_blake3"],
        "013d52d10b90498adec2d19598f4040731dc00cf1539e748f05d86d355f919ec"
    );
    assert_eq!(
        image_resource["fallback_policy"],
        "style_face_then_symbols_nerd_font_then_missing_glyph_box"
    );
    assert_eq!(image_resource["raster_profile"], "supersampled-2x");
    assert_eq!(image_resource["font_size"], 20);
    assert_eq!(image_resource["text_baseline"], 20);
    assert_eq!(image_resource["symbols_font_size"], 12);
    assert_eq!(image_resource["symbols_baseline"], 16);
    assert_eq!(image_resource["cell_width"], 12);
    assert_eq!(image_resource["cell_height"], 24);
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
    assert!(
        fs_err::read_to_string(destination.join("screen.txt"))
            .expect("screen text")
            .contains("AAAA")
    );
    assert!(
        fs_err::read(destination.join("screen.vt"))
            .expect("saved VT")
            .contains(&0x1b)
    );
    assert!(
        fs_err::read_to_string(destination.join("screen.html"))
            .expect("saved HTML")
            .contains(">A<")
    );
    let svg = fs_err::read_to_string(destination.join("screen.svg")).expect("saved SVG");
    assert!(svg.contains("data-truth-layer=\"semantic_rendering\""));
    assert_eq!(svg.matches("@font-face").count(), 5);
    assert!(
        svg.matches("font-family=\"Termtrace JetBrains Mono\"")
            .count()
            >= 4
    );
    assert!(
        svg.matches("font-family=\"Termtrace Symbols Nerd Font\"")
            .count()
            >= 2
    );
    for attributes in [
        "font-weight=\"normal\" font-style=\"normal\"",
        "font-weight=\"bold\" font-style=\"normal\"",
        "font-weight=\"normal\" font-style=\"italic\"",
        "font-weight=\"bold\" font-style=\"italic\"",
    ] {
        assert!(svg.contains(attributes));
    }
    assert!(svg.contains("></text>") && svg.contains("></text>"));
    assert!(svg.contains("stroke=\"#ff00ff\""));
    assert!(svg.contains("stroke-dasharray"));
    assert!(svg.contains("<path d="));
    assert!(svg.contains("<line "));
    let png = fs_err::read(destination.join("screen.png")).expect("saved PNG");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(
        (
            u32::from_be_bytes(png[16..20].try_into().expect("PNG width")),
            u32::from_be_bytes(png[20..24].try_into().expect("PNG height")),
        ),
        (1920, 1080),
        "the default semantic screenshot must match the default 1080p recording canvas"
    );
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
    assert_eq!(batch_manifest["version"], 3);
    assert_eq!(batch_manifest["entries"][0]["manifest"]["version"], 4);
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
        .args(["--response", "full", "screenshot"])
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
    show.arg("show")
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
        .arg("show")
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
        .args(["--response", "full", "bundle"])
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
