"""Scenario suites run through the Python facade against the real recorder."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any
from xml.etree import ElementTree

import pytest
from termtrace import (
    Termtrace,
    TermtraceError,
    scenario_ci_failed,
    write_scenario_junit,
)
from termtrace.aio import AsyncTermtrace

TERMTRACE_BIN = os.path.abspath(
    os.environ.get("TERMTRACE_BIN", "target/debug/termtrace")
)
SHELL_PATH = {"base": "clear", "copy": ["PATH"]}


def write_scenario(
    path: Path,
    name: str,
    script: str,
    steps: list[dict[str, Any]],
    *,
    env: dict[str, str] | None = None,
    matrix: dict[str, list[str]] | None = None,
    timeout_ms: int = 5000,
) -> Path:
    document: dict[str, Any] = {
        "version": 2,
        "name": name,
        "launch": {
            "command": ["/bin/sh", "-c", script],
            "geometry": {"cols": 40, "rows": 4},
            "environment": {**SHELL_PATH, "set": env or {}},
            "timeoutMs": timeout_ms,
        },
        "steps": steps,
    }
    if matrix is not None:
        document["matrix"] = matrix
    path.write_text(json.dumps(document), encoding="utf-8")
    return path


def ready_scenario(path: Path, name: str, delay: str = "0") -> Path:
    return write_scenario(
        path,
        name,
        f"sleep {delay}; printf READY; read line",
        [{"waitScreen": {"contains": "READY"}}, {"stop": {}}],
    )


def failing_scenario(path: Path) -> Path:
    return write_scenario(
        path,
        "never shows done",
        "printf READY; read line",
        [{"waitScreen": {"contains": "DONE", "timeoutMs": 500}}, {"stop": {}}],
    )


def client(tmp_path: Path) -> Termtrace:
    return Termtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))


def attempt_total(report: dict[str, Any]) -> int:
    return sum(a["duration_ms"] for case in report["cases"] for a in case["attempts"])


def test_parallel_jobs_overlap_cases_and_report_every_case(tmp_path: Path) -> None:
    scenarios = [
        ready_scenario(tmp_path / f"slow-{i}.json", f"slow {i}", "1") for i in range(3)
    ]
    tt = client(tmp_path)

    parallel = tt.run_scenarios(
        scenarios, failure_root=tmp_path / "parallel" / "failures", jobs=3
    )
    assert parallel["jobs"] == 3
    assert sorted(Path(case["scenario"]) for case in parallel["cases"]) == sorted(
        scenarios
    )
    assert all(case["status"] == "passed" for case in parallel["cases"])
    assert parallel["duration_ms"] < attempt_total(parallel) * 2 / 3

    serial = tt.run_scenarios(
        scenarios, failure_root=tmp_path / "serial" / "failures", jobs=1
    )
    assert len(serial["cases"]) == 3
    assert serial["duration_ms"] >= attempt_total(serial)

    with pytest.raises(TermtraceError, match="jobs"):
        tt.run_scenarios(scenarios, failure_root=tmp_path / "none" / "failures", jobs=0)


@pytest.fixture
def matrix_document(tmp_path: Path) -> tuple[Path, Path]:
    created = tmp_path / "created"
    created.mkdir()
    document = write_scenario(
        tmp_path / "matrix.json",
        "matrix",
        'printf x > "$CREATED/{{size}}-{{theme}}"; printf READY; read line',
        [{"waitScreen": {"contains": "READY"}}, {"stop": {}}],
        env={"CREATED": str(created)},
        matrix={"size": ["small", "large"], "theme": ["dark", "light"]},
    )
    return document, created


async def test_matrix_document_expands_to_one_labelled_case_per_combination(
    tmp_path: Path, matrix_document: tuple[Path, Path]
) -> None:
    document, created = matrix_document
    tt = AsyncTermtrace(binary=TERMTRACE_BIN, evidence_root=str(tmp_path / "sessions"))

    report = await tt.run_scenarios(
        [document], failure_root=tmp_path / "out" / "failures", jobs=4
    )

    assert report["status"] == "passed"
    assert sorted(path.name for path in created.iterdir()) == [
        "large-dark",
        "large-light",
        "small-dark",
        "small-light",
    ]
    names = [case["attempts"][0]["report"]["name"] for case in report["cases"]]
    identities = [case["scenario"] for case in report["cases"]]
    for size in ["small", "large"]:
        for theme in ["dark", "light"]:
            label_parts = (f"size={size}", f"theme={theme}")
            assert sum(all(part in name for part in label_parts) for name in names) == 1
            assert (
                sum(
                    all(part in identity for part in label_parts)
                    for identity in identities
                )
                == 1
            )
    assert (
        len(
            {
                a["report"]["canonical_session"]
                for c in report["cases"]
                for a in c["attempts"]
            }
        )
        == 4
    )


def test_shards_partition_the_expanded_cases(
    tmp_path: Path, matrix_document: tuple[Path, Path]
) -> None:
    document, _ = matrix_document
    tt = client(tmp_path)
    everything = tt.run_scenarios(
        [document], failure_root=tmp_path / "all" / "failures", jobs=4
    )

    shards = [
        tt.run_scenarios(
            [document],
            failure_root=tmp_path / f"shard-{index}" / "failures",
            jobs=2,
            shard_index=index,
            shard_count=2,
        )
        for index in range(2)
    ]

    selected = [{case["scenario"] for case in shard["cases"]} for shard in shards]
    assert [(shard["shard_index"], shard["shard_count"]) for shard in shards] == [
        (0, 2),
        (1, 2),
    ]
    assert all(selected)
    assert not selected[0] & selected[1]
    assert selected[0] | selected[1] == {
        case["scenario"] for case in everything["cases"]
    }

    with pytest.raises(TermtraceError, match="shard"):
        tt.run_scenarios(
            [document],
            failure_root=tmp_path / "bad" / "failures",
            shard_index=2,
            shard_count=2,
        )
    with pytest.raises(TermtraceError, match="shard"):
        tt.run_scenarios(
            [document], failure_root=tmp_path / "bad" / "failures", shard_index=0
        )


def test_snapshot_update_writes_a_baseline_that_later_runs_compare(
    tmp_path: Path,
) -> None:
    text = tmp_path / "screen.txt"
    text.write_text("alpha beta\n", encoding="utf-8")
    document = write_scenario(
        tmp_path / "snapshot.json",
        "snapshot",
        'IFS= read -r line < "$TEXT"; printf "%s" "$line"; read line',
        [
            {"waitScreen": {"contains": "alpha"}},
            {"matchScreenSnapshot": {"name": "greeting", "stableForMs": 10}},
            {"stop": {}},
        ],
        env={"TEXT": str(text)},
    )
    artifacts = tmp_path / "artifacts"
    tt = client(tmp_path)

    updated = tt.run_scenarios(
        [document], failure_root=artifacts / "failures", update_snapshots=True
    )
    assert updated["status"] == "passed"
    baselines = list((artifacts / "snapshots").iterdir())
    assert len(baselines) == 1
    baseline = baselines[0].read_bytes()
    assert b"alpha beta" in baseline

    unchanged = tt.run_scenarios([document], failure_root=artifacts / "failures")
    assert unchanged["status"] == "passed"

    text.write_text("alpha gamma\n", encoding="utf-8")
    changed = tt.run_scenarios([document], failure_root=artifacts / "failures")
    attempt = changed["cases"][0]["attempts"][0]["report"]
    assert changed["status"] == "failed"
    assert attempt["failure"]["step_kind"] == "match_screen_snapshot"
    assert Path(attempt["failure_artifacts"]).is_dir()
    assert baselines[0].read_bytes() == baseline


def test_reruns_keep_failed_attempts_and_report_recovered_cases_as_flaky(
    tmp_path: Path,
) -> None:
    counter = tmp_path / "attempts"
    document = write_scenario(
        tmp_path / "flaky.json",
        "passes on the second attempt",
        'n=0; if test -f "$COUNTER"; then IFS= read -r n < "$COUNTER"; fi; n=$((n + 1)); '
        'printf "%s\\n" "$n" > "$COUNTER"; if test "$n" -eq 1; then printf BROKEN; else printf DONE; fi; read line',
        [{"waitScreen": {"contains": "DONE"}}, {"stop": {}}],
        env={"COUNTER": str(counter)},
        timeout_ms=1000,
    )
    tt = client(tmp_path)

    permissive = tt.run_scenarios(
        [document], failure_root=tmp_path / "a" / "failures", reruns=2
    )
    case = permissive["cases"][0]
    assert permissive["status"] == "flaky"
    assert case["status"] == "flaky"
    assert [a["report"]["status"] for a in case["attempts"]] == ["failed", "passed"]
    assert (
        case["attempts"][0]["report"]["canonical_session"]
        != case["attempts"][1]["report"]["canonical_session"]
    )
    assert Path(case["attempts"][0]["report"]["failure_artifacts"]).is_dir()
    assert counter.read_text(encoding="utf-8") == "2\n"
    assert not scenario_ci_failed(permissive)

    counter.unlink()
    strict = tt.run_scenarios(
        [document],
        failure_root=tmp_path / "b" / "failures",
        reruns=2,
        fail_on_flaky=True,
    )
    assert strict["status"] == "flaky"
    assert strict["fail_on_flaky"]
    assert scenario_ci_failed(strict)

    counter.unlink()
    exhausted = tt.run_scenarios([document], failure_root=tmp_path / "c" / "failures")
    assert exhausted["status"] == "failed"
    assert len(exhausted["cases"][0]["attempts"]) == 1


@pytest.fixture
def mixed_report(tmp_path: Path) -> dict[str, Any]:
    passing = ready_scenario(tmp_path / "a-passing.json", "shows ready")
    failing = failing_scenario(tmp_path / "b-failing.json")
    return client(tmp_path).run_scenarios(
        [passing, failing], failure_root=tmp_path / "mixed" / "failures", jobs=2
    )


def test_junit_has_every_case_and_marks_failures(
    tmp_path: Path, mixed_report: dict[str, Any]
) -> None:
    junit = tmp_path / "junit.xml"
    write_scenario_junit(junit, mixed_report)

    suite = ElementTree.parse(junit).getroot()
    cases = {case.get("name"): case for case in suite.iter("testcase")}
    assert suite.tag == "testsuite"
    assert (suite.get("tests"), suite.get("failures"), suite.get("errors")) == (
        "2",
        "1",
        "0",
    )
    assert set(cases) == {"shows ready", "never shows done"}
    assert cases["shows ready"].find("failure") is None
    failure = cases["never shows done"].find("failure")
    assert failure is not None
    assert "wait_screen" in (failure.text or "")

    before = junit.read_bytes()
    with pytest.raises(TermtraceError):
        write_scenario_junit(junit, mixed_report)
    assert junit.read_bytes() == before


def test_ci_verdict_fails_only_when_a_case_fails(
    tmp_path: Path, mixed_report: dict[str, Any]
) -> None:
    assert mixed_report["status"] == "failed"
    assert [case["status"] for case in mixed_report["cases"]] == ["passed", "failed"]
    assert scenario_ci_failed(mixed_report)

    passing = client(tmp_path).run_scenarios(
        [ready_scenario(tmp_path / "only.json", "only")],
        failure_root=tmp_path / "ok" / "failures",
    )
    assert passing["status"] == "passed"
    assert not scenario_ci_failed(passing)

    relabelled = {**mixed_report, "status": "passed"}
    assert scenario_ci_failed(relabelled)
