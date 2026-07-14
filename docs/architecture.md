# Architecture

## Truth layers

Termtrace keeps four truths separate so one representation never silently substitutes for another:

1. **Execution truth:** ordered PTY bytes, input, resize, signal, marker, lifecycle, and exit events.
2. **Semantic truth:** terminal states reconstructed by the same `libghostty-vt` engine used by Ghostty.
3. **Native visual truth:** pixels drawn by the installed Ghostty GTK/OpenGL application on a headless display.
4. **Encoded visual truth:** screenshots and video produced from native visual capture.

The append-only execution log is canonical. The immutable session manifest is published first and its exact BLAKE3 is committed by the event header. The completion manifest binds both roots, requires a final `Exit`, rejects pending mutations, and agrees with the final event hash, event count, output count, and exit outcome. New files and directories are synchronized with their parent directory at publication boundaries. This provides tamper detection relative to retained roots, not recorder authentication. Every derived artifact records the event range, clock mapping, component versions, configuration hash, profile, and whole-file member hashes from which it was derived. Completed derivation manifests are chained in `artifact-roots.jsonl`; this post-completion catalog never reopens canonical execution truth and remains locally self-consistent rather than authenticated until bound by a trusted detached checkpoint. Explicit live snapshots are source-hash-addressed and marked incomplete rather than being allowed to masquerade as final artifacts.

Detached receipt signing is a narrow external boundary. Termtrace assembles and domain-separates fixed-order canonical payload bytes, then delegates signing and verification to OpenSSH; it never stores or interprets private keys. Cryptographic validity proves that the embedded signing key covered those bytes. Authenticity additionally requires the caller's allowed-signers policy to bind that key to the asserted identity. The local signing-host timestamp is labeled accordingly and is not described as witnessed time or external anchoring.

Friendly names are resolved through a separate owner-only registry boundary. Entries are no-replace files opened without following symlinks and contain both exact canonical path bytes and the immutable manifest session ID. Every resolution validates ownership, permissions, canonical path stability, supported format/protocol, and manifest identity. Aliases therefore remain mutable navigation metadata rather than provenance roots.

Live registration records socket peer credentials plus process-start ticks, preventing a reused PID from making a stale alias appear healthy. Registry listing derives lifecycle from that pair, socket presence, immutable manifest identity, and completion identity without appending canonical observations. Removing an alias validates and unlinks only its owner-only entry, then synchronizes the registry directory; it has no evidence-deletion path.

Detached start reuses the exact foreground recording option grammar. It allocates a collision-free absent session path, redirects mirrored output and recorder diagnostics to separate owner-only files, launches the recorder with standard input closed, and waits for both immutable manifest publication and a socket-derived recorder PID/start pair. Only then is the alias published. Duplicate aliases fail before allocation; process failure, readiness timeout, or a publication race reports the recoverable orphan and diagnostic paths without deleting evidence.

Restart first drives the current alias owner to validated completion, then creates a separate successor from the predecessor's effective settings. The successor manifest binds the predecessor session ID, exact manifest hash, and final event-chain root. Registry mutation is serialized by an owner-only file lock and uses compare-and-swap on the predecessor ID plus an atomic rename, so readiness failure or a concurrent owner change leaves the previous binding intact. Neither path is reused or truncated.

Full-session derivation preserves the original `derived/` and `visual/` layouts. A requested half-open interval receives deterministic range-specific directories. Semantic replay reconstructs the prefix without materializing prefix states. Native playback emits the prefix without source-time sleeps, performs a non-mutating terminal-query barrier, and publishes its capture clock only after Ghostty has processed that prefix. FFmpeg therefore records neither the prefix nor its original elapsed time.

## Runtime topology

```text
agent control socket ─┐
human/Ghostty input ──┼─> PTY proxy ─> target process
                     │      │
                     │      ├─> owner-only event log
                     │      └─> live semantic state
                     │
Ghostty on Xvfb <─────┘
       │
       └─> FFmpeg native-window capture
```

The proxy forwards bytes unchanged. It owns the target PTY, logs before acknowledging state-changing control requests, propagates window changes, and distinguishes input originating from a human terminal, an agent, or an automatic host response.

Read-only control operations are observations rather than execution events. Ping, status, wait registration/polling, and live boundary discovery return actor-consistent `observed` and `durable` positions without changing the event count or hash. Mutations and explicit markers remain canonical.

The actor retains the latest presentation-eligible semantic snapshot separately from its newest parser state. Outside synchronized output it advances after output and resize processing; while DEC mode 2026 is active it remains frozen until commit. Its observer-only endpoint returns bounded viewport text, cursor/mode facts, a full-state hash, and canonical source boundaries without serializing the full cell graph over the control socket.

Presented-screen predicates register as bounded actor waiters with server-side monotonic deadlines. Matching-state hashes and stability clocks advance on actor ticks without blocking the event loop, so output, input, status, stop, query responses, and other waiters continue concurrently. Wait registration and completion remain observations and never append canonical events.

Mutation idempotency uses a bounded in-memory cache backed by the append-only, hash-chained `requests.jsonl` journal. Each record binds the request UUID and command hash to its operation IDs, canonical event range and hash, and exact response. Cache eviction therefore does not permit a duplicate mutation. Read-only observations are excluded, and a per-session mutation limit bounds journal growth. The journal is disposable acceleration: verification cross-checks it against canonical events when present, but missing or invalid journal data cannot invalidate otherwise complete canonical evidence.

Output persistence is explicit provenance. `strict` synchronizes every PTY output event, `bounded` synchronizes when either its configured byte limit or elapsed-time limit is reached, and `throughput` buffers output until a durable mutation/lifecycle/completion boundary. The actor checks bounded elapsed deadlines even while the child is idle. Completion reports measured maxima and final synchronization; this is syscall-level evidence under the recorded host assumptions, not offline proof of power-loss survival.

The target process starts in the recorder's effective current directory unless `--cwd` overrides it. Termtrace resolves that directory once before creating provenance or spawning the PTY child, so the manifest and process cannot disagree.

Execution environment collection is policy-driven. `minimal` retains recorder-owned terminal values and inventories common locale/timezone names, `reproducible` retains those effective values and hashes, and `custom` applies explicit per-variable value/hash/name/not-captured rules. The immutable manifest records both the selection policy and the resulting inventory; hashes support later equality checks but are not presented as reproducible values or confidentiality controls.

Executable resolution happens against the same effective working directory and inherited `PATH` used for the child. Reproducible policy streams complete-file hashes for the resolved executable and recorder binary. A direct script is included only for a recognized interpreter with an unambiguous first file argument; unresolved, unreadable, and unrequested identities remain distinct. Runtime UID/GID, privilege, umask, kernel, build, and semantic-engine facts explain the execution host without using a hostname as identity.

The pinned libghostty state machine runs live as the terminal host. Its write-PTY callback answers status, cursor, and mode queries; each exact query/response is logged before the response is delivered through a host input intent/result. This keeps detached TUIs functional without conflating generated responses with human or agent input.

The event header pins the effective foreground, background, cursor color, and 256-color palette. Recording reads these from `ghostty +show-config` when available and records a configuration hash as provenance; pinned libghostty defaults are an explicit fallback. Replay never substitutes the current machine's theme.

Geometry can be explicit, inherited from the controlling terminal, or fitted to a target capture viewport. Fitting launches the real Ghostty application on Xvfb and uses XTWINOPS reports for grid size, cell pixels, and text-area pixels. The resulting geometry, viewport, source, and effective-config hash are immutable session provenance. Native capture first runs a parent-observed probe on its actual viewport and rejects grid/text mismatch before timed playback. The playback child also publishes a structured failure handshake. Native playback applies and verifies the initial grid before event time zero; it never guesses font metrics from point size.

A recording may replace the normal Ghostty user config with `--ghostty-config` and then apply focused font, padding, foreground, and background overrides. Termtrace resolves recursive config files, applies those overrides last, validates the result through Ghostty, strips recursive file controls, and stores a canonical effective profile in the session. Geometry fitting, recorded terminal colors, and native capture all consume that same hashed profile. Referenced resources such as fonts and shaders remain external host dependencies.

Every input, resize, and signal uses a unique intent/result pair. The intent is durably written before the syscall; the result records complete application or a stable failure code. An intent left without a result after interruption is reported as uncertain and is never retried automatically. Input results also carry the exact number of bytes written so partial writes cannot masquerade as successful delivery.

tmux is supported as a target command but is not required. Asciicast is an interoperability export and is never the canonical recording.

## Event and state granularity

PTY reads are persisted without idle coalescing using monotonic high-resolution timestamps and ordered sequence numbers. Offline replay may feed bytes incrementally through `libghostty-vt` and emits a state only when terminal render state changes.

Synchronized output mode is represented explicitly. Intermediate mutations inside a synchronized-output transaction may be retained for protocol diagnosis but are marked as not presented; the committed state is emitted when synchronization ends.

Video sampling does not limit semantic capture. A state that exists between video frames remains inspectable and can be rendered as a derived screenshot.

## Dependency boundary

`libghostty-vt` is pinned behind a narrow internal adapter. Its unstable upstream API must not leak into Termtrace's public artifact or control schemas. Upgrades require replay fixtures and semantic-diff review.

The current pin is Ghostty `55a3e33ab26a23d75b274b23c7f76d837db00578`, built with Zig 0.15.2. Checked-in generated bindings remain private and are refreshed only during an intentional Ghostty upgrade; stable serializable Rust structures form the artifact boundary.

The Linux Ghostty GUI currently has no stable headless framebuffer or screenshot API. Native pixels are therefore captured from a real Ghostty process on a virtual display rather than recreated by Termtrace.

## Failure policy

- Failure to generate a derived artifact never deletes or rewrites the raw recording.
- Event-log write failures stop further state-changing input rather than creating an unauditable gap.
- PTY writes are not retried automatically because duplicate input is unsafe.
- Safe connection and process-readiness probes may use bounded retries.
- Cleanup failures are reported alongside, but never replace, the primary failure.
- Public failures carry stable diagnostic codes and structured context.
