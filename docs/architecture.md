# Architecture

## Truth layers

Termtrace keeps four truths separate so one representation never silently substitutes for another:

1. **Execution truth:** ordered PTY bytes, input, resize, signal, marker, lifecycle, and exit events.
2. **Semantic truth:** terminal states reconstructed by the same `libghostty-vt` engine used by Ghostty.
3. **Native visual truth:** pixels drawn by the installed Ghostty GTK/OpenGL application on a headless display.
4. **Encoded visual truth:** screenshots and video produced from native visual capture.

The append-only execution log is canonical. Every other artifact records the event range, clock mapping, component versions, configuration hash, and profile from which it was derived.

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

Every input, resize, and signal uses a unique intent/result pair. The intent is durably written before the syscall; the result records complete application or a stable failure code. An intent left without a result after interruption is reported as uncertain and is never retried automatically. Input results also carry the exact number of bytes written so partial writes cannot masquerade as successful delivery.

tmux is supported as a target command but is not required. Asciicast is an interoperability export and is never the canonical recording.

## Event and state granularity

PTY reads are persisted without idle coalescing using monotonic high-resolution timestamps and ordered sequence numbers. Offline replay may feed bytes incrementally through `libghostty-vt` and emits a state only when terminal render state changes.

Synchronized output mode is represented explicitly. Intermediate mutations inside a synchronized-output transaction may be retained for protocol diagnosis but are marked as not presented; the committed state is emitted when synchronization ends.

Video sampling does not limit semantic capture. A state that exists between video frames remains inspectable and can be rendered as a derived screenshot.

## Dependency boundary

`libghostty-vt` is pinned behind a narrow internal adapter. Its unstable upstream API must not leak into Termtrace's public artifact or control schemas. Upgrades require replay fixtures and semantic-diff review.

The current pin is Ghostty `55a3e33ab26a23d75b274b23c7f76d837db00578`, built with Zig 0.15.2. Build-time bindings remain private; stable serializable Rust structures form the artifact boundary.

The Linux Ghostty GUI currently has no stable headless framebuffer or screenshot API. Native pixels are therefore captured from a real Ghostty process on a virtual display rather than recreated by Termtrace.

## Failure policy

- Failure to generate a derived artifact never deletes or rewrites the raw recording.
- Event-log write failures stop further state-changing input rather than creating an unauditable gap.
- PTY writes are not retried automatically because duplicate input is unsafe.
- Safe connection and process-readiness probes may use bounded retries.
- Cleanup failures are reported alongside, but never replace, the primary failure.
- Public failures carry stable diagnostic codes and structured context.
