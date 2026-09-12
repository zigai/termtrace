#!/usr/bin/env python3
"""Execute the complete acceptance contract, retaining measurements and failures."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import math
import os
from pathlib import Path
import platform
import pty
import re
import select
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
from collections.abc import Callable, Iterator

import msgspec

from acceptance_models import Contract, Gate, Result, Sample, distribution, latency_result

ROOT = Path(__file__).resolve().parent.parent


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def json_file(path: Path, value: object) -> None:
    path.write_bytes(msgspec.json.format(msgspec.json.encode(value), indent=2) + b"\n")


def object_json(data: bytes) -> dict[str, object]:
    value = msgspec.json.decode(data)
    if isinstance(value, list) and len(value) == 1:
        value = value[0]
    if not isinstance(value, dict):
        raise ValueError("expected one JSON object")
    return value


def text_command(command: list[str]) -> str:
    return subprocess.check_output(command, cwd=ROOT, stderr=subprocess.STDOUT,
                                   timeout=120, text=True).strip()


def source_identity() -> str:
    names = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT)
    identity = hashlib.sha256()
    for name in sorted(set(names.split(b"\0")) - {b""}):
        path = ROOT / os.fsdecode(name)
        if path.is_file():
            identity.update(name + b"\0" + bytes.fromhex(digest(path)))
    return identity.hexdigest()


def tree_rss(pid: int) -> int:
    try:
        status = Path(f"/proc/{pid}/status").read_text()
        match = re.search(r"^VmRSS:\s+(\d+) kB$", status, re.MULTILINE)
        own = int(match[1]) * 1024 if match else 0
        children = Path(f"/proc/{pid}/task/{pid}/children").read_text().split()
        return own + sum(tree_rss(int(child)) for child in children)
    except (FileNotFoundError, ProcessLookupError):
        return 0


class MemoryMonitor:
    def __init__(self, process: subprocess.Popen[bytes], limit: int) -> None:
        self.process = process
        self.limit = limit
        self.peak = 0
        self.exceeded = False
        self.done = threading.Event()
        self.thread = threading.Thread(target=self.watch, daemon=True)

    def watch(self) -> None:
        while not self.done.is_set():
            self.peak = max(self.peak, tree_rss(self.process.pid))
            if self.peak > self.limit:
                self.exceeded = True
                try:
                    os.killpg(self.process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                return
            self.done.wait(0.005)

    def __enter__(self) -> MemoryMonitor:
        self.thread.start()
        return self

    def __exit__(self, *_: object) -> None:
        self.done.set()
        self.thread.join()


def wait_process(process: subprocess.Popen[bytes], timeout: float) -> int:
    expired = threading.Event()

    def expire() -> None:
        expired.set()
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    timer = threading.Timer(timeout, expire)
    timer.start()
    try:
        code = process.wait()
    except BaseException:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        raise
    finally:
        timer.cancel()
        timer.join()
    if expired.is_set():
        raise TimeoutError(f"process exceeded {timeout}s deadline")
    return code


class AcceptanceRunner:
    def __init__(self, output: Path, reduced: bool, binary: Path | None,
                 allow_non_reference: bool | None = None) -> None:
        self.output = output
        self.reduced = reduced
        self.external_binary = binary
        self.binary = binary or ROOT / "target/release/termtrace"
        if allow_non_reference is not None:
            self.allow_non_reference = allow_non_reference
        else:
            allow_env = os.environ.get("TERMTRACE_BENCHMARK_ALLOW_NON_REFERENCE") in ("1", "true")
            strict_env = os.environ.get("TERMTRACE_BENCHMARK_STRICT") in ("1", "true")
            self.allow_non_reference = allow_env and not strict_env
        self.contract = msgspec.json.decode((ROOT / "benchmarks/acceptance-v1.json").read_bytes(), type=Contract)
        self.measurement = self.contract.measurement
        self.warmups = 1 if reduced else self.measurement.warmup_repetitions
        self.repetitions = 2 if reduced else self.measurement.measured_repetitions
        self.limit = self.measurement.resident_memory_limit_bytes
        self.timer = shutil.which("time")
        self.counter = 0
        self.results: list[Result] = []
        self.test_binaries: dict[str, str] = {}
        self.artifacts: dict[str, Path] = {}
        self.eligible = False
        self.env = dict(os.environ, TERM="xterm-256color", COLORTERM="truecolor", LC_ALL="C.UTF-8", TZ="UTC",
                        XDG_STATE_HOME=str(output / "state"), TERMTRACE_BINARY=str(self.binary))
        self.live_monitors: list[MemoryMonitor] = []
        self.profile = output / "ghostty.ghostty"
        profile = self.contract.defaults["profile"]
        if not isinstance(profile, dict) or not isinstance(profile.get("ghostty_config"), str):
            raise ValueError("missing Ghostty profile")
        self.profile.write_text(profile["ghostty_config"])

    def cli(self, *args: str | Path) -> list[str]:
        return [str(self.binary), "--response", "full", *(str(arg) for arg in args)]

    def run(self, command: list[str], *, timeout: float | None = None,
            check: bool = True, memory_limit: int | None = None) -> Sample:
        limit = memory_limit or self.limit
        if self.timer is None:
            raise RuntimeError("GNU time is required on PATH")
        self.counter += 1
        stem = self.output / f"command-{self.counter:06}"
        stdout = stem.with_suffix(".stdout")
        stderr = stem.with_suffix(".stderr")
        rss = stem.with_suffix(".rss")
        json_file(stem.with_suffix(".json"), {"argv": command, "started_unix_ns": time.time_ns()})
        started = time.monotonic_ns()
        with stdout.open("wb") as out, stderr.open("wb") as err:
            process = subprocess.Popen([self.timer, "-f", "%M", "-o", str(rss), *command],
                                       stdout=out, stderr=err, env=self.env, cwd=ROOT, start_new_session=True)
            with MemoryMonitor(process, limit) as memory:
                code = wait_process(process, timeout or self.measurement.timeout_seconds)
        elapsed = time.monotonic_ns() - started
        peaks = [memory.peak, *(monitor.peak for monitor in self.live_monitors)]
        if rss.exists():
            numeric = [line for line in rss.read_text().splitlines() if line.isdecimal()]
            if numeric:
                peaks.append(int(numeric[-1]) * 1024)
        peak = max(peaks) if peaks else memory.peak
        if memory.exceeded or peak > limit or any(m.exceeded for m in self.live_monitors):
            raise RuntimeError(f"{limit // (1024 * 1024)} MiB resident memory limit exceeded; evidence {stem}")
        if code and check:
            raise RuntimeError(f"exit {code}; evidence {stem}: {stderr.read_text(errors='replace')[-2000:]}")
        return Sample(elapsed, peak, stdout.stat().st_size, tuple(command), str(stdout))

    def payload(self, sample: Sample) -> dict[str, object]:
        return object_json(Path(sample.evidence).read_bytes())

    def prepare(self) -> None:
        missing = [tool for tool in ("cargo", "rustc", "git", "jq", "ffprobe", "ffmpeg", "ghostty")
                   if shutil.which(tool) is None]
        if missing:
            raise RuntimeError(f"missing acceptance dependencies: {', '.join(missing)}")
        if self.timer is None:
            raise RuntimeError("GNU time is required on PATH")
        timer_version = text_command([self.timer, "--version"]).splitlines()[0]
        if not timer_version.startswith("time (GNU Time)"):
            raise RuntimeError("acceptance resource measurements require GNU time")
        before = source_identity()
        revision = text_command(["git", "rev-parse", "HEAD"])
        build_env = dict(os.environ, TERMTRACE_BUILD_REVISION=f"{revision}+source.{before}")
        build_command = ["cargo", "build", "--locked", "--release", "--all-features", "--message-format=json"]
        build_command.extend(["--workspace", "--bins"] if self.external_binary is None
                             else ["-p", "termtrace-test-fixture", "--bin", "termtrace-fixture"])
        build = self.output / "build.jsonl"
        with build.open("wb") as out, build.with_suffix(".stderr").open("wb") as err:
            subprocess.run(build_command, cwd=ROOT, env=build_env, stdout=out, stderr=err, check=True)
        executables: dict[str, Path] = {}
        for line in build.read_bytes().splitlines():
            message = object_json(line)
            target = message.get("target")
            executable = message.get("executable")
            if isinstance(target, dict) and isinstance(executable, str):
                name = target.get("name")
                if isinstance(name, str):
                    executables[name] = Path(executable)
        self.generator = executables["termtrace-fixture"]
        if self.external_binary is None:
            self.binary = executables["termtrace"]
        self.env["TERMTRACE_BINARY"] = str(self.binary)
        for package, selector, target in (
            ("termtrace", "--test", "cli"),
            ("termtrace-evidence", "--lib", ""),
            ("termtrace-schema", "--lib", ""),
        ):
            command = ["cargo", "test", "--locked", "--release", "--all-features", "-p", package,
                       selector, *([target] if target else []), "--no-run", "--message-format=json"]
            build_log = self.output / f"build-{package}.jsonl"
            with build_log.open("wb") as out, build_log.with_suffix(".stderr").open("wb") as err:
                subprocess.run(command, cwd=ROOT, env=build_env, stdout=out, stderr=err, check=True)
            for line in build_log.read_bytes().splitlines():
                message = object_json(line)
                executable = message.get("executable")
                profile = message.get("profile")
                if (message.get("reason") == "compiler-artifact" and isinstance(executable, str)
                        and isinstance(profile, dict) and profile.get("test") is True):
                    self.test_binaries[package] = executable
            if package not in self.test_binaries:
                raise RuntimeError(f"no compiled behavioral scenario executable for {package}")
        after = source_identity()
        if before != after:
            raise RuntimeError("source changed during build; refusing ambiguous build provenance")
        version = self.payload(self.run(self.cli("version", "--json")))
        if self.external_binary is None and version.get("build_revision") != build_env["TERMTRACE_BUILD_REVISION"]:
            raise RuntimeError("binary build revision does not match this source identity")
        os_release = platform.freedesktop_os_release()
        cpu = next(line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                   if line.startswith("model name"))
        memory = next(int(line.split()[1]) * 1024 for line in Path("/proc/meminfo").read_text().splitlines()
                      if line.startswith("MemTotal:"))
        rust = text_command(["rustc", "--version", "--verbose"])
        rust_fields = dict(line.split(": ", 1) for line in rust.splitlines() if ": " in line)
        operating_system = os_release.get("PRETTY_NAME", "unknown")
        if os_release.get("BUILD_ID"):
            operating_system += f" {os_release['BUILD_ID']}"
        runtime = {"operating_system": operating_system,
                   "kernel": f"{platform.system()} {platform.release()}", "architecture": platform.machine(),
                   "cpu": cpu, "logical_cpus": os.cpu_count(), "memory_bytes": memory,
                   "rust_toolchain": f"{rust_fields.get('release')} ({rust_fields.get('commit-hash')})",
                   "ghostty_native": text_command(["ghostty", "--version"]).splitlines()[0].removeprefix("Ghostty "),
                   "ffmpeg": text_command(["ffmpeg", "-version"]).splitlines()[0].split()[2],
                   "jq": text_command(["jq", "--version"]).removeprefix("jq-")}
        expected = self.contract.environment
        mismatches = {key: {"expected": expected[key], "observed": value} for key, value in runtime.items()
                      if expected.get(key) != value}
        self.eligible = (not mismatches or self.allow_non_reference) and self.external_binary is None
        json_file(self.output / "run.json", {
            "format": "termtrace-acceptance-run", "version": 1, "reduced": self.reduced,
            "binary": {"path": str(self.binary.resolve()), "sha256": digest(self.binary),
                       "reported_identity": version, "profile": "release" if self.external_binary is None else "unverified-external",
                       "built_source_revision": revision if self.external_binary is None else None,
                       "built_source_sha256": after if self.external_binary is None else None,
                       "source_dirty": bool(text_command(["git", "status", "--porcelain"])),
                       "build_command": build_command if self.external_binary is None else None},
            "runtime": runtime, "python": sys.version, "rust_verbose": rust,
            "build_environment": {key: build_env.get(key) for key in (
                "TERMTRACE_BUILD_REVISION", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC",
                "RUSTC_WRAPPER", "CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTFLAGS")},
            "clock_runtime": {"implementation": time.get_clock_info("monotonic").implementation,
                              "resolution_seconds": time.get_clock_info("monotonic").resolution},
            "resource_timer": {"path": self.timer, "sha256": digest(Path(self.timer)),
                               "version": timer_version},
            "invariant_binaries": {package: {"path": path, "sha256": digest(Path(path))}
                                   for package, path in self.test_binaries.items()},
            "host_eligible": self.eligible, "host_mismatches": mismatches,
            "allow_non_reference": self.allow_non_reference,
            "measurement": self.measurement, "actual_warmups": self.warmups,
            "actual_repetitions": self.repetitions, "concurrency": 1,
            "resource_accounting": "5ms /proc descendant RSS samples plus GNU time maximum RSS; 512MiB runtime tree cap; compilation excluded",
            "cache_identity": {"host_page_cache": "retained", "process": "fresh CLI for every sample; same actor and fixture",
                               "artifact_directories": "fresh per screenshot", "warmups": self.warmups},
            "fixture_spec_sha256": digest(ROOT / "benchmarks/acceptance-v1.json"),
            "workload_sha256": digest(ROOT / "benchmarks/workload.py"),
            "ghostty_profile_sha256": digest(self.profile),
        })

    def recording_args(self) -> list[str]:
        return ["--cols", "120", "--rows", "40", "--cell-width", "10", "--cell-height", "20",
                "--term", "xterm-256color", "--ghostty-config", str(self.profile),
                "--durability", "bounded", "--durability-max-interval-ms", "50",
                "--durability-max-uncommitted-bytes", "65536"]

    @contextmanager
    def live(self, name: str) -> Iterator[tuple[Path, Path]]:
        session = self.output / f"{name}.termtrace"
        control = self.output / f"{name}.fifo"
        os.mkfifo(control, 0o600)
        fixture = next(f for f in self.contract.fixtures if f.id == "fullscreen-query-heavy")
        workload = [argument.replace("CONTROL_FIFO", str(control)) for argument in fixture.command]
        command = self.cli("record", "--no-input", *self.recording_args(), "--out", session, "--", *workload)
        with (self.output / f"{name}.stdout").open("wb") as out, (self.output / f"{name}.stderr").open("wb") as err:
            process = subprocess.Popen(command, cwd=ROOT, stdout=out, stderr=err, env=self.env, start_new_session=True)
            with MemoryMonitor(process, self.limit) as memory:
                self.live_monitors.append(memory)
                try:
                    deadline = time.monotonic() + 30
                    while not (session / "session.json").is_file():
                        if process.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError(f"recorder failed to publish manifest: {name}")
                        time.sleep(0.01)
                    manifest = object_json((session / "session.json").read_bytes())
                    socket_name = manifest.get("control_socket")
                    if not isinstance(socket_name, str):
                        raise ValueError("recording manifest has no control socket")
                    while f"@{socket_name}" not in Path("/proc/net/unix").read_text():
                        if process.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError(f"recorder failed to bind control socket: {name}")
                        time.sleep(0.01)
                    self.run(self.cli("wait", session, "--screen", "READY", "--timeout", "30s"))
                    yield session, control
                finally:
                    try:
                        if process.poll() is None:
                            self.run(self.cli("stop", session, "--wait"))
                            process.wait(timeout=30)
                    finally:
                        if process.poll() is None:
                            os.killpg(process.pid, signal.SIGKILL)
                            process.wait()
                        self.live_monitors.remove(memory)
                        if (session / "completion.json").is_file():
                            json_file(self.output / f"{name}-fixture.json", {
                                "session": str(session), "command": command,
                                "event_log_sha256": digest(session / "events.jsonl"),
                                "session_sha256": digest(session / "session.json")})

    def repeated(self, operation: Callable[[int], Sample]) -> list[Sample]:
        samples = []
        for iteration in range(self.warmups + self.repetitions):
            sample = operation(iteration)
            if iteration >= self.warmups:
                samples.append(sample)
        return samples

    def emit(self, result: Result) -> None:
        self.results.append(result)
        with (self.output / "results.jsonl").open("ab") as stream:
            stream.write(msgspec.json.encode(result) + b"\n")
        print(f"{result.id}: {result.status}", flush=True)

    def attempt(self, identifier: str, kind: str, operation: Callable[[], Result]) -> None:
        try:
            self.emit(operation())
        except (OSError, ValueError, RuntimeError, LookupError, TimeoutError, subprocess.SubprocessError) as e:
            self.emit(Result(identifier, kind, "failed", error=str(e)))

    def latency(self, gate: Gate, operation: Callable[[int], Sample]) -> Result:
        return latency_result(gate, self.repeated(operation), self.eligible, self.reduced, self.limit)

    def notification(self, session: Path, iteration: int) -> Sample:
        # Sample the actor/system mapping, never substitute command elapsed time for actor delay.
        status = self.payload(self.run(self.cli("status", session)))
        clock = status.get("clock_sample")
        if not isinstance(clock, dict):
            raise RuntimeError("actor clock calibration unavailable: notification delta is inconclusive")
        origin = int(clock["system_monotonic_ns"]) - int(clock["session_monotonic_ns"])
        uncertainty = int(clock["uncertainty_ns"])
        marker = f"INPUT-{iteration + 1}"
        self.counter += 1
        target = self.output / f"wait-{self.counter}.stdout"
        error = target.with_suffix(".stderr")
        command = self.cli("wait", session, "--screen", marker, "--timeout", "30s")
        receipt: list[tuple[int, bytes]] = []
        with error.open("wb") as err:
            process = subprocess.Popen(command, cwd=ROOT, env=self.env, stdout=subprocess.PIPE,
                                       stderr=err, start_new_session=True)
            if process.stdout is None:
                raise RuntimeError("wait notification pipe is unavailable")
            pipe = process.stdout

            def receive() -> None:
                data = pipe.readline()
                receipt.append((time.monotonic_ns(), data))

            reader = threading.Thread(target=receive, daemon=True)
            reader.start()
            with MemoryMonitor(process, self.limit) as monitor:
                try:
                    self.run(self.cli("send", session, "--text", "x\n"))
                    code = wait_process(process, 30)
                finally:
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                    reader.join(timeout=5)
                    pipe.close()
        if not receipt:
            raise RuntimeError("wait returned without delivering its actor notification")
        received, data = receipt[0]
        target.write_bytes(data)
        if code or monitor.exceeded:
            raise RuntimeError(f"wait notification failed: {error}")
        result = object_json(target.read_bytes())
        matched = result.get("first_match_monotonic_ns")
        if not isinstance(matched, int) or result.get("matched") is not True:
            raise ValueError("wait did not return matching actor processing timestamp")
        delta = received - origin - matched
        if delta < -uncertainty:
            raise ValueError("actor calibration inconsistent with CLOCK_MONOTONIC")
        return Sample(max(delta, 0), monitor.peak, target.stat().st_size, tuple(command), str(target), uncertainty)

    def screenshot(self, session: Path, renderer: str, iteration: int) -> Sample:
        target = self.output / f"{renderer}-screenshot-{iteration}"
        sample = self.run(self.cli("screenshot", session, "--renderer", renderer, "--out", target))
        pngs = sorted(target.rglob("*.png"))
        if not pngs:
            raise RuntimeError(f"screenshot produced no PNG: {target}")
        self.artifacts["png"] = pngs[0]
        if renderer == "native":
            self.artifacts["native-capture"] = target
        return sample

    def reviewer(self, session: Path, iteration: int) -> Sample:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 1600, 800))
        target = self.output / f"reviewer-{iteration}.terminal"
        command = self.cli("analyze", "review", session)
        started = time.monotonic_ns()
        process = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave,
                                   cwd=ROOT, env=self.env, start_new_session=True)
        os.close(slave)
        data = bytearray()
        try:
            with MemoryMonitor(process, self.limit) as memory:
                deadline = time.monotonic() + self.measurement.timeout_seconds
                while True:
                    if time.monotonic() >= deadline:
                        raise TimeoutError("reviewer did not present initial viewport")
                    readable, _, _ = select.select([master], [], [], 0.1)
                    if readable:
                        chunk = os.read(master, 65536)
                        if not chunk:
                            raise RuntimeError("reviewer exited before initial draw")
                        data.extend(chunk)
                        plain = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(data))
                        if b"? help" in plain and b"semantic viewport" in plain:
                            elapsed = time.monotonic_ns() - started
                            os.write(master, b"q")
                            if process.wait(timeout=10):
                                raise RuntimeError("reviewer failed after initial draw")
                            break
                    elif process.poll() is not None:
                        raise RuntimeError("reviewer exited before initial draw")
                if memory.exceeded:
                    raise RuntimeError("reviewer exceeded resident memory cap")
        finally:
            target.write_bytes(data)
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            os.close(master)
        return Sample(elapsed, memory.peak, len(data), tuple(command), str(target))

    def workflow_gate(self, gate: Gate) -> Result:
        name = f"acceptance-{os.getpid()}"
        scenario = ROOT / "skills/termtrace/examples/ready.termtrace.json"
        commands = {
            "start": self.cli("start", name, *self.recording_args(), "--until-screen", "READY", "--", "/bin/sh", "-c", "printf READY; cat"),
            "act": self.cli("act", name, "text:x", "enter"),
            "send": self.cli("send", name, "--text", "x\n"),
            "wait": self.cli("wait", name, "--screen", "READY"),
            "screen": self.cli("screen", name, "--screen", "text"),
            "control resize": self.cli("control", "resize", name, "--cols", "120", "--rows", "40", "--pixel-width", "1200", "--pixel-height", "800"),
            "restart": self.cli("restart", name),
            "stop": self.cli("stop", name, "--wait"),
            "screenshot": self.cli("screenshot", name, "--renderer", "semantic", "--out", self.output / "workflow-screenshot"),
            "test": self.cli("test", scenario, "--artifacts", self.output / "workflow-test"),
            "video": self.cli("video", name, "--renderer", "semantic", "--out", self.output / "workflow.mp4"),
        }
        results: dict[str, object] = {}
        samples = []
        try:
            for workflow in gate.workflows:
                command = commands[workflow]
                try:
                    sample = self.run(command)
                    samples.append(sample)
                    # One actual invocation and one domain selector; artifact destinations are not selectors.
                    results[workflow] = {"commands": 1, "required_identifiers": 1, "argv": command,
                                         "stdout_bytes": sample.stdout_bytes, "evidence": sample.evidence,
                                         "passed": True}
                except (OSError, ValueError, RuntimeError, TimeoutError) as e:
                    results[workflow] = {"commands": 1, "required_identifiers": 1, "argv": command,
                                         "passed": False, "error": str(e)}
        finally:
            self.run(self.cli("stop", name, "--wait"), check=False)
        if len(results) != 11 or set(results) != set(gate.workflows):
            raise ValueError("ergonomics coverage is incomplete")
        self.artifacts["video"] = self.output / "workflow.mp4"
        passed = (gate.maximum_commands_per_workflow == 1 and gate.maximum_required_identifiers_per_workflow == 1
                  and all(isinstance(value, dict) and value.get("passed") is True for value in results.values()))
        return Result(gate.id, gate.kind, "passed" if passed else "failed", tuple(samples), details=results)

    def large_fixture(self) -> Path:
        fixture = next(f for f in self.contract.fixtures if f.id == "million-event-1gib")
        session = self.output / "large-completed.termtrace"
        events = 1000 if self.reduced else fixture.event_count
        output_bytes = 1_048_576 if self.reduced else fixture.output_bytes
        generator = self.generator
        self.run([str(generator), "large", "--events", str(events), "--output-bytes", str(output_bytes), "--out", str(session)], timeout=7200)
        json_file(self.output / "large-fixture.json", {"path": str(session), "event_count": events,
                  "output_bytes": output_bytes, "generator_sha256": digest(generator),
                  "event_log_sha256": digest(session / "events.jsonl"), "session_sha256": digest(session / "session.json")})
        self.artifacts["evidence"] = session
        return session

    @staticmethod
    def log_position(session: Path) -> int:
        with (session / "events.jsonl").open("rb") as stream:
            stream.seek(0, os.SEEK_END)
            size = stream.tell()
            stream.seek(max(0, size - 131072))
            lines = stream.read().split(b"\n")
        for line in reversed(lines[1:-1]):
            value = object_json(line)
            seq = value.get("seq")
            if isinstance(seq, int):
                return seq
        return 0

    def grow(self, session: Path, control: Path, target_events: int, target_bytes: int) -> int:
        ack = control.with_suffix(".ack")
        produced = 0
        deadline = time.monotonic() + 7200
        while self.log_position(session) < target_events or produced < target_bytes:
            if time.monotonic() > deadline:
                raise TimeoutError("large-live fixture growth exceeded two hours")
            chunks = max(1, min(8192, math.ceil((target_bytes - produced) / 4096),
                                max(1, target_events - self.log_position(session))))
            ack.unlink(missing_ok=True)
            with control.open("w") as stream:
                stream.write(f"{chunks}\n")
            while not ack.exists():
                if any(m.exceeded or m.process.poll() is not None for m in self.live_monitors):
                    raise RuntimeError("large-live recorder exited or exceeded resident memory cap")
                if time.monotonic() > deadline:
                    raise TimeoutError("large-live producer did not acknowledge growth")
                time.sleep(0.01)
            produced += chunks * 4096
        self.run(self.cli("wait", session, "--screen", "READY", "--timeout", "30s"))
        return produced

    def follow(self, session: Path, control: Path, iteration: int) -> Sample:
        self.counter += 1
        target = self.output / f"follow-{self.counter}.jsonl"
        status = self.payload(self.run(self.cli("status", session)))
        boundary = status.get("boundary")
        boundaries = status.get("boundaries")
        cursor = None
        if isinstance(boundaries, dict) and isinstance(boundaries.get("durable"), dict):
            cursor = boundaries["durable"].get("next_event_seq")
        elif isinstance(boundary, dict):
            if isinstance(boundary.get("next_event_seq"), int):
                cursor = boundary["next_event_seq"]
            elif isinstance(boundary.get("durable_boundary"), dict):
                cursor = boundary["durable_boundary"].get("next_event_seq")
        if not isinstance(cursor, int):
            raise ValueError(f"live status did not return its durable event cursor: {status}")
        command = self.cli("analyze", "events", session, "--follow", "--metadata-only",
                           "--from-event-seq", str(cursor), "--limit", "1")
        started = time.monotonic_ns()
        with target.open("wb") as out:
            process = subprocess.Popen(command, cwd=ROOT, env=self.env, stdout=out, stderr=subprocess.DEVNULL,
                                       start_new_session=True)
            with MemoryMonitor(process, self.limit) as monitor:
                try:
                    self.run(self.cli("control", "mark", session, "--label", f"follow-{iteration}"))
                    deadline = time.monotonic() + self.measurement.timeout_seconds
                    while True:
                        lines = target.read_bytes().split(b"\n")[:-1]
                        # Require a real event, not a stream header/page acknowledgement.
                        if any(isinstance(object_json(line).get("seq"), int) for line in lines):
                            elapsed = time.monotonic_ns() - started
                            break
                        if process.poll() is not None or time.monotonic() >= deadline:
                            raise RuntimeError(f"follow failed to deliver the new durable event: {target}")
                        time.sleep(0.005)
                finally:
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=10)
        if monitor.exceeded:
            raise RuntimeError("follow exceeded resident memory limit")
        return Sample(elapsed, monitor.peak, target.stat().st_size, tuple(command), str(target))

    def complexity(self, gate: Gate) -> Result:
        points: list[dict[str, object]] = []
        metrics: dict[str, list[tuple[int, list[Sample]]]] = {op: [] for op in gate.operations}
        with self.live("large-live") as (session, control):
            total_produced = 0
            for scale in (0.01, 0.1, 1.0):
                events = int((1000 if self.reduced else 1_000_000) * scale)
                size = int((1_048_576 if self.reduced else 1_073_741_824) * scale)
                total_produced += self.grow(session, control, events, max(0, size - total_produced))
                length = self.log_position(session)
                operations = {"screen": lambda i: self.run(self.cli("screen", session, "--screen", "text")),
                              "wait": lambda i: self.run(self.cli("wait", session, "--screen", "READY")),
                              "analyze events --follow": lambda i: self.follow(session, control, i)}
                for op in gate.operations:
                    samples = self.repeated(operations[op])
                    metrics[op].append((length, samples))
                    points.append({"operation": op, "event_count": length, "generated_output_bytes": total_produced,
                                   "log_bytes": (session / "events.jsonl").stat().st_size,
                                   "distribution": distribution(samples), "samples": samples})
        trends: dict[str, object] = {}
        passed = True
        for op, observations in metrics.items():
            if len({length for length, _ in observations}) < 3:
                raise ValueError("complexity requires at least three distinct canonical log lengths")
            for metric in ("elapsed_ns", "maximum_rss_bytes"):
                # O(1) must not exhibit a statistically significant positive linear slope.
                xs = [float(length) for length, samples in observations for _ in samples]
                ys = [float(getattr(sample, metric)) for _, samples in observations for sample in samples]
                mean_x, mean_y = sum(xs) / len(xs), sum(ys) / len(ys)
                sxx = sum((x - mean_x) ** 2 for x in xs)
                slope = sum((x - mean_x) * (y - mean_y) for x, y in zip(xs, ys, strict=True)) / sxx
                residual = sum((y - mean_y - slope * (x - mean_x)) ** 2 for x, y in zip(xs, ys, strict=True))
                stderr = math.sqrt(residual / max(1, len(xs) - 2) / sxx)
                # 99% two-sided Student-t bound is conservative at the complete-run n=90.
                lower = slope - 2.64 * stderr
                threshold = 1.0
                independent = lower <= threshold
                passed &= independent
                trends[f"{op}:{metric}"] = {"slope_per_event": slope, "standard_error": stderr,
                                            "lower_99_percent_bound": lower, "independent": independent}
        return Result(gate.id, gate.kind, "failed" if not passed else "reduced" if self.reduced else "passed",
                      details={"points": points, "linear_growth_tests": trends,
                               "criterion": "no positive linear growth at 99% confidence across three real live lengths"})

    def invariant(self, name: str) -> Result:
        scenarios = {
            "canonical_integrity": "verify_reports_manifest_binding_and_detects_tampering",
            "presentation_scope": "show_requires_explicit_permission_for_hidden_states",
            "artifact_provenance": "save_atomically_publishes_one_state_with_manifested_files",
            "mutation_retry_exactly_once": "oversized_action_projection_is_cached_without_replaying_input",
            "crash_outcome_classification": "recovers_valid_prefix_and_reports_uncertain_intent",
            "geometry_exactness": "start_act_and_screen_form_one_state_safe_workflow",
            "schema_consistency": "checked_in_schemas_match_catalog",
            "truth_layer_labeling": "action_termination_frame_remains_live_actor_evidence",
        }
        package = {"crash_outcome_classification": "termtrace-evidence",
                   "schema_consistency": "termtrace-schema"}.get(name, "termtrace")
        binary = self.test_binaries[package]
        listing = text_command([binary, "--list"])
        matches = [line.removesuffix(": test") for line in listing.splitlines()
                   if line.endswith(f"::{scenarios[name]}: test")]
        if len(matches) != 1:
            raise RuntimeError(f"invariant must resolve to one real behavioral scenario: {scenarios[name]}")
        sample = self.run([binary, "--exact", matches[0], "--test-threads=1", "--nocapture"], timeout=1800)
        output = Path(sample.evidence).read_text()
        if not re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", output):
            raise RuntimeError("invariant scenario did not execute exactly one passing behavioral test")
        return Result(name, "invariant", "passed", (sample,), details={"scenario": scenarios[name]})

    def validator(self, artifact: str, template: str) -> Result:
        if artifact == "native-capture":
            target = self.output / "native-validation.termtrace"
            command = shlex.split(template.replace("OUTPUT_DIRECTORY", shlex.quote(str(target))))
        else:
            path = self.artifacts[artifact]
            command = shlex.split(template.replace("ARTIFACT", shlex.quote(str(path))).replace("SESSION", shlex.quote(str(path))))
            if command[0] == "termtrace":
                command = self.cli(*command[1:])
        sample = self.run(command, timeout=600,
                          memory_limit=self.limit * 2 if artifact == "native-capture" else None)
        if artifact in ("png", "video"):
            streams = self.payload(sample).get("streams")
            if not isinstance(streams, list) or not streams:
                raise ValueError("ffprobe found no media streams")
            if not any(isinstance(stream, dict) and int(stream.get("width", 0)) > 0
                       and int(stream.get("height", 0)) > 0 for stream in streams):
                raise ValueError("media has no positive geometry")
        return Result(f"validator:{artifact}", "artifact_validator", "passed", (sample,))

    def execute(self) -> bool:
        self.prepare()
        line_fixture = next(f for f in self.contract.fixtures if f.id == "line-oriented")
        line_session = self.output / "line-oriented.termtrace"
        self.run(self.cli("record", "--no-input", *self.recording_args(), "--out", line_session,
                          "--", *line_fixture.command))
        line_output = self.run(self.cli("analyze", "output", line_session, "--raw"))
        if line_fixture.completion_marker.encode() not in Path(line_output.evidence).read_bytes():
            raise RuntimeError("line-oriented fixture did not reach its declared completion marker")
        json_file(self.output / "line-fixture.json", {
            "session": str(line_session), "event_log_sha256": digest(line_session / "events.jsonl"),
            "session_sha256": digest(line_session / "session.json")})
        gates = {gate.id: gate for gate in self.contract.gates}
        ergonomic = gates.pop("cli-command-count-parity")
        self.attempt(ergonomic.id, ergonomic.kind, lambda: self.workflow_gate(ergonomic))
        live_ids = ("warm-status-p95", "warm-live-show-p95", "input-ack-p95", "presented-wait-notification-p95")
        try:
            with self.live("query-heavy") as (session, _):
                operations = {"warm-status-p95": lambda i: self.run(self.cli("status", session)),
                              "warm-live-show-p95": lambda i: self.run(self.cli("screen", session, "--screen", "text")),
                              "input-ack-p95": lambda i: self.run(self.cli("send", session, "--text", "x")),
                              "presented-wait-notification-p95": lambda i: self.notification(session, i)}
                for identifier in live_ids:
                    gate = gates.pop(identifier)
                    self.attempt(identifier, gate.kind, lambda g=gate: self.latency(g, operations[g.id]))
            for renderer in ("semantic", "native"):
                gate = gates.pop(f"{renderer}-screenshot-p95")
                self.attempt(gate.id, gate.kind, lambda g=gate, r=renderer: self.latency(g, lambda i: self.screenshot(session, r, i)))
            self.artifacts["json"] = session / "session.json"
            self.artifacts["jsonl"] = session / "events.jsonl"
        except (OSError, ValueError, RuntimeError, TimeoutError) as e:
            for identifier in (*live_ids, "semantic-screenshot-p95", "native-screenshot-p95"):
                if identifier in gates:
                    gate = gates.pop(identifier)
                    self.emit(Result(identifier, gate.kind, "failed", error=str(e)))
        gate = gates.pop("large-live-independence")
        self.attempt(gate.id, gate.kind, lambda: self.complexity(gate))
        try:
            large = self.large_fixture()
            for identifier, operation in (
                ("large-completed-show-p95", lambda i: self.run(self.cli("screen", large, "--screen", "text"))),
                ("large-reviewer-range-load-p95", lambda i: self.reviewer(large, i)),
            ):
                gate = gates.pop(identifier)
                self.attempt(identifier, gate.kind, lambda g=gate, op=operation: self.latency(g, op))
        except (OSError, ValueError, RuntimeError, TimeoutError) as e:
            for identifier in ("large-completed-show-p95", "large-reviewer-range-load-p95"):
                if identifier in gates:
                    gate = gates.pop(identifier)
                    self.emit(Result(identifier, gate.kind, "failed", error=str(e)))
        for gate in gates.values():
            self.emit(Result(gate.id, gate.kind, "failed", error="no executor registered for declared gate"))
        for name in self.contract.invariants:
            self.attempt(name, "invariant", lambda n=name: self.invariant(n))
        for validator in self.contract.artifact_validators:
            self.attempt(f"validator:{validator.artifact}", "artifact_validator",
                         lambda v=validator: self.validator(v.artifact, v.command))
        passed = all(result.status == "passed" for result in self.results) and self.eligible and not self.reduced
        json_file(self.output / "summary.json", {"accepted": passed, "reduced": self.reduced,
                  "host_eligible": self.eligible, "allow_non_reference": self.allow_non_reference,
                  "results": self.results,
                  "expected_gate_ids": [gate.id for gate in self.contract.gates]})
        return passed

    def fail_remaining(self, error: str) -> None:
        present = {result.id for result in self.results}
        required = [(gate.id, gate.kind) for gate in self.contract.gates]
        required.extend((name, "invariant") for name in self.contract.invariants)
        required.extend((f"validator:{validator.artifact}", "artifact_validator")
                        for validator in self.contract.artifact_validators)
        for identifier, kind in required:
            if identifier not in present:
                self.emit(Result(identifier, kind, "failed", error=error))
        json_file(self.output / "summary.json", {"accepted": False, "reduced": self.reduced,
                  "host_eligible": self.eligible, "allow_non_reference": self.allow_non_reference,
                  "results": self.results})


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--reduced", action="store_true", help="reduced mechanics only; never an acceptance pass")
    parser.add_argument("--binary", type=Path, help="external binary; provenance is unverified and cannot pass acceptance")
    parser.add_argument("--allow-non-reference", action="store_true", default=None,
                        help="allow host gating to pass on non-reference environments")
    parser.add_argument("--strict", action="store_true", default=None,
                        help="enforce strict reference host checking, overriding allow-non-reference")
    args = parser.parse_args()
    os.umask(0o077)
    output = args.output
    if output is None:
        parent = ROOT / "benchmarks/results"
        parent.mkdir(parents=True, exist_ok=True)
        output = Path(tempfile.mkdtemp(prefix="acceptance-", dir=parent))
    else:
        output = output.resolve()
        output.mkdir(parents=True, exist_ok=False)
    print(f"acceptance evidence: {output}", flush=True)
    runner = None
    allow_non_reference: bool | None = None
    if args.strict:
        allow_non_reference = False
    elif args.allow_non_reference:
        allow_non_reference = True
    try:
        runner = AcceptanceRunner(output, args.reduced, args.binary.resolve() if args.binary else None,
                                  allow_non_reference=allow_non_reference)
        accepted = runner.execute()
    except (OSError, ValueError, RuntimeError, LookupError, subprocess.SubprocessError) as e:
        json_file(output / "failure.json", {"accepted": False, "error": str(e)})
        if runner is not None:
            runner.fail_remaining(str(e))
        print(str(e), file=sys.stderr)
        return 1
    return 0 if accepted else 1


if __name__ == "__main__":
    raise SystemExit(main())
