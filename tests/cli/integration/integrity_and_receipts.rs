//! integrity and receipts CLI integration tests.

use crate::support::*;

#[test]
fn verify_reports_manifest_binding_and_detects_tampering() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("verified-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf verified"])
        .assert()
        .success();

    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    let output = verify
        .args(["integrity", "verify"])
        .arg(&session)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value = serde_json::from_slice(&output).expect("verify JSON");

    assert_eq!(report["valid"], true);
    assert_eq!(report["level"], "content");

    let mut verify_durability = Command::cargo_bin("termtrace").expect("binary should build");
    let output = verify_durability
        .args(["integrity", "verify"])
        .arg(&session)
        .args(["--level", "durability-record", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value =
        serde_json::from_slice(&output).expect("verify durability JSON");

    assert_eq!(report["valid"], true);
    assert_eq!(report["level"], "durability_record");

    let manifest_path = session.join("session.json");
    let manifest = fs_err::read_to_string(&manifest_path).expect("session manifest");
    let tampered = manifest.replace("L2Jpbi9zaA==", "L2Jpbi9ueA==");
    assert_ne!(manifest, tampered, "fixture command should be replaced");

    fs_err::write(&manifest_path, tampered).expect("tamper session manifest");

    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    let output = verify
        .args(["integrity", "verify"])
        .arg(&session)
        .arg("--json")
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value = serde_json::from_slice(&output).expect("tampered verify JSON");

    assert_eq!(report["valid"], false);
    assert!(
        report["findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .any(|f| return f["code"] == "session_manifest_hash_mismatch"),
        "expected session_manifest_hash_mismatch finding in: {report:?}"
    );
}

#[test]
fn verify_resolves_session_names() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "record",
            "-d",
            "named",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf named; while :; do sleep .1; done",
        ])
        .assert()
        .success();

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "named", "--wait"])
        .assert()
        .success();

    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    let output = verify
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["analyze", "verify", "named", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value = serde_json::from_slice(&output).expect("verify JSON");

    assert_eq!(report["valid"], true);
}

#[test]
fn detached_receipt_separates_cryptography_from_signer_trust() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("signed-session");
    let key = temporary.path().join("signing-key");
    let allowed = temporary.path().join("allowed-signers");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/true"])
        .assert()
        .success();

    assert!(
        StdCommand::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .status()
            .expect("launch key generator")
            .success(),
        "generate signing key"
    );

    let public = fs_err::read_to_string(key.with_extension("pub")).expect("public key");
    let mut fields = public.split_whitespace();
    let algorithm = fields.next().expect("public key algorithm");
    let encoded = fields.next().expect("public key value");

    fs_err::write(&allowed, format!("fixture-signer {algorithm} {encoded}\n"))
        .expect("allowed signers");

    let mut sign = Command::cargo_bin("termtrace").expect("binary should build");
    sign.args(["integrity", "sign"])
        .arg(&session)
        .arg("--key")
        .arg(&key)
        .args(["--signer", "fixture-signer"])
        .assert()
        .success();

    let mut cryptography_only = Command::cargo_bin("termtrace").expect("binary should build");
    let output = cryptography_only
        .args(["integrity", "verify"])
        .arg(&session)
        .args(["--level", "signature", "--json"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let verified: serde_json::Value = serde_json::from_slice(&output).expect("verify JSON");

    assert_eq!(verified["signature_cryptographic_valid"], true);
    assert_eq!(verified["signature_signer_trusted"], false);
    assert!(
        verified["findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .any(|f| return f["code"] == "signature_trust_policy_missing"),
        "expected signature_trust_policy_missing finding in: {verified:?}"
    );

    let mut trusted = Command::cargo_bin("termtrace").expect("binary should build");
    let trusted_output = trusted
        .args(["integrity", "verify"])
        .arg(&session)
        .args(["--level", "signature", "--allowed-signers"])
        .arg(&allowed)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let trusted_json: serde_json::Value =
        serde_json::from_slice(&trusted_output).expect("trusted JSON");

    assert_eq!(trusted_json["session_authenticity_established"], true);
    assert_eq!(trusted_json["signature_signer_trusted"], true);

    let receipt_path = session.join("evidence-receipt.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs_err::read(&receipt_path).expect("receipt"))
            .expect("receipt JSON");
    *receipt
        .pointer_mut("/signature/value")
        .expect("signature value") = serde_json::to_value(ByteString::from(b"invalid".as_slice()))
        .expect("invalid signature value");
    fs_err::write(
        &receipt_path,
        serde_json::to_vec_pretty(&receipt).expect("tampered receipt JSON"),
    )
    .expect("tamper receipt");

    let mut tampered = Command::cargo_bin("termtrace").expect("binary should build");
    let output = tampered
        .args(["integrity", "verify"])
        .arg(&session)
        .args(["--level", "signature", "--allowed-signers"])
        .arg(&allowed)
        .arg("--json")
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value = serde_json::from_slice(&output).expect("tampered verify JSON");

    assert_eq!(report["valid"], false);
    assert!(
        report["findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .any(|f| return f["code"] == "signature_invalid"),
        "expected signature_invalid finding in: {report:?}"
    );
}
