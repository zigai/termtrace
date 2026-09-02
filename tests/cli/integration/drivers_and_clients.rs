//! drivers and clients CLI integration tests.

use crate::support::*;

#[test]
fn driver_handshake_attaches_sessions_and_caches_correlated_retries() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("driver-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["/bin/sh", "-c", "printf DRIVER"])
        .assert()
        .success();
    let hello_id = "019f0000-0000-7000-8000-000000000001";
    let attach_id = "019f0000-0000-7000-8000-000000000002";
    let hello = serde_json::json!({
        "version": 5,
        "request_id": hello_id,
        "method": { "method": "hello" }
    });
    let attach = serde_json::json!({
        "version": 5,
        "request_id": attach_id,
        "method": { "method": "attach", "session": session }
    });
    let input = format!("{hello}\n{hello}\n{attach}\n");
    let mut driver = Command::cargo_bin("termtrace").expect("binary should build");
    let output = driver
        .arg("agent")
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let responses = output
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
        .map(|line| return serde_json::from_slice::<serde_json::Value>(line).expect("driver JSON"))
        .collect::<Vec<_>>();
    let [first, second, third] = responses.as_slice() else {
        panic!("expected three driver responses");
    };
    assert_eq!(first, second);
    assert_eq!(first["result"]["data"]["kind"], "hello");
    assert_eq!(first["result"]["data"]["driver_protocol_version"], 5);
    assert_eq!(third["result"]["data"]["kind"], "attached");
    assert_eq!(third["result"]["data"]["completed"], true);
    assert!(third["result"]["data"]["handle"].is_string());
}

#[test]
fn python_sync_and_async_clients_parse_driver_completion_contract() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("python-client-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf PYTHON"])
        .assert()
        .success();
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/python");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let sync_code = r#"import json, sys
from pathlib import Path
from termtrace import EnvironmentOptions, Terminal, assert_frame_text, assert_terminal_snapshot
with Terminal(binary_path=sys.argv[1]) as terminal:
    Path(sys.argv[3]).mkdir(parents=True, exist_ok=True)
    session = terminal.attach(sys.argv[2])
    assert str(session.evidence.completion().session_id) == session.session_id
    assert session.evidence.verify().valid is True
    assert session.evidence.bundle(Path(sys.argv[3]) / "python-screen-bundle").directory
    assert isinstance(session.session_id, str)
    assert session.semantic.inspect().state_hash
    assert isinstance(session.semantic.timeline().entries, list)
    assert isinstance(session.semantic.diff({"selector": "state_id", "state_id": 0}, {"selector": "latest_presented"}).rows, list)
    assert isinstance(session.semantic.diagnose().findings, list)
    assert session.events.page(metadata_only=True).next_event_seq >= 0
    semantic_image = session.screen.screenshot(Path(sys.argv[3]) / "python-semantic-image")
    assert Path(semantic_image.primary).is_file()
    assert_frame_text(session.screen.frame(), "PYTHON")
    snapshot = Path(sys.argv[3]) / "completed.snapshot.json"
    assert_terminal_snapshot(session, snapshot, update=True)
    assert_terminal_snapshot(session, snapshot)
    session.close()
    launched = terminal.launch(
        ["/bin/sh", "-c", "test \"${#CLIENT_SECRET}\" -eq 20; test -z \"${HOME+x}\"; printf PY_LAUNCH; sleep 30"],
        evidence_root=sys.argv[3],
        environment=EnvironmentOptions(inherit_env=False, set={"CLIENT_SECRET": "python-private-value"}, copy=["PATH"], color="never"),
        output_byte_budget=1000000,
    )
    assert launched.canonical_session.is_dir()
    launched.screen.wait_for_text("PY_LAUNCH")
    launched.screen.wait_for_regex("PY_[A-Z]+")
    assert launched.screen.wait_for_idle().quality == "stable"
    assert b"python-private-value" not in launched.canonical_session.joinpath("session.json").read_bytes()
    assert json.loads(launched.canonical_session.joinpath("session.json").read_text())["resource_limits"]["output_bytes"] == 1000000
    assert "PY_LAUNCH" in launched.screen.text()
    assert launched.screen.frame().viewport_text.startswith("PY_LAUNCH")
    assert launched.output.wait_for_text("PY_LAUNCH").event_seq >= 0
    launched.stop()
    assert str(launched.evidence.complete().session_id) == launched.session_id
    launched.close()
    with terminal.launch(["/bin/sh", "-c", "printf DISPOSABLE; sleep 30"], evidence_root=sys.argv[3], evidence="on-failure") as disposable:
        disposable_path = disposable.canonical_session
        disposable.screen.wait_for_text("DISPOSABLE")
    assert not disposable_path.exists()
    try:
        with terminal.launch(["/bin/sh", "-c", "printf RETAINED; sleep 30"], evidence_root=sys.argv[3], evidence="on-failure") as retained:
            retained_path = retained.canonical_session
            retained.screen.wait_for_text("RETAINED")
            raise RuntimeError("fixture failure")
    except RuntimeError:
        pass
    assert retained_path.joinpath("completion.json").is_file()
"#;
    let sync = StdCommand::new("uv")
        .args(["run", "--isolated", "--project"])
        .arg(&package)
        .arg("python")
        .arg("-c")
        .arg(sync_code)
        .arg(binary)
        .arg(&session)
        .arg(temporary.path().join("python-sync-evidence"))
        .status()
        .expect("run sync Python client");
    assert!(sync.success());

    let async_code = r#"import asyncio, json, sys
from pathlib import Path
from termtrace import AsyncTerminal, EnvironmentOptions
async def main():
    Path(sys.argv[3]).mkdir(parents=True, exist_ok=True)
    terminal = await AsyncTerminal.create(binary_path=sys.argv[1])
    async with terminal:
        session = await terminal.attach(sys.argv[2])
        assert str((await session.completion()).session_id) == session.session_id
        assert (await session.evidence.verify()).valid is True
        assert (await session.evidence.bundle(Path(sys.argv[3]) / "async-screen-bundle")).directory
        assert isinstance(session.session_id, str)
        assert (await session.semantic.inspect()).state_hash
        assert isinstance((await session.semantic.timeline()).entries, list)
        assert isinstance((await session.semantic.diff({"selector": "state_id", "state_id": 0}, {"selector": "latest_presented"})).rows, list)
        assert isinstance((await session.semantic.diagnose()).findings, list)
        assert (await session.events.page(metadata_only=True)).next_event_seq >= 0
        semantic_image = await session.screen.screenshot(Path(sys.argv[3]) / "async-semantic-image")
        assert Path(semantic_image.primary).is_file()
        await session.close()
        launched = await terminal.launch(
            ["/bin/sh", "-c", "test \"${#CLIENT_SECRET}\" -eq 19; test -z \"${HOME+x}\"; printf ASYNC_LAUNCH; sleep 30"],
            evidence_root=sys.argv[3],
            environment=EnvironmentOptions(inherit_env=False, set={"CLIENT_SECRET": "async-private-value"}, copy=["PATH"]),
            output_byte_budget=1000000,
        )
        assert launched.canonical_session.is_dir()
        await launched.screen.wait_for_text("ASYNC_LAUNCH")
        await launched.screen.wait_for_regex("ASYNC_[A-Z]+")
        assert (await launched.screen.wait_for_idle()).quality == "stable"
        assert b"async-private-value" not in launched.canonical_session.joinpath("session.json").read_bytes()
        assert json.loads(launched.canonical_session.joinpath("session.json").read_text())["resource_limits"]["output_bytes"] == 1000000
        assert "ASYNC_LAUNCH" in await launched.screen.text()
        assert (await launched.screen.frame()).viewport_text.startswith("ASYNC_LAUNCH")
        assert (await launched.output.wait_for_text("ASYNC_LAUNCH")).event_seq >= 0
        await launched.stop()
        assert str((await launched.evidence.complete()).session_id) == launched.session_id
        await launched.close()
        disposable = await terminal.launch(["/bin/sh", "-c", "printf ASYNC_DISPOSABLE; sleep 30"], evidence_root=sys.argv[3], evidence="on-failure")
        disposable_path = disposable.canonical_session
        async with disposable:
            await disposable.screen.wait_for_text("ASYNC_DISPOSABLE")
        assert not disposable_path.exists()
asyncio.run(main())
"#;
    let asynchronous = StdCommand::new("uv")
        .args(["run", "--isolated", "--project"])
        .arg(package)
        .arg("python")
        .arg("-c")
        .arg(async_code)
        .arg(binary)
        .arg(&session)
        .arg(temporary.path().join("python-async-evidence"))
        .status()
        .expect("run async Python client");
    assert!(asynchronous.success());
}

#[test]
fn typescript_client_attaches_and_reads_completion_contract() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("typescript-client-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf TYPESCRIPT"])
        .assert()
        .success();
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/typescript/src/index.ts");
    let code = r#"const { existsSync, mkdirSync } = await import("node:fs")
const { Termtrace, assertScreenText, stableTerminalSnapshot } = await import(process.argv[1])
mkdirSync(process.argv[4], { recursive: true })
await using terminal = await Termtrace.create({ binaryPath: process.argv[2] })
await using attached = await terminal.attach(process.argv[3])
const completion = await attached.evidence.complete()
if (completion.session_id !== attached.sessionId) throw new Error("missing completion")
if ((await attached.evidence.verify()).valid !== true) throw new Error("invalid evidence")
if (!(await attached.evidence.bundle(`${process.argv[4]}/typescript-screen-bundle`)).directory) throw new Error("missing bundle")
if (typeof attached.sessionId !== "string") throw new Error("missing session identity")
if (!(await attached.semantic.inspect()).state_hash) throw new Error("missing observation")
if (!Array.isArray((await attached.semantic.timeline()).entries)) throw new Error("missing timeline")
if (!Array.isArray((await attached.semantic.diff({ selector: "state_id", state_id: 0 }, { selector: "latest_presented" })).rows)) throw new Error("missing diff")
if (!Array.isArray((await attached.semantic.diagnose()).findings)) throw new Error("missing diagnosis")
if ((await attached.events.page({ metadataOnly: true })).kind !== "events") throw new Error("missing events")
const semanticImage = await attached.screen.screenshot(`${process.argv[4]}/typescript-semantic-image`)
if (semanticImage.kind !== "artifact" || !existsSync(String(semanticImage.primary))) throw new Error("missing semantic screenshot")
await assertScreenText(attached, "TYPESCRIPT")
if ((await stableTerminalSnapshot(attached)).version !== 1) throw new Error("missing stable snapshot")
await using launched = await terminal.launch({
  command: ["/bin/sh", "-c", "test \"${#CLIENT_SECRET}\" -eq 16; test -z \"${HOME+x}\"; printf TS_LAUNCH; sleep 30"],
  evidenceRoot: process.argv[4],
  environment: { inheritEnv: false, set: { CLIENT_SECRET: "ts-private-value" }, copy: ["PATH"], color: "never" },
  outputByteBudget: 1000000,
})
if (typeof launched.sessionId !== "string") throw new Error("missing launched identity")
for (const timeoutMs of [-1, Number.NaN, Number.POSITIVE_INFINITY, Math.floor(Number.MAX_SAFE_INTEGER / 1_000_000) + 1]) {
  try {
    await launched.screen.waitForPredicate({ predicate: "contains", text: "TS_LAUNCH" }, { timeoutMs })
    throw new Error("invalid duration was accepted")
  } catch (cause) {
    if (!(cause instanceof TypeError)) throw cause
  }
}
await launched.screen.waitForText(/TS_LAUNCH/)
await launched.screen.waitForPredicate({ predicate: "row_contains", row: 0, text: "TS_LAUNCH" })
if ((await launched.screen.waitForIdle()).kind !== "stable_snapshot") throw new Error("missing stable capture")
if (Buffer.from(await Bun.file(`${launched.canonicalSession}/session.json`).arrayBuffer()).includes("ts-private-value")) throw new Error("environment value leaked")
if ((await Bun.file(`${launched.canonicalSession}/session.json`).json()).resource_limits.output_bytes !== 1000000) throw new Error("output budget missing")
if (!await launched.screen.text().then((text) => text.includes("TS_LAUNCH"))) throw new Error("missing screen text")
if (!String((await launched.screen.frame()).viewport_text).includes("TS_LAUNCH")) throw new Error("missing frame")
if ((await launched.output.waitForText("TS_LAUNCH")).event_seq < 0) throw new Error("missing output wait")
await launched.stop()
if ((await launched.evidence.complete()).session_id !== launched.sessionId) throw new Error("missing launched completion")
const disposable = await terminal.launch({ command: ["/bin/sh", "-c", "printf TS_DISPOSABLE; sleep 30"], evidenceRoot: process.argv[4], evidence: "on-failure" })
const disposablePath = disposable.canonicalSession
await disposable.screen.waitForText("TS_DISPOSABLE")
await disposable.finish(true)
if (existsSync(disposablePath)) throw new Error("successful disposable evidence remained")
const retained = await terminal.launch({ command: ["/bin/sh", "-c", "printf TS_RETAINED; sleep 30"], evidenceRoot: process.argv[4], evidence: "on-failure" })
const retainedPath = retained.canonicalSession
await retained.screen.waitForText("TS_RETAINED")
await retained.finish(false)
if (!existsSync(`${retainedPath}/completion.json`)) throw new Error("failed-run evidence was not retained")
"#;
    let status = StdCommand::new("bun")
        .arg("-e")
        .arg(code)
        .arg(package)
        .arg(env!("CARGO_BIN_EXE_termtrace"))
        .arg(&session)
        .arg(temporary.path().join("typescript-evidence"))
        .status()
        .expect("run TypeScript client");
    assert!(status.success());
}

#[test]
fn managed_client_runners_leave_the_callers_worktree_untouched() {
    let temporary = TempDir::new().expect("temporary directory");
    let assets = temporary.path().join("client-assets");
    let typescript = assets.join("typescript");
    fs_err::create_dir_all(typescript.join("lib")).expect("TypeScript fixture assets");
    symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/python"),
        assets.join("python"),
    )
    .expect("Python fixture asset link");
    fs_err::write(
        typescript.join("package.json"),
        br#"{"name":"@termtrace/test","type":"module","exports":{".":"./lib/index.js"}}"#,
    )
    .expect("TypeScript fixture package");
    fs_err::write(
        typescript.join("lib/index.js"),
        b"export const managedRunnerMarker = 'typescript-managed';\n",
    )
    .expect("TypeScript fixture module");
    let worktree = temporary.path().join("worktree");
    fs_err::create_dir(&worktree).expect("empty caller worktree");

    let mut python = Command::cargo_bin("termtrace").expect("binary should build");
    python
        .env("TERMTRACE_CLIENT_ASSETS", &assets)
        .current_dir(&worktree)
        .args(["tooling", "client", "python", "-"])
        .write_stdin("from termtrace import Terminal\nprint(Terminal.__name__)\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Terminal"));

    let mut typescript_run = Command::cargo_bin("termtrace").expect("binary should build");
    typescript_run
        .env("TERMTRACE_CLIENT_ASSETS", &assets)
        .current_dir(&worktree)
        .args(["tooling", "client", "typescript", "-"])
        .write_stdin(
            "import { managedRunnerMarker } from '@termtrace/test';\nconsole.log(managedRunnerMarker);\n",
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("typescript-managed"));

    let source = temporary.path().join("typescript-source");
    fs_err::create_dir(&source).expect("TypeScript source directory");
    fs_err::write(
        source.join("helper.ts"),
        "export const localMarker = 'local-import';\n",
    )
    .expect("TypeScript helper");
    fs_err::write(
        source.join("client.ts"),
        "import { managedRunnerMarker } from '@termtrace/test';\nimport { localMarker } from './helper.ts';\nconsole.log(`${managedRunnerMarker}:${localMarker}`);\n",
    )
    .expect("TypeScript client file");
    let mut typescript_file = Command::cargo_bin("termtrace").expect("binary should build");
    typescript_file
        .env("TERMTRACE_CLIENT_ASSETS", &assets)
        .current_dir(&worktree)
        .args(["tooling", "client", "typescript"])
        .arg(source.join("client.ts"))
        .assert()
        .success()
        .stdout(predicate::str::contains("typescript-managed:local-import"));

    assert_eq!(
        fs_err::read_dir(&worktree)
            .expect("caller worktree")
            .count(),
        0,
        "managed runners must not create project files"
    );
}
