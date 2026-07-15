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
                    └> native capture on a private display
```

The session actor orders child output, control requests, and terminal-generated replies. State-changing requests are recorded before their effects are acknowledged. Read-only operations such as `ping`, `status`, `show`, and waits do not change the event log.

## Canonical recording

`session.json` fixes the command, working directory, geometry, terminal profile, provenance policy, and durability policy. `events.jsonl` contains the integrity-chained execution. `completion.json` closes the recording and rejects pending mutations.

Input, resize, and signal operations use intent/result pairs. If recording stops between the pair, delivery is uncertain. Termtrace does not retry an uncertain mutation automatically.

A separate request journal makes caller-owned mutation IDs idempotent across retries. It can be reconstructed from canonical events and is not itself an evidence root.

## Live semantic state

The pinned semantic engine runs while recording. It answers terminal queries and maintains two states:

- the newest parser state;
- the newest state eligible for presentation.

They differ during synchronized output. Screen observations and waits use the presented state so hidden intermediate changes do not masquerade as visible UI.

Mutations may carry an expected state hash. The actor checks this immediately before writing to the PTY and rejects stale actions without creating an input intent.

## Replay and capture

Replay validates the canonical log and feeds output bytes through the pinned semantic engine. It emits a new state only when terminal content changes. Video frame rate does not limit semantic state granularity.

Native capture launches the installed terminal application on a private display. Fitted recordings retain the measured viewport, grid, cell size, and effective configuration hash. Capture verifies geometry before playback instead of guessing font metrics.

Derived manifests bind their source range, component versions, configuration, and member hashes. Completed derivations are cataloged separately; the closed execution log is never reopened.

## Names and restarts

Friendly names live in an owner-only registry outside the recording. Each entry binds a canonical path and immutable session ID. Removing or retargeting a name cannot rewrite evidence.

Restart creates a new session and records its predecessor identity. The old session remains closed and unchanged.

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
