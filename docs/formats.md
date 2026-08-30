# Artifacts and verification

A session directory contains canonical execution evidence and optional derived views. All files are owner-only by default.

## Session layout

```text
SESSION/
├── session.json
├── events.jsonl
├── completion.json
├── requests.jsonl
├── ghostty-profile.ghostty
├── derived/
├── visual/
└── artifact-roots.jsonl
```

Only the first three files are required canonical evidence. Other entries appear when their feature is used.

## Canonical files

### `session.json`

The immutable session manifest records the command, working directory, initial geometry, terminal profile, build identity, value-free child-environment policy and effective digest, sensitivity-aware provenance, durability policy, resource limits, and optional stable tmux pane provenance. It is published before the event log. The current session-manifest version is 12. Version 12 adds chain-bound provenance for imported ANSI streams. Version 11 added the tmux server, session, window, pane, geometry, start-command, and launch-kind provenance captured by native tmux recording. Version 10 previously separated the flattened replay profile from the fingerprint of Ghostty's reported effective output.

### `events.jsonl`

The first line is a header that binds the exact session manifest and terminal defaults. Remaining lines are ordered, BLAKE3-chained event records. Raw bytes use padded Base64.

Input, resize, and signal mutations have intent/result pairs. A missing result means the outcome is uncertain.

Offline readers fail closed at 64 MiB per JSONL line, 1,000,000 events,
512 MiB of encoded log data, and 256 MiB of decoded byte payloads. Initial
and resize geometry is limited to 1,000 columns, 1,000 rows, and 250,000
cells. These limits apply equally to complete and prefix readers.

### `completion.json`

The completion manifest binds the session and event roots, final exit event, event and output counts, exit outcome, durability record, and optional resource-limit trigger. A complete session ends in `Exit` and has no pending mutation. The current completion version is 5.

## API contract versions

Rich screen frames, atomic input sequences, and structured retry details use
JSONL driver protocol version 5 and control protocol version 15. This is a
clean cutover: older peers are rejected as incompatible rather than receiving
partial frames or losing action outcomes. Scenario files require scenario
version 2. Checked-in JSON Schemas are authoritative for each shape.

`schemas/cli-response.schema.json` defines CLI agent response version 1. Agent
mode writes exactly one object, bounded to 64 KiB, with required `v`, `ok`, and
`type` fields. Failures also require `code`, `applied`, `retry`, and
`session_state`. These fields are authoritative: a failed postcondition can
coexist with applied input, and callers must not invent a retry policy from the
process exit code. Logs and progress are written to stderr.

The Rust contract owner is
`termtrace_model::protocol::cli_response::{CLI_RESPONSE_VERSION,
CliResponseEnvelope, Applied, Retry, SessionState}`. The executable's
`cli::output` modules only select and render response modes and flatten
command-specific fields into that envelope; they do not own another response
model.

`schemas/tmux-recording-manifest.schema.json` defines aggregate tmux recording
version 2: the composite client track, linked pane tracks, observed lifecycle,
layout and focus transitions, and explicit coverage gaps.

CLI result fields ending in `*_display` are lossy human-readable path
conveniences. The adjacent byte-safe path fields remain authoritative.

## Supporting canonical data

`ghostty-profile.ghostty` contains the exact flattened input Termtrace chose for replay. Termtrace may derive those bytes from Ghostty's reported normalization of the selected configuration and overrides. Ghostty guarantees that reported configuration remains valid replayable input with equivalent behavior, although another load-and-report cycle need not produce identical bytes. The profile is not a bundle of every dependency: fonts, shaders, images, and other referenced resources remain external.

After Ghostty loads those chosen replay bytes, the session separately binds a fingerprint of the effective output Ghostty reports. Native capture retains this subsequent report as the visual diagnostic `visual/ghostty-effective-config.txt`. Keeping the chosen input and subsequent output distinct verifies how the profile resolves even when their serialized bytes differ.

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
Semantic state format 5 records Ghostty's authoritative row soft-wrap and
continuation flags and each cell's native wide role. Its state hashes use the
`termtrace-semantic-state-v2` domain, so semantic hash selectors are scoped to
the state-format version. Version-4 derived checkpoints and indexes cannot
truthfully supply these facts and are rejected rather than defaulted; regenerate
them from the still-readable canonical event log. Derived bytes remain stable
only while the semantic engine and state-format versions are unchanged.

The compatibility boundary is deliberate. Canonical recording remains event-log
version 6, while rich semantic projections require state format 5. Generated
Python and TypeScript clients move in lockstep with driver protocol 5; there is
no legacy screen-envelope shim. Public package versions remain unchanged until
the coordinated release is published.


## Native visual artifacts

Running `termtrace artifact capture --session SESSION` creates:

| File | Contents |
| --- | --- |
| `visual/session.mkv` | lossless RGB H.264 video at the configured 24, 30, or 60 FPS (24 FPS by default) |
| `visual/final.png` | stable final native frame |
| `visual/video-map.jsonl` | semantic-state to nominal-frame mapping |
| `visual/ghostty-effective-config.txt` | Ghostty-reported effective output retained for fingerprint diagnostics |
| `visual/manifest.json` | source, geometry, effective-output fingerprint, clocks, probes, and member hashes |

Native capture is a separate capability. It never silently falls back to
semantic rendering. The current native visual-manifest version is 5. Its
version bump is intentional: the manifest identifies the effective output
reported after loading the chosen replay input as a distinct representation.

## Presentation artifacts

`termtrace video SESSION --out FILE.mp4` is the normal facade. It writes the new
MP4 and retains a manifested `.termtrace` sidecar containing the resolved render
configuration and provenance. Presets are `docs`, `social`, and `lossless`.
`--renderer native` is the default, requires a verified native capture, and
never falls back to semantic rendering. Select `--renderer semantic` explicitly
when deterministic semantic replay is the intended source. Frame rate,
resolution or scale, marker range, idle trimming, endpoint holds, and cursor
treatment can be selected explicitly.

The `termtrace video` command can print the resolved plan with `--dry-run`,
render a review artifact with `--preview-out DIRECTORY`, or export an MP4 with
`--out FILE.mp4`. Exports retain the normalized plan, output probe, per-frame
presentation map, and manifest in the `.termtrace` sidecar.

With `--renderer semantic`, the sidecar also retains:

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

Generation replaces existing files with known generated schema names and leaves
unrelated files untouched. Tests require generated output to match the checked-in
files and validate fixtures against the schemas. Schema validity describes
structure, not trust or safety.

## Export and handoff

`termtrace artifact import --input FILE --out SESSION` converts an ANSI/VT byte
stream into canonical chained evidence; use `--input -` for standard input.

`termtrace artifact asciicast` creates a lossy UTF-8 interoperability view.
Invalid bytes, exact timing, and advanced semantic state cannot be preserved, so
the cast is never canonical evidence.

`termtrace screenshot SESSION -o DIRECTORY` writes a manifested semantic PNG
by default and also supports exact selectors, screenshot batches, SVG, and
explicit native rendering. `termtrace bundle SESSION -o DIRECTORY` publishes
the least-sensitive manifested screen handoff and supports `debug` and
`forensic` profiles when their additional sensitivity is authorized.
`termtrace bug-report SESSION -o DIRECTORY [--screenshot]` publishes a bounded
manifest containing redacted metadata, the last screen, last-action analysis,
diagnostics, and a canonical session reference. It omits raw input/output event
streams by default.

`termtrace artifact save` creates selected multi-format representations.
Bundle manifests are limited to 1 MiB and 10,000 payload members. Verify a
bundle with `termtrace integrity verify-bundle BUNDLE --source SESSION
--profile PROFILE`; member sensitivity, relationship, and schema labels are
derived by the verifier rather than accepted from the bundle.

Terminal content remains untrusted and potentially sensitive in every
representation.
