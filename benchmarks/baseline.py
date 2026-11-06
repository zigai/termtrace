#!/usr/bin/env python3
"""Deterministic baseline performance benchmark suite for Termtrace.

Measures all critical hit paths:
1. PTY byte ingestion & durability modes (strict, bounded, throughput)
2. Semantic rendering (SVG export, PNG rasterization, video export)
3. Replay engine & timeline forensics (event timeline, state lookup, diffing)
4. Live control IPC latency (status, screen, send/wait)
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def format_ms(ns: float) -> str:
    ms = ns / 1_000_000
    if ms < 1.0:
        return f"{ms:.3f} ms"
    elif ms < 10.0:
        return f"{ms:.2f} ms"
    elif ms < 1000.0:
        return f"{ms:.1f} ms"
    else:
        return f"{ms / 1000.0:.2f} s"


def format_throughput(bytes_count: int, ns: float) -> str:
    secs = ns / 1_000_000_000
    if secs <= 0:
        return "N/A"
    mb = bytes_count / (1024 * 1024)
    mb_s = mb / secs
    return f"{mb_s:.1f} MB/s"


@dataclass
class BenchmarkMetric:
    category: str
    name: str
    unit: str
    samples: list[float]  # raw values (usually nanoseconds or MB/s)
    warmups: int
    repetitions: int
    median: float = 0.0
    mean: float = 0.0
    stddev: float = 0.0
    p95: float = 0.0
    min_val: float = 0.0
    max_val: float = 0.0

    def compute_stats(self) -> None:
        if not self.samples:
            return
        sorted_samples = sorted(self.samples)
        self.median = statistics.median(sorted_samples)
        self.mean = statistics.mean(sorted_samples)
        self.stddev = (
            statistics.stdev(sorted_samples) if len(sorted_samples) > 1 else 0.0
        )
        self.min_val = sorted_samples[0]
        self.max_val = sorted_samples[-1]
        p95_idx = int(len(sorted_samples) * 0.95)
        self.p95 = sorted_samples[min(p95_idx, len(sorted_samples) - 1)]


class BenchmarkSuite:
    def __init__(
        self,
        binary: Path,
        fixture_binary: Path,
        quick: bool = False,
    ):
        self.binary = binary
        self.fixture_binary = fixture_binary
        self.quick = quick
        self.metrics: list[BenchmarkMetric] = []
        self.temp_dir = Path(tempfile.mkdtemp(prefix="termtrace-bench-"))

    def cleanup(self) -> None:
        shutil.rmtree(self.temp_dir, ignore_errors=True)

    def run_cmd(
        self, args: list[str | Path], check: bool = True
    ) -> tuple[subprocess.CompletedProcess[bytes], float]:
        cmd = [str(x) for x in args]
        start = time.perf_counter_ns()
        res = subprocess.run(cmd, capture_output=True, check=False)
        elapsed = time.perf_counter_ns() - start
        if check and res.returncode != 0:
            err = res.stderr.decode("utf-8", errors="replace")
            raise RuntimeError(
                f"Command failed ({res.returncode}): {' '.join(cmd)}\n{err}"
            )
        return res, elapsed

    def measure(
        self,
        category: str,
        name: str,
        warmups: int,
        repetitions: int,
        fn,
        unit: str = "ms",
    ) -> BenchmarkMetric:
        if self.quick:
            warmups = max(1, warmups // 2)
            repetitions = max(2, repetitions // 2)

        for _ in range(warmups):
            fn()

        samples: list[float] = []
        for _ in range(repetitions):
            val = fn()
            samples.append(val)

        metric = BenchmarkMetric(
            category=category,
            name=name,
            unit=unit,
            samples=samples,
            warmups=warmups,
            repetitions=repetitions,
        )
        metric.compute_stats()
        self.metrics.append(metric)
        return metric

    # -------------------------------------------------------------------------
    # 1. Durability & Ingestion Hit Path
    # -------------------------------------------------------------------------
    def bench_durability(self) -> None:
        print("--> Measuring PTY Ingestion & Durability Modes...")
        payload = 5 * 1024 * 1024  # 5 MB

        for mode in ["throughput", "bounded", "strict"]:

            def run_record(mode: str = mode) -> float:
                session = self.temp_dir / f"bench-dur-{mode}.termtrace"
                if session.exists():
                    shutil.rmtree(session)
                cmd = [
                    self.binary,
                    "record",
                    "--no-input",
                    "--durability",
                    mode,
                    "--out",
                    session,
                    "--",
                    "/bin/sh",
                    "-c",
                    f"head -c {payload} /dev/zero",
                ]
                _, elapsed = self.run_cmd(cmd)
                shutil.rmtree(session, ignore_errors=True)
                return elapsed

            self.measure(
                category="Ingestion & Durability",
                name=f"record --durability {mode} (5MB)",
                warmups=2,
                repetitions=6 if not self.quick else 3,
                fn=run_record,
                unit="ms",
            )

    # -------------------------------------------------------------------------
    # 2. Semantic Rendering & SVG / Raster Hit Path
    # -------------------------------------------------------------------------
    def bench_rendering(self) -> None:
        print("--> Measuring Semantic SVG & Raster Rendering...")
        # Prepare a rich terminal session with 100 frames of styled content
        session = self.temp_dir / "render-fixture.termtrace"
        if session.exists():
            shutil.rmtree(session)
        init_cmd = [
            self.binary,
            "record",
            "--no-input",
            "--cols",
            "120",
            "--rows",
            "40",
            "--cell-width",
            "10",
            "--cell-height",
            "20",
            "--out",
            session,
            "--",
            "/bin/sh",
            "-c",
            'for i in $(seq 1 60); do printf "\\033[H\\033[1;3%dm[Frame %03d] \\033[44;37mStyled Box Content Row 1234567890\\033[0m\\n" "$((i % 7 + 1))" "$i"; done',
        ]
        self.run_cmd(init_cmd)

        # SVG Screenshot
        def run_screenshot_svg() -> float:
            out = self.temp_dir / "shot-svg"
            shutil.rmtree(out, ignore_errors=True)
            cmd = [
                self.binary,
                "screenshot",
                session,
                "--renderer",
                "semantic",
                "--format",
                "svg",
                "--out",
                out,
            ]
            _, elapsed = self.run_cmd(cmd)
            shutil.rmtree(out, ignore_errors=True)
            return elapsed

        self.measure(
            category="Rendering",
            name="screenshot --format svg (120x40)",
            warmups=2,
            repetitions=6 if not self.quick else 3,
            fn=run_screenshot_svg,
            unit="ms",
        )

        # PNG Screenshot (Supersampled2x)
        def run_screenshot_png() -> float:
            out = self.temp_dir / "shot-png"
            shutil.rmtree(out, ignore_errors=True)
            cmd = [
                self.binary,
                "screenshot",
                session,
                "--renderer",
                "semantic",
                "--format",
                "png",
                "--out",
                out,
            ]
            _, elapsed = self.run_cmd(cmd)
            shutil.rmtree(out, ignore_errors=True)
            return elapsed

        self.measure(
            category="Rendering",
            name="screenshot --format png (Supersampled2x)",
            warmups=2,
            repetitions=6 if not self.quick else 3,
            fn=run_screenshot_png,
            unit="ms",
        )

        # Presentation Preview / Video export if ffmpeg is available
        if shutil.which("ffmpeg") is not None:
            events_jsonl = session / "events.jsonl"
            with events_jsonl.open("r", encoding="utf-8") as f:
                lines = [line.strip() for line in f if line.strip()]
                last_event = json.loads(lines[-1])
                end_ns = last_event["monotonic_ns"]

            edit_path = self.temp_dir / "render-edit.json"
            res, _ = self.run_cmd(
                [
                    sys.executable,
                    str(ROOT / "benchmarks/models.py"),
                    "emit-render-edit",
                    "--end-ns",
                    str(end_ns),
                ]
            )
            edit_path.write_bytes(res.stdout)

            def run_video_export() -> float:
                out = self.temp_dir / "bench-video.mp4"
                if out.exists():
                    out.unlink()
                cmd = [
                    self.binary,
                    "video",
                    session,
                    "--renderer",
                    "semantic",
                    "--fps",
                    "24",
                    "--semantic-raster-profile",
                    "direct",
                    "--edit",
                    edit_path,
                    "--out",
                    out,
                ]
                _, elapsed = self.run_cmd(cmd)
                if out.exists():
                    out.unlink()
                return elapsed

            self.measure(
                category="Rendering",
                name="semantic video export (FFmpeg RGBA pipe)",
                warmups=1,
                repetitions=4 if not self.quick else 2,
                fn=run_video_export,
                unit="ms",
            )

            def run_presentation_preview() -> float:
                out = self.temp_dir / "bench-preview"
                shutil.rmtree(out, ignore_errors=True)
                cmd = [
                    self.binary,
                    "video",
                    session,
                    "--renderer",
                    "semantic",
                    "--semantic-raster-profile",
                    "direct",
                    "--edit",
                    edit_path,
                    "--preview-out",
                    out,
                ]
                _, elapsed = self.run_cmd(cmd)
                shutil.rmtree(out, ignore_errors=True)
                return elapsed

            self.measure(
                category="Rendering",
                name="presentation preview (contact sheet + MP4)",
                warmups=1,
                repetitions=4 if not self.quick else 2,
                fn=run_presentation_preview,
                unit="ms",
            )

    # -------------------------------------------------------------------------
    # 3. Replay Engine & Forensics Hit Path
    # -------------------------------------------------------------------------
    def bench_replay_and_forensics(self) -> None:
        print("--> Measuring Replay Engine & Forensics...")
        # Generate a canonical fixture with 64 events and 4KB output
        session = self.temp_dir / "replay-fixture.termtrace"
        if session.exists():
            shutil.rmtree(session)
        gen_cmd = [
            self.fixture_binary,
            "large",
            "--events",
            "64",
            "--output-bytes",
            "4096",
            "--out",
            session,
        ]
        self.run_cmd(gen_cmd)

        # Timeline analysis
        def run_timeline() -> float:
            cmd = [
                self.binary,
                "analyze",
                "timeline",
                session,
                "--limit",
                "100",
                "--jsonl",
            ]
            _, elapsed = self.run_cmd(cmd)
            return elapsed

        self.measure(
            category="Replay & Forensics",
            name="analyze timeline (64-event session)",
            warmups=2,
            repetitions=5 if not self.quick else 3,
            fn=run_timeline,
            unit="ms",
        )

        # Historical state inspection (screen --state)
        def run_state_screen() -> float:
            cmd = [
                self.binary,
                "screen",
                session,
                "--state",
                "30",
            ]
            _, elapsed = self.run_cmd(cmd)
            return elapsed

        self.measure(
            category="Replay & Forensics",
            name="screen --state 30 (replay lookup)",
            warmups=2,
            repetitions=5 if not self.quick else 3,
            fn=run_state_screen,
            unit="ms",
        )

        # Semantic diff between two states
        def run_diff() -> float:
            cmd = [
                self.binary,
                "analyze",
                "diff",
                session,
                "--from-state",
                "10",
                "--to-state",
                "30",
                "--format",
                "text",
            ]
            _, elapsed = self.run_cmd(cmd)
            return elapsed

        self.measure(
            category="Replay & Forensics",
            name="analyze diff --from-state 10 --to-state 30",
            warmups=1,
            repetitions=3 if not self.quick else 2,
            fn=run_diff,
            unit="ms",
        )

    # -------------------------------------------------------------------------
    # 4. Live Control IPC & Wait Hit Path
    # -------------------------------------------------------------------------
    def bench_live_control(self) -> None:
        print("--> Measuring Live Control IPC & Wait...")
        # Start a background session
        session_name = f"bench-ipc-{os.getpid()}"
        start_cmd = [
            self.binary,
            "record",
            "-d",
            session_name,
            "--cols",
            "120",
            "--rows",
            "40",
            "--",
            "/bin/sh",
            "-c",
            'printf "Ready\\n"; while :; do sleep 0.1; done',
        ]
        self.run_cmd(start_cmd)

        try:
            # Synchronize on readiness
            self.run_cmd(
                [
                    self.binary,
                    "wait",
                    session_name,
                    "--screen",
                    "Ready",
                    "--timeout",
                    "5s",
                ]
            )

            # Measure live screen text query
            def run_live_screen() -> float:
                cmd = [self.binary, "screen", session_name]
                _, elapsed = self.run_cmd(cmd)
                return elapsed

            self.measure(
                category="Live Control IPC",
                name="screen live session (text)",
                warmups=3,
                repetitions=10 if not self.quick else 4,
                fn=run_live_screen,
                unit="ms",
            )

            # Measure live screen json projection query
            def run_live_screen_json() -> float:
                cmd = [self.binary, "screen", session_name, "--json"]
                _, elapsed = self.run_cmd(cmd)
                return elapsed

            self.measure(
                category="Live Control IPC",
                name="screen live session (--json projection)",
                warmups=3,
                repetitions=10 if not self.quick else 4,
                fn=run_live_screen_json,
                unit="ms",
            )

            # Measure live status query
            def run_live_status() -> float:
                cmd = [self.binary, "status", session_name, "--json"]
                _, elapsed = self.run_cmd(cmd)
                return elapsed

            self.measure(
                category="Live Control IPC",
                name="status live session (--json)",
                warmups=3,
                repetitions=10 if not self.quick else 4,
                fn=run_live_status,
                unit="ms",
            )

            # Measure send and wait postcondition
            def run_send_wait() -> float:
                cmd = [
                    self.binary,
                    "send",
                    session_name,
                    "text:x",
                    "--until-screen",
                    "Ready",
                    "--timeout",
                    "2s",
                ]
                _, elapsed = self.run_cmd(cmd)
                return elapsed

            self.measure(
                category="Live Control IPC",
                name="send text:x --until-screen Ready",
                warmups=2,
                repetitions=6 if not self.quick else 3,
                fn=run_send_wait,
                unit="ms",
            )

        finally:
            self.run_cmd([self.binary, "stop", session_name, "--wait"], check=False)
            self.run_cmd([self.binary, "status", "--gc"], check=False)

    def print_report(self) -> None:
        print("\n" + "=" * 80)
        print("  TERMTRACE BASELINE PERFORMANCE BENCHMARK REPORT")
        print("=" * 80 + "\n")

        current_cat = ""
        for m in self.metrics:
            if m.category != current_cat:
                current_cat = m.category
                print(f"\n### {current_cat}")
                print(
                    f"{'Operation':<45} | {'Median':<10} | {'Mean':<10} | {'P95':<10} | {'Min':<10} | {'Max':<10}"
                )
                print("-" * 105)

            med_str = format_ms(m.median)
            mean_str = format_ms(m.mean)
            p95_str = format_ms(m.p95)
            min_str = format_ms(m.min_val)
            max_str = format_ms(m.max_val)

            print(
                f"{m.name:<45} | {med_str:<10} | {mean_str:<10} | {p95_str:<10} | {min_str:<10} | {max_str:<10}"
            )

        print("\n" + "=" * 80 + "\n")

    def export_json(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        data = {
            "timestamp_unix_ns": time.time_ns(),
            "binary": str(self.binary),
            "metrics": [asdict(m) for m in self.metrics],
        }
        with path.open("w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)
        print(f"Saved baseline metrics JSON to: {path}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Termtrace Baseline Benchmark")
    parser.add_argument(
        "--binary", type=Path, default=ROOT / "target/release/termtrace"
    )
    parser.add_argument(
        "--fixture-binary", type=Path, default=ROOT / "target/release/termtrace-fixture"
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "benchmarks/results/baseline_pre_optimization.json",
    )
    parser.add_argument(
        "--quick", action="store_true", help="Run fewer iterations for quick baseline"
    )
    args = parser.parse_args()

    if not args.binary.exists():
        print(
            f"Error: binary {args.binary} does not exist. Build it with `cargo build --release` first.",
            file=sys.stderr,
        )
        return 1

    if not args.fixture_binary.exists():
        print(f"Building fixture binary {args.fixture_binary}...", flush=True)
        subprocess.run(
            [
                "cargo",
                "build",
                "--release",
                "-p",
                "termtrace-test-fixture",
                "--bin",
                "termtrace-fixture",
            ],
            cwd=ROOT,
            check=True,
        )

    suite = BenchmarkSuite(
        binary=args.binary, fixture_binary=args.fixture_binary, quick=args.quick
    )
    try:
        suite.bench_durability()
        suite.bench_rendering()
        suite.bench_replay_and_forensics()
        suite.bench_live_control()
        suite.print_report()
        suite.export_json(args.output)
    finally:
        suite.cleanup()

    return 0


if __name__ == "__main__":
    sys.exit(main())
