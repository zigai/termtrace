# Artifacts and verification

A session directory contains canonical execution evidence and optional derived views. All files are owner-only by default.

## Session layout

```text
SESSION/
├── session.json
├── events.jsonl
├── completion.json
├── requests.jsonl
├── ghostty-config.txt
├── derived/
├── visual/
└── artifact-roots.jsonl
```

Only the first three files are required canonical evidence. Other entries appear when their feature is used.

## Canonical files

### `session.json`

The immutable session manifest records the command, working directory, initial geometry, terminal profile, build identity, value-free child-environment policy and effective digest, sensitivity-aware provenance, durability policy, and resource limits. It is published before the event log. The current session-manifest version is 9.

### `events.jsonl`

The first line is a header that binds the exact session manifest and terminal defaults. Remaining lines are ordered, BLAKE3-chained event records. Raw bytes use padded Base64.

Input, resize, and signal mutations have intent/result pairs. A missing result means the outcome is uncertain.

Offline readers fail closed at 64 MiB per JSONL line, 1,000,000 events,
512 MiB of encoded log data, and 256 MiB of decoded byte payloads. Initial
and resize geometry is limited to 1,000 columns, 1,000 rows, and 250,000
cells. These limits apply equally to complete and prefix readers.

### `completion.json`

The completion manifest binds the session and event roots, final exit event, event and output counts, exit outcome, durability record, and optional resource-limit trigger. A complete session ends in `Exit` and has no pending mutation. The current completion version is 4.

## API contract versions

Atomic input sequences and structured retry details use JSONL driver protocol version 3 and control protocol version 13. Scenario files that use the isolation launch contract have scenario version 2; legacy version 1 files remain accepted when they do not use those fields. Checked-in JSON Schemas are authoritative for each shape.

## Supporting canonical data

`ghostty-config.txt` is the flattened effective terminal profile used by fitted geometry and native capture. External fonts, shaders, and images are referenced rather than copied.

`requests.jsonl` stores mutation outcomes so retries remain idempotent after in-memory cache eviction. It is disposable acceleration, not an evidence root. Verification reports journal damage separately from canonical validity.

Friendly-name registry files are also outside canonical evidence. Names may change without changing a session's identity.

## Semantic artifacts

Running `termtrace analyze index --session SESSION` creates:

| File | Contents |
| --- | --- |
| `derived/states.jsonl.zst` | compressed full-first, delta-following terminal states |
| `derived/state-index.jsonl` | event, time, byte offset, state hash, and presentation mapping |
| `derived/final.txt` | final viewport as text |
| `derived/final.vt` | final state formatted as terminal bytes |
| `derived/final.html` | final state formatted as HTML |
| `derived/manifest.json` | source identity and hashes for every member |

Ranged indexes use a range-specific directory. Live snapshots are hash-addressed and marked incomplete. `--presented-only` excludes states hidden by synchronized output without changing canonical events.

## Native visual artifacts

Running `termtrace artifact capture --session SESSION` creates:

| File | Contents |
| --- | --- |
| `visual/session.mkv` | lossless RGB H.264 video at 24 FPS |
| `visual/final.png` | stable final native frame |
| `visual/video-map.jsonl` | semantic-state to nominal-frame mapping |
| `visual/manifest.json` | source, geometry, configuration, clocks, probes, and member hashes |

Native capture is a separate capability. It never silently falls back to semantic rendering.

## Presentation artifacts

`termtrace artifact video export` publishes `presentation.mp4`, a normalized plan, an output probe, a per-frame presentation map, and a manifest. Native Ghostty capture is the default presentation source.

With `--source semantic`, the same directory also retains:

| File | Contents |
| --- | --- |
| `semantic-source.mkv` | lossless deterministic semantic-rendering source |
| `semantic-source-map.jsonl` | compact exact frame spans bound to semantic state IDs and hashes |
| `semantic-source-manifest.json` | canonical source, renderer, font, schedule, pixel-stream, FFmpeg, and probe identities |
| `semantic-source-ffmpeg.log` | source encoder diagnostics |

The final presentation map includes exact source semantic state identity when semantic rendering was selected. Manifests label the source as `native_encoded_evidence` or `semantic_rendering`; these truth layers are not interchangeable.

## Derived artifact catalog

Completed derivations append a root to `artifact-roots.jsonl`. Each entry binds the derived manifest to the session and final event hash. This catalog is locally integrity-checked but unauthenticated until covered by a trusted receipt.

## Verify a session

```bash
termtrace integrity verify SESSION --level content --json
```

Verification levels are cumulative:

| Level | Checks |
| --- | --- |
| `structure` | required files, permissions, and basic shape |
| `content` | canonical hashes, events, completion, and derived members |
| `durability-record` | content plus the recorded synchronization contract |
| `signature` | content plus detached signature and caller trust policy |

Offline verification cannot prove historical storage behavior. A valid unsigned hash graph detects changes relative to its retained roots but does not authenticate who created it.

## Sign and authenticate

Verify content before creating a detached OpenSSH receipt:

```bash
termtrace integrity sign SESSION --key SSH_KEY --signer BUILD_IDENTITY
termtrace integrity verify SESSION \
  --level signature \
  --allowed-signers ALLOWED_SIGNERS \
  --json
```

Cryptographic validity and trusted identity are separate results. Authenticity is established only when the caller's allowed-signers file trusts the asserted signer. A receipt covers the artifact catalog prefix that exists when the receipt is created.

## Schemas

Public JSON Schemas are checked into `schemas/` and generated from Rust types:

```bash
termtrace tooling schemas --out schemas
```

Tests require generated output to match the checked-in files and validate fixtures against the schemas. Schema validity describes structure, not trust or safety.

## Export and handoff

`export-asciicast` creates a lossy UTF-8 interoperability view. Invalid bytes, exact timing, and advanced semantic state cannot be preserved, so the cast is never canonical evidence.

`save`, `screenshot`, and `bundle` publish owner-only, no-replace artifact directories with manifests. Use a bundle profile appropriate to the data being shared and verify the bundle after copying it.

Bundle manifests are limited to 1 MiB and 10,000 payload members. Bundle
verification requires the caller's expected source session and profile; member
sensitivity, relationship, and schema labels are derived by the verifier rather
than accepted from the bundle.

Terminal content remains untrusted and potentially sensitive in every representation.
