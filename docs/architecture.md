# Architecture

Termtrace separates the original terminal execution from the views derived from it.

## Evidence layers

| Layer | Contents | Role |
| --- | --- | --- |
| Execution | PTY output, input, resize, signal, marker, lifecycle, and exit events | canonical |
| Semantic | terminal states reconstructed by the pinned parser | derived |
| Native visual | pixels drawn by the installed terminal application | derived |
| Presentation | screenshots, video, captions, and edits | derived |

The append-only execution log is the source of truth. A session manifest is published before recording. Each event commits to the previous event, and the completion manifest binds the final event hash, counts, and exit outcome.

This detects changes relative to retained hashes. It does not identify the recorder. Optional detached receipts add an external signature; signer trust still depends on the reviewer's allowed-signers policy.

## Runtime

```text
human input ────────┐
control socket ─────┼─> session actor ─> child PTY
terminal replies ───┘        │
                             ├─> canonical event log
                             └─> live semantic state

completed event log ─> semantic replay
                    ├> semantic video rendering (explicit alternative)
                    └> native capture on a private display (default video source)
```

The session actor orders child output, control requests, and terminal-generated
replies. State-changing requests are recorded before their effects are
acknowledged. Read-only operations such as `ping`, `status`, `screen`, and waits
do not change the event log.

The tmux integration installs Termtrace as the root process for each managed
pane. Termtrace creates an inner PTY for the configured shell or explicit
command, forwards the outer pane's input and output, propagates resize and
lifecycle signals, and sends every event through the same session actor. A
small per-server supervisor audits the pane root processes so direct tmux
commands and respawns cannot silently bypass recording in strict mode.

## Canonical recording

`session.json` fixes the command, working directory, geometry, terminal profile, value-free child-environment policy and effective-environment commitment, provenance policy, durability policy, resource limits, and optional stable tmux pane provenance. `events.jsonl` contains the integrity-chained execution. `completion.json` closes the recording, records any resource trigger, and rejects pending mutations.

Input, resize, and signal operations use intent/result pairs. Multi-atom input is admitted under one state guard and retains one pair per atom plus an exact attempted-prefix result if PTY delivery stops. If recording stops between a pair, delivery is uncertain. Termtrace does not retry an uncertain mutation automatically.

A separate request journal makes caller-owned mutation IDs idempotent across retries. It can be reconstructed from canonical events and is not itself an evidence root.

## Live semantic state

The pinned semantic engine runs while recording. It answers terminal queries and maintains two states:

- the newest parser state;
- the newest state eligible for presentation.

They differ during synchronized output. Screen observations and waits use the presented state so hidden intermediate changes do not masquerade as visible UI.

`capture_stable` is registered and completed inside this actor. Stability
timing, semantic bytes and text, the state hash, terminal modes, and source
boundaries therefore describe one state. Agent responses project that state
into a compact task result; `--response full` exposes forensic internals. Typed
deadline, output-close, and completion outcomes keep input application,
postcondition status, retry policy, and session lifecycle distinct.

Mutations may carry an expected state hash. The actor checks it immediately
before writing to the PTY and rejects stale actions without creating an input
intent.

## Replay and capture

Replay validates the canonical log and feeds output bytes through the pinned semantic engine. It emits a new state only when terminal content changes. Video frame rate does not limit semantic state granularity.

Native capture launches the installed terminal application on a private display and loads the exact flattened replay input chosen in `ghostty-profile.ghostty`. Fitted recordings retain the measured viewport, grid, cell size, and a separate fingerprint of the effective output Ghostty subsequently reports. Capture verifies that fingerprint and the geometry before playback instead of guessing font metrics. The subsequent report is retained as the visual diagnostic `ghostty-effective-config.txt`.

Semantic video instead schedules presentation-eligible replay states onto deterministic frame boundaries, renders them with the bundled semantic renderer, and retains exact state-to-frame spans. It requires no native display stack, but it is labeled semantic rendering and does not claim Ghostty pixel parity. Presentation source selection is explicit and never falls back automatically; native remains the default.

Derived manifests bind their source range, component versions, configuration, and member hashes. Completed derivations are cataloged separately; the closed execution log is never reopened.

## Names and restarts

Friendly names live in an owner-only registry outside the recording. Each entry binds a canonical path and immutable session ID. Removing or retargeting a name cannot rewrite evidence.

Restart creates a new session and records its predecessor identity. The old session remains closed and unchanged. Environment reconstruction is verified against the predecessor's effective-environment commitment; a mismatch is refused instead of silently inheriting a different host environment. Explicit environment and resource overrides are bound into the successor manifest.

## Dependency boundary

The semantic engine is pinned behind an internal adapter. Its unstable API does not appear in public artifact or control schemas. Upgrades require fixture replay and semantic comparison.

The current Linux native path uses a real GUI process because no stable headless framebuffer API is available. Semantic rendering remains available without native capture dependencies.

## Failure policy

- Derived-artifact failure never deletes or rewrites the recording.
- Event-log failure stops further mutations.
- Uncertain PTY writes are not retried automatically.
- Existing evidence and artifact directories are not replaced.
- Cleanup errors are reported without hiding the primary failure.
- Public failures use stable diagnostic codes and structured context.
