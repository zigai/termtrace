"""Typed diagnostics, policy, trust and management against real shared-service recordings."""

from __future__ import annotations

import base64
import gc
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path

import pytest
from termtrace import Edit, Instrumentation, Select, Termtrace, TermtraceError, aio
from test_evidence_pages import BINARY, ROOT, evidence_root

FIXTURE = json.loads(
    (ROOT / "fixtures/conformance/sdk/diagnostics-evidence.json").read_bytes()
)


def canonical_hashes(source: str | Path) -> dict[str, str]:
    return {
        p.name: hashlib.sha256(p.read_bytes()).hexdigest()
        for p in Path(source).iterdir()
        if p.is_file()
    }


def normalize(observation):
    return {**observation, "resolution": "canonical_replay"}


@pytest.fixture
def completed(tmp_path: Path):
    root = evidence_root(tmp_path)
    root.chmod(0o700)
    client = Termtrace(binary=BINARY, evidence_root=str(root / "sessions"))
    rec = client.launch(
        ["/bin/sh", "-c", "printf READY; read line; printf DONE; read line"],
        {"cols": 40, "rows": 8, "environment": {"base": "clear"}},
    )
    try:
        rec.output().wait_for(b"READY", 5)
        rec.mark("cut")
        emitter = rec.instrumentation()
        ack = emitter.event("fixture", FIXTURE["application_attributes"])
        span = emitter.start_span("complete")
        ended = emitter.end_span("complete", span["span_id"])
        incomplete = emitter.start_span("incomplete", span["span_id"])
        assert ended["span_id"] == span["span_id"]
        assert ack["process_instance_id"] == emitter.process_instance_id
        assert ack["boundaries"]["durable"]["next_event_seq"] > 0
        with pytest.raises((ValueError, OverflowError, TermtraceError)):
            emitter.event("unsafe", {"nested": {"inode": 2**53 + 1}})
        cyclic = {}
        cyclic["self"] = cyclic
        cyclic_array = []
        cyclic_array.append(cyclic_array)
        deep = None
        for _ in range(130):
            deep = [deep]
        for invalid in [
            cyclic,
            {"array": cyclic_array},
            {"deep": deep},
            {"large": "x" * 65536},
            {"many": [None] * 65537},
        ]:
            with pytest.raises((ValueError, OverflowError, TermtraceError)):
                emitter.event("unsafe", invalid)
        del emitter
        gc.collect()
        rec.actions().send_text("go\n").until_screen("DONE", 5).run()
        rec.stop()
    finally:
        if rec.is_active():
            rec.stop()
    return root, client, rec, incomplete


def test_typed_diagnostics_all_saves_selectors_cache_profiles_and_rejection(
    completed,
) -> None:
    root, client, rec, incomplete = completed
    diag, evidence = rec.analysis(), rec.evidence()
    attached = client.attach(rec.path)
    before = canonical_hashes(rec.path)
    observations = [normalize(attached.screen().observe()) for _ in range(2)]
    assert observations[0] == observations[1]
    page = diag.timeline(limit=1000, controls=True)
    states = [
        entry for entry in page["entries"] if entry["item"]["kind"] == "semantic_state"
    ]
    last = states[-1]
    assert isinstance(last["item"]["state_id"], int)
    assert diag.diff(
        {"selector": "state_id", "state_id": 0}, {"selector": "latest_presented"}
    )["changed"]
    assert isinstance(diag.explain()["source_event_count"], int)
    assert isinstance(diag.diagnose()["findings"], list)
    assert isinstance(diag.last_action()["diagnostics"], dict)
    assert any(marker["label"] == "cut" for marker in diag.markers())
    assert diag.review_summary()["canonical_session"] == rec.path
    assert isinstance(diag.review_search("READY"), dict)
    exported = diag.review_export(
        last["item"]["state_id"], os.fsencode(root / "review")
    )
    assert exported["canonical_session"] == rec.path
    saved = evidence.save(os.fsencode(root / "all-formats"), FIXTURE["save_formats"])
    assert saved["manifest"]["canonical_session"] == os.fsencode(rec.path)
    assert len(saved["manifest"]["files"]) == len(FIXTURE["save_formats"])
    assert saved["manifest"]["semantic_image_resources"]
    png = next(
        item for item in saved["manifest"]["files"] if item["path"] == "screen.png"
    )
    assert png["truth_layer"] == "semantic_rendering" and png["renderer"]
    assert saved["manifest"]["sensitivity"] == "terminal_screen_may_contain_secrets"
    published = canonical_hashes(root / "all-formats")
    with pytest.raises(TermtraceError):
        evidence.save(os.fsencode(root / "all-formats"), ["txt"])
    assert canonical_hashes(root / "all-formats") == published
    points = [
        {"selector": "latest_presented"},
        {"selector": "state_id", "state_id": last["item"]["state_id"]},
        {"selector": "state_hash", "state_hash": last["item"]["state_hash"]},
        {"selector": "event_seq", "event_seq": last["event_seq"]},
        {"selector": "monotonic_ns", "monotonic_ns": last["monotonic_ns"]},
        {"selector": "marker", "label": "cut"},
    ]
    assert [point["selector"] for point in points] == FIXTURE["selectors"]
    for point in points:
        result = evidence.save(os.fsencode(root / point["selector"]), ["txt"], point)
        assert result["manifest"]["state_point"] == point
    evidence.asciicast(os.fsencode(root / "session.cast"))
    assert (root / "session.cast").is_file()
    other = client.launch(["/bin/true"], {"environment": {"base": "clear"}})
    other.stop()
    for profile in FIXTURE["profiles"]:
        dest = root / profile
        report = evidence.bundle(os.fsencode(dest), profile)
        assert report["profile"] == profile
        assert report["includes_canonical_evidence"] == (profile == "forensic")
        assert all(item["sensitivity"] for item in report["members"])
        assert diag.cache_prune()["remaining"]["entries"] == 0
        assert normalize(attached.screen().observe()) == observations[0]
        assert evidence.verify_bundle(os.fsencode(dest), profile) == report
        with pytest.raises(TermtraceError):
            client.evidence(os.fsencode(other.path)).verify_bundle(
                os.fsencode(dest), profile
            )
        with pytest.raises(TermtraceError):
            evidence.verify_bundle(
                os.fsencode(dest), "debug" if profile == "screen" else "screen"
            )
        with pytest.raises(TermtraceError):
            evidence.bundle(os.fsencode(dest), profile)
        member = next(
            item for item in report["members"] if item["path"] == "screen.txt"
        )
        with (dest / member["path"]).open("ab") as stream:
            stream.write(b"TAMPER")
        with pytest.raises(TermtraceError):
            evidence.verify_bundle(os.fsencode(dest), profile)
    for operation in [
        lambda: evidence.bundle(os.fsencode(root / "typo"), "forensik"),
        lambda: evidence.verify("signatur"),
        lambda: diag.timeline(limit=0),
        lambda: diag.timeline(start_monotonic_ns=1),
        lambda: diag.timeline(start_monotonic_ns=2, end_monotonic_ns=1),
    ]:
        with pytest.raises((ValueError, TermtraceError)):
            operation()
    assert canonical_hashes(rec.path) == before
    events = rec.events().page(limit=1000, event_types=["application"])["events"]
    app = [entry["event"]["data"] for entry in events]
    assert (
        next(item for item in app if item["name"] == "fixture")["attributes"]
        == FIXTURE["application_attributes"]
    )
    assert not any(item["name"] == "unsafe" for item in app)
    assert any(
        item["span_id"] == incomplete["span_id"] and item["kind"] == "span_start"
        for item in app
    )
    assert not any(
        item["span_id"] == incomplete["span_id"] and item["kind"] == "span_end"
        for item in app
    )
    metadata = rec.events().page(
        limit=1000, metadata_only=True, event_types=["application"]
    )
    assert all("attributes" not in entry["metadata"] for entry in metadata["events"])
    with pytest.raises(TermtraceError):
        Instrumentation(os.fsencode(rec.path)).event("after-completion")
    (root / "diagnostics-observed.json").write_text(
        json.dumps(
            {
                "source_hashes_before": before,
                "source_hashes_after": canonical_hashes(rec.path),
                "profiles": FIXTURE["profiles"],
                "selectors": FIXTURE["selectors"],
                "incomplete_span_id": incomplete["span_id"],
                "all_formats": FIXTURE["save_formats"],
            },
            indent=2,
        )
    )


def test_real_openssh_trust_is_distinct_from_crypto_and_receipt_presence(
    completed,
) -> None:
    root, _, rec, _ = completed
    evidence = rec.integrity()
    assert evidence.verify("content")["valid"]
    unsigned = evidence.verify("signature")
    assert not unsigned["valid"] and not unsigned["signature_receipt_present"]
    before = canonical_hashes(rec.path)
    with tempfile.TemporaryDirectory(prefix="tt05-ssh-") as directory:
        key = Path(directory) / "key"
        subprocess.run(
            ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)],
            check=True,
            capture_output=True,
        )
        policy = Path(directory) / "allowed"
        policy.write_text("task05 " + key.with_suffix(".pub").read_text())
        wrong = Path(directory) / "wrong"
        wrong.write_text("wrong-identity " + key.with_suffix(".pub").read_text())
        receipt = evidence.sign(os.fsencode(key), "task05")
        path = Path(receipt)
        assert path.name == "evidence-receipt.json"
        untrusted = evidence.verify("signature")
        assert untrusted["signature_cryptographic_valid"]
        assert not untrusted["valid"] and not untrusted["signature_signer_trusted"]
        trusted = evidence.verify("signature", os.fsencode(policy))
        assert trusted["valid"] and trusted["signature_signer_trusted"]
        assert trusted["session_authenticity_established"]
        rejected = evidence.verify("signature", os.fsencode(wrong))
        assert (
            rejected["signature_cryptographic_valid"]
            and not rejected["signature_signer_trusted"]
        )
        original = path.read_bytes()
        path.write_bytes(b"{")
        assert not evidence.verify("signature", os.fsencode(policy))["valid"]
        value = json.loads(original)
        raw = bytearray(base64.b64decode(value["signature"]["value"]))
        raw[len(raw) // 2] ^= 1
        value["signature"]["value"] = base64.b64encode(raw).decode()
        path.write_text(json.dumps(value))
        tampered = evidence.verify("signature", os.fsencode(policy))
        assert (
            not tampered["valid"] and not tampered["session_authenticity_established"]
        )
        path.write_bytes(original)
        assert evidence.verify("signature", os.fsencode(policy))["valid"]
    after = canonical_hashes(rec.path)
    assert {key: after[key] for key in before} == before
    (root / "trust-observed.json").write_text(
        json.dumps(
            {
                "unsigned": unsigned,
                "valid_untrusted": untrusted,
                "trusted": trusted,
                "wrong_signer": rejected,
                "tampered": tampered,
            },
            default=lambda value: value.hex(),
            indent=2,
        )
    )


def test_registry_exact_u128_failure_taxonomy_and_gc_protects_sources(
    tmp_path: Path,
) -> None:
    root = evidence_root(tmp_path)
    root.chmod(0o700)
    state = root / "state"
    registry = Termtrace(binary=BINARY).registry(os.fsencode(state))
    assert registry.resolve("absent") is None
    (state / "sessions").mkdir(mode=0o700)
    client = Termtrace(binary=BINARY, evidence_root=str(state / "sessions"))
    rec = client.launch(["/bin/sh", "-c", "printf LIVE; read line"])
    try:
        rec.output().wait_for(b"LIVE", 5)
        assert registry.resolve("absent") is None
        registry.register("named", os.fsencode(rec.path))
        assert registry.lifecycle("named")["lifecycle"] == "live"
        protected = registry.gc(0, 0, 0, True)
        assert protected["removed_sessions"] == 0 and Path(rec.path).exists()
        rec.stop()
    finally:
        if rec.is_active():
            rec.stop()
    before = canonical_hashes(rec.path)
    record = state / "registry/named.json"
    entry = json.loads(record.read_bytes())
    entry["registered_unix_ns"] = int(FIXTURE["u128_max"])
    record.write_text(json.dumps(entry))
    assert registry.resolve("named")["registered_unix_ns"] == int(FIXTURE["u128_max"])
    assert registry.list(True)[0]["registered_unix_ns"] == int(FIXTURE["u128_max"])
    record.write_bytes(b"{")
    with pytest.raises(TermtraceError):
        registry.resolve("named")
    blocked_gc = registry.gc(0, 0, 0, True)
    assert blocked_gc["skipped_corrupt_registry_entries"] == 1
    assert blocked_gc["removed_sessions"] == 0 and Path(rec.path).exists()
    record.write_text(json.dumps(entry))
    assert not registry.gc(0, 0, 0)["changes_applied"]
    assert Path(rec.path).exists()
    registry.remove("named")
    assert canonical_hashes(rec.path) == before
    dry = registry.gc(0, 0, 0)
    assert dry["candidates"] and Path(rec.path).exists()
    applied = registry.gc(0, 0, 0, True)
    assert applied["removed_sessions"] == 1 and not Path(rec.path).exists()


@pytest.mark.asyncio
async def test_attached_async_report_namespaces_use_native_off_loop_operations(
    completed,
) -> None:
    root, _, rec, _ = completed
    client = aio.AsyncTermtrace(binary=BINARY)
    attached = await client.attach(rec.path)
    assert (await attached.status())["kind"] == "completed"
    assert (await (await attached.integrity()).verify())["valid"]
    assert (await attached.analysis().timeline())["entries"]
    assert (await attached.output().page())["bytes"]
    assert (await attached.events().follow())["kind"] == "completed"
    assert (
        await attached.evidence().bundle(os.fsencode(root / "async-bundle"), "screen")
    )["profile"] == "screen"
    assert (await attached.analysis().cache_prune())["remaining"]["entries"] == 0
    assert (
        await client.registry(os.fsencode(root / "async-state")).resolve("absent")
    ) is None
    versions = await client.system().versions()
    assert versions["target_os"] == "linux"
    schemas = await client.system().schemas()
    assert "recovery-report.schema.json" in schemas
    for schema in schemas:
        assert isinstance(await client.system().schema(schema), dict)
    probes = await client.system().capabilities()
    assert (
        next(
            item for item in probes["capabilities"] if item["id"] == "receipt-signing"
        )["status"]
        == "ready"
    )
    with pytest.raises(TermtraceError):
        await client.system().schema("no-such-schema")


def test_native_restart_success_and_failed_admission_preserve_predecessor(
    completed,
) -> None:
    from termtrace import Registry

    root, client, rec, _ = completed
    state = root / "restart-state"
    registry = client.registry(os.fsencode(state))
    predecessor = registry.register("demo", os.fsencode(rec.path))
    before = canonical_hashes(rec.path)
    changed_environment = {"base": "clear", "set": {"FIXTURE_NEW": b"yes"}}
    with pytest.raises(TermtraceError, match="recoverable orphan"):
        registry.restart(
            "demo",
            {"command": [b"/bin/true"]},
            environment=changed_environment,
            timeout_ms=5000,
        )
    assert registry.resolve("demo")["session_id"] == predecessor["session_id"]
    successor = registry.restart(
        "demo",
        {"command": [b"/bin/true"], "cols": 66, "allow_environment_change": True},
        environment=changed_environment,
        timeout_ms=5000,
    )
    assert successor["ready"]
    assert successor["predecessor"]["session_id"] == predecessor["session_id"]
    assert (
        registry.resolve("demo")["session_id"] == successor["successor"]["session_id"]
    )
    manifest = json.loads(
        Path(
            os.fsdecode(successor["successor"]["canonical_session"] + b"/session.json")
        ).read_bytes()
    )
    assert manifest["lineage"]["predecessor_session_id"] == predecessor["session_id"]
    assert manifest["geometry"]["cols"] == 66
    registry.register("failure", os.fsencode(rec.path))
    failed = Registry(os.fsencode(state), b"/bin/false")
    with pytest.raises(TermtraceError, match="recoverable orphan"):
        failed.restart("failure", timeout_ms=500)
    assert registry.resolve("failure")["session_id"] == predecessor["session_id"]
    assert before == canonical_hashes(rec.path)


@pytest.mark.asyncio
async def test_cancelled_async_instrumentation_remains_off_loop_and_is_not_rollback(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import asyncio
    import signal
    import threading

    from termtrace import types

    root = evidence_root(tmp_path)
    root.chmod(0o700)
    client = Termtrace(binary=BINARY, evidence_root=str(root / "sessions"))
    rec = client.launch(["/bin/sh", "-c", "printf READY; read line"])
    pid = None
    worker = None
    try:
        rec.output().wait_for(b"READY", 5)
        entry = client.registry(os.fsencode(root / "state")).register(
            "live", os.fsencode(rec.path)
        )
        pid = entry["recorder_pid"]
        assert pid is not None
        emitter = rec.instrumentation()
        entered = threading.Event()
        main_thread = threading.get_ident()

        class EnteredNativeCall:
            def event(
                self, name: str, attributes: dict[str, types.AnyValue] | None = None
            ) -> types.ApplicationAcknowledgement:
                assert threading.get_ident() != main_thread
                entered.set()
                return emitter.event(name, attributes)

        wrapped = aio.AsyncInstrumentation._wrap(emitter)
        monkeypatch.setattr(wrapped, "_sync", EnteredNativeCall())
        os.kill(pid, signal.SIGSTOP)
        worker = asyncio.create_task(wrapped.event("cancelled-await"))
        assert await asyncio.to_thread(entered.wait, 1)
        worker.cancel()
        with pytest.raises(asyncio.CancelledError):
            await worker
        os.kill(pid, signal.SIGCONT)
        pid = None
        deadline = asyncio.get_running_loop().time() + 4
        while True:
            page = await asyncio.to_thread(
                rec.events().page, None, 1000, False, ["application"]
            )
            matches = [
                item
                for item in page["events"]
                if item["event"]["data"]["name"] == "cancelled-await"
            ]
            if matches:
                assert len(matches) == 1
                break
            assert asyncio.get_running_loop().time() < deadline
            await asyncio.sleep(0.01)
    finally:
        if pid is not None:
            os.kill(pid, signal.SIGCONT)
        if worker is not None and not worker.done():
            worker.cancel()
        if rec.is_active():
            rec.stop()


def test_hidden_state_save_requires_explicit_permission(tmp_path: Path) -> None:
    root = evidence_root(tmp_path)
    client = Termtrace(binary=BINARY, evidence_root=str(root / "sessions"))
    rec = client.launch(
        ["/bin/sh", "-c", "printf '\\033[?2026hHIDDEN\\033[?2026lVISIBLE'"]
    )
    try:
        rec.output().wait_for(b"VISIBLE", 5)
        rec.stop()
    finally:
        if rec.is_active():
            rec.stop()
    hidden = next(
        entry
        for entry in rec.analysis().timeline(limit=1000, include_hidden=True)["entries"]
        if entry["item"]["kind"] == "semantic_state" and not entry["item"]["presented"]
    )
    point = {"selector": "state_id", "state_id": hidden["item"]["state_id"]}
    with pytest.raises(TermtraceError):
        rec.evidence().save(os.fsencode(root / "not-published"), ["txt"], point)
    assert not (root / "not-published").exists()
    assert (
        rec.evidence().save(os.fsencode(root / "hidden"), ["txt"], point, True)[
            "manifest"
        ]["state_point"]
        == point
    )


@pytest.mark.parametrize("asynchronous", [False, True])
@pytest.mark.asyncio
async def test_live_restart_failure_is_management_not_an_actor_retry(
    tmp_path: Path, asynchronous: bool
) -> None:
    from termtrace import Registry

    root = evidence_root(tmp_path)
    client = Termtrace(binary=BINARY, evidence_root=str(root / "sessions"))
    rec = client.launch(
        ["/bin/sh", "-c", "printf LIVE; read line"], {"environment": {"base": "clear"}}
    )
    registry = Registry(os.fsencode(root / "registry"), b"/bin/false")
    try:
        rec.output().wait_for(b"LIVE", 5)
        before = registry.register("live", os.fsencode(rec.path))
        operation = aio.AsyncRegistry._wrap(registry) if asynchronous else registry
        with pytest.raises(TermtraceError) as caught:
            if asynchronous:
                await operation.restart("live", timeout_ms=5000)
            else:
                operation.restart("live", timeout_ms=5000)
        error = caught.value
        assert error.scope == "management"
        assert error.effect is None and error.retry is None and error.request_id is None
        assert error.guidance == "inspect_no_automatic_retry"
        assert error.kind == "recorder_startup" and error.code == "recorder_startup"
        context = error.management
        assert context["operation"] == "registry_restart"
        facts = context["facts"]
        assert facts["phase"] == "successor_launched"
        assert facts["predecessor_path"] == os.fsencode(rec.path)
        assert facts["predecessor_completion"]["session_id"] == before["session_id"]
        assert facts["predecessor_completion"]["event_count"] > 0
        assert facts["successor"]["launched"]
        assert facts["successor"]["readiness"] == "exited"
        assert all(
            isinstance(facts["successor"][key], bytes)
            for key in ["session", "stdout_log", "stderr_log"]
        )
        assert registry.resolve("live")["session_id"] == before["session_id"]
        completed = client.registry(os.fsencode(root / "completed")).register(
            "done", os.fsencode(rec.path)
        )
        assert completed["recorder_process_start_ticks"] is None
    finally:
        rec.stop()


@pytest.mark.asyncio
async def test_async_restart_cas_failure_keeps_orphan_and_other_alias_owner(
    tmp_path: Path,
) -> None:
    import asyncio
    import shlex

    from termtrace import Registry

    root = evidence_root(tmp_path)
    client = Termtrace(binary=BINARY, evidence_root=str(root / "sessions"))
    rec = client.launch(["/bin/sh", "-c", "printf LIVE; read line"])
    other = client.launch(["/bin/true"])
    other.stop()
    state = os.fsencode(root / "registry")
    plain = Registry(state, os.fsencode(BINARY))
    entered, release, helper = root / "entered", root / "release", root / "helper"
    helper.write_text(
        f'#!/bin/sh\n: > {shlex.quote(str(entered))}\nwhile [ ! -e {shlex.quote(str(release))} ]; do sleep 0.01; done\nexec {shlex.quote(BINARY)} "$@"\n'
    )
    helper.chmod(0o700)
    wrapped = aio.AsyncRegistry._wrap(Registry(state, os.fsencode(helper)))
    pending = None
    try:
        rec.output().wait_for(b"LIVE", 5)
        old = plain.register("race", os.fsencode(rec.path))
        pending = asyncio.create_task(
            wrapped.restart(
                "race",
                {"command": [b"/bin/true"], "allow_environment_change": True},
                {"base": "clear"},
                5000,
            )
        )
        for _ in range(500):
            if entered.exists():
                break
            await asyncio.sleep(0.01)
        assert entered.exists()
        plain.remove("race")
        winner = plain.register("race", os.fsencode(other.path))
        release.touch()
        with pytest.raises(TermtraceError) as caught:
            await pending
        error = caught.value
        assert (
            error.scope == "management" and error.retry is None and error.effect is None
        )
        assert error.kind == "engine"
        facts = error.management["facts"]
        assert facts["phase"] == "alias_retarget"
        assert facts["predecessor_completion"]["session_id"] == old["session_id"]
        assert facts["successor"]["readiness"] in ["live", "completed"]
        assert (
            Path(os.fsdecode(facts["successor"]["session"]))
            .joinpath("session.json")
            .exists()
        )
        assert plain.resolve("race")["session_id"] == winner["session_id"]
    finally:
        release.touch()
        if pending is not None and not pending.done():
            try:
                await pending
            except TermtraceError:
                pass
        rec.stop()
        other.stop()


def test_nonutf8_source_debug_forensic_verification_paths_and_source_hashes(
    tmp_path: Path,
) -> None:
    root = evidence_root(tmp_path)
    client = Termtrace(binary=BINARY, evidence_root=str(root / "sessions"))
    rec = client.launch(["/bin/sh", "-c", "printf BYTEBOUND"])
    rec.stop()
    other = client.launch(["/bin/true"])
    other.stop()
    before = canonical_hashes(rec.path)
    source = os.fsencode(root) + b"/source-\xff"
    os.rename(os.fsencode(rec.path), source)
    source = os.path.realpath(source)
    assert canonical_hashes(os.fsdecode(source)) == before
    api = client.evidence(source)
    native = api.verify("content")
    assert native["valid"] and os.fsencode(native["session"]) == source
    profiles = []
    for profile in ["debug", "forensic"]:
        destination = os.fsencode(root) + b"/bundle-\xfe-" + profile.encode()
        report = api.bundle(destination, profile)
        assert api.verify_bundle(destination, profile) == report
        with pytest.raises(TermtraceError):
            client.evidence(os.fsencode(other.path)).verify_bundle(destination, profile)
        verification = destination + b"/verification.json"
        encoded = Path(os.fsdecode(verification)).read_bytes()
        value = json.loads(encoded)
        assert bytes(value["session"]) == source
        assert value["valid"] and value["session_id"] == native["session_id"]
        Path(os.fsdecode(verification)).write_bytes(encoded + b"TAMPER")
        with pytest.raises(TermtraceError):
            api.verify_bundle(destination, profile)
        Path(os.fsdecode(verification)).write_bytes(encoded)
        assert api.verify_bundle(destination, profile) == report
        assert canonical_hashes(os.fsdecode(source)) == before
        profiles.append(profile)
    (root / "verification-path-observed.json").write_text(
        json.dumps(
            {
                "source_base64": base64.b64encode(source).decode(),
                "profiles": profiles,
                "source_hashes_before": before,
                "source_hashes_after": canonical_hashes(os.fsdecode(source)),
            },
            indent=2,
        )
    )


def ffmpeg_without_vp9(directory: Path) -> Path:
    directory.mkdir()
    real = subprocess.run(["sh", "-c", "command -v ffmpeg"], check=True, capture_output=True, text=True)
    wrapper = directory / "ffmpeg"
    wrapper.write_text(
        "#!/bin/sh\n"
        "case \" $* \" in *libvpx-vp9*) echo 'Unknown encoder libvpx-vp9' >&2; exit 1;; esac\n"
        f"exec '{real.stdout.strip()}' \"$@\"\n"
    )
    wrapper.chmod(0o700)
    return directory


def test_delivery_codec_availability_follows_the_ffmpeg_on_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = Termtrace(binary=BINARY, evidence_root=str(evidence_root(tmp_path) / "sessions"))
    with client.record(["/bin/sh", "-c", "printf READY; IFS= read -r line"], rows=6, cols=30) as rec:
        rec.screen().wait_for_text("READY", timeout_secs=5)
        rec.mark("start")
        rec.mark("end")
        rec.stop()
    edit = Edit().clip(Select.marker("start"), Select.marker("end"))

    def capabilities() -> dict[str, dict]:
        report = client.system().capabilities()
        return {item["id"]: item for item in report["capabilities"]}

    host = capabilities()
    assert host["gif-delivery"]["status"] == "ready"
    assert host["webm-delivery"]["status"] == "ready"

    tools = ffmpeg_without_vp9(tmp_path / "tools")
    monkeypatch.setenv("PATH", f"{tools}{os.pathsep}{os.environ['PATH']}")
    limited = capabilities()
    assert limited["gif-delivery"]["status"] == "ready"
    assert limited["webm-delivery"]["status"] == "blocked"
    assert limited["webm-delivery"]["code"] == "derivative_encoder_unavailable"
    assert limited["webm-delivery"]["remediation"]

    out = tmp_path / "blocked.webm"
    with pytest.raises(TermtraceError, match="webm-delivery") as blocked:
        client.video_render(rec.path, edit, out, renderer="semantic")
    assert "derivative_encoder_unavailable" in str(blocked.value)
    assert not out.exists()
    gif = client.video_render(rec.path, edit, tmp_path / "allowed.gif", renderer="semantic")
    assert Path(gif["output"]).is_file()
