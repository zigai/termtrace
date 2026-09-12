"""Validated acceptance contracts and machine-readable measured outcomes."""
from __future__ import annotations

import math
from typing import Literal

import msgspec


class Gate(msgspec.Struct, forbid_unknown_fields=True):
    id: str
    kind: Literal["ergonomics", "latency", "latency_delta", "complexity"]
    operation: str = ""
    operations: tuple[str, ...] = ()
    workflows: tuple[str, ...] = ()
    fixture: str = ""
    cache_policy: str = "warm"
    percentile: int = 95
    maximum_ms: int | None = None
    maximum_delta_ms: int | None = None
    maximum_commands_per_workflow: int | None = None
    maximum_required_identifiers_per_workflow: int | None = None
    reference: str = ""
    requirement: str = ""


class Measurement(msgspec.Struct, forbid_unknown_fields=True):
    clock: str
    warmup_repetitions: int
    measured_repetitions: int
    reported_percentiles: tuple[int, ...]
    percentile_method: str
    cold_policy: str
    warm_policy: str
    concurrency: int
    timeout_seconds: int
    resident_memory_limit_bytes: int

    def __post_init__(self) -> None:
        if (self.clock != "CLOCK_MONOTONIC" or self.warmup_repetitions != 5
                or self.measured_repetitions != 30 or self.reported_percentiles != (50, 95, 99)
                or self.percentile_method != "nearest-rank" or self.concurrency != 1
                or self.timeout_seconds != 120 or self.resident_memory_limit_bytes != 536_870_912):
            raise ValueError("acceptance requires the pinned complete measurement protocol; use --reduced for reduced mechanics")


class Fixture(msgspec.Struct, forbid_unknown_fields=True):
    id: str
    kind: str
    command: tuple[str, ...] = ()
    completion_marker: str = ""
    event_count: int = 0
    output_bytes: int = 0
    generator: str = ""


class Validator(msgspec.Struct, forbid_unknown_fields=True):
    artifact: str
    command: str


class Contract(msgspec.Struct, rename={"schema": "$schema"}, forbid_unknown_fields=True):
    schema: str
    format: str
    version: int
    baseline: dict[str, str]
    environment: dict[str, str | int]
    measurement: Measurement
    defaults: dict[str, object]
    fixtures: tuple[Fixture, ...]
    gates: tuple[Gate, ...]
    invariants: tuple[str, ...]
    artifact_validators: tuple[Validator, ...]


class Sample(msgspec.Struct):
    elapsed_ns: int
    maximum_rss_bytes: int
    stdout_bytes: int
    command: tuple[str, ...]
    evidence: str
    clock_uncertainty_ns: int = 0


class Distribution(msgspec.Struct):
    p50_ns: int
    p95_ns: int
    p99_ns: int
    maximum_rss_bytes: int


class Result(msgspec.Struct):
    id: str
    kind: str
    status: Literal["passed", "failed", "ineligible", "reduced"]
    samples: tuple[Sample, ...] = ()
    distribution: Distribution | None = None
    threshold_ms: int | None = None
    details: dict[str, object] = msgspec.field(default_factory=dict)
    error: str | None = None


def percentile(values: list[int], rank: int) -> int:
    if not values or not 0 < rank <= 100:
        raise ValueError("percentile needs samples and a rank in (0, 100]")
    return sorted(values)[math.ceil(len(values) * rank / 100) - 1]


def distribution(samples: list[Sample]) -> Distribution:
    values = [sample.elapsed_ns for sample in samples]
    return Distribution(*(percentile(values, rank) for rank in (50, 95, 99)),
                        maximum_rss_bytes=max(sample.maximum_rss_bytes for sample in samples))


def latency_result(gate: Gate, samples: list[Sample], eligible: bool, reduced: bool,
                   memory_limit: int) -> Result:
    summary = distribution(samples)
    threshold = gate.maximum_ms if gate.maximum_ms is not None else gate.maximum_delta_ms
    if threshold is None:
        raise ValueError(f"{gate.id}: missing quantitative threshold")
    upper_bounds = [sample.elapsed_ns + sample.clock_uncertainty_ns for sample in samples]
    passed = (percentile(upper_bounds, gate.percentile) <= threshold * 1_000_000
              and summary.maximum_rss_bytes <= memory_limit)
    status = "failed" if not passed else "reduced" if reduced else "passed" if eligible else "ineligible"
    return Result(gate.id, gate.kind, status, tuple(samples), summary, threshold,
                  {"absolute_latency_host_eligible": eligible,
                   "uncertainty_included_in_threshold": True})
