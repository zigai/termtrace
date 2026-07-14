# Architecture

## Truth layers

Termtrace keeps four truths separate so one representation never silently substitutes for another:

1. **Execution truth:** ordered PTY bytes, input, resize, signal, marker, lifecycle, and exit events.
2. **Semantic truth:** terminal states reconstructed by the same `libghostty-vt` engine used by Ghostty.
3. **Native visual truth:** pixels drawn by the installed Ghostty GTK/OpenGL application on a headless display.
4. **Encoded visual truth:** screenshots and video produced from native visual capture.

The append-only execution log is canonical. Every other artifact records the event range, clock mapping, component versions, configuration hash, and profile from which it was derived.

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

The target process starts in the recorder's effective current directory unless `--cwd` overrides it. Termtrace resolves that directory once before creating provenance or spawning the PTY child, so the manifest and process cannot disagree.

The pinned libghostty state machine runs live as the terminal host. Its write-PTY callback answers status, cursor, and mode queries; each exact query/response is logged before the response is delivered through a host input intent/result. This keeps detached TUIs functional without conflating generated responses with human or agent input.

The event header pins the effective foreground, background, cursor color, and 256-color palette. Recording reads these from `ghostty +show-config` when available and records a configuration hash as provenance; pinned libghostty defaults are an explicit fallback. Replay never substitutes the current machine's theme.

Geometry can be explicit, inherited from the controlling terminal, or fitted to a target capture viewport. Fitting launches the real Ghostty application on Xvfb and uses XTWINOPS reports for grid size, cell pixels, and text-area pixels. The resulting geometry, viewport, source, and effective-config hash are immutable session provenance. Native playback applies the initial grid before event time zero and verifies the live report; it never guesses font metrics from point size.

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
