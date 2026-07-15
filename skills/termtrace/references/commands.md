# Generated Termtrace command reference

Generated from the current tested command graph. Prefer `termtrace COMMAND --help` when using a different binary version.

## `start`

```text
Start a detached recording and publish a name after readiness

Usage: termtrace start [OPTIONS] <NAME> <COMMAND>...

Arguments:
  <NAME>        New friendly session name
  <COMMAND>...  Executable and arguments; options must precede the executable

Options:
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --startup-timeout-ms <STARTUP_TIMEOUT_MS>
          Maximum time to wait for manifest and control readiness [default: 10000]
      --cwd <CWD>
          Child working directory; defaults to the caller's current directory
      --geometry <GEOMETRY>
          Geometry selection policy [default: manual] [possible values: manual, inherit, fit]
      --viewport <VIEWPORT>
          Intended capture viewport as WIDTHxHEIGHT; required for fitted geometry
      --ghostty-config <GHOSTTY_CONFIG>
          Ghostty config file used instead of the default user config
      --font-family <FONT_FAMILY>
          Override the regular Ghostty font family
      --font-size <FONT_SIZE>
          Override the Ghostty font size in points
      --padding-x <PADDING_X>
          Override symmetric Ghostty horizontal padding in pixels
      --padding-y <PADDING_Y>
          Override symmetric Ghostty vertical padding in pixels
      --foreground <FOREGROUND>
          Override the Ghostty foreground color as #RRGGBB
      --background <BACKGROUND>
          Override the Ghostty background color as #RRGGBB
      --cols <COLS>
          Terminal columns used by manual geometry [default: 160]
      --rows <ROWS>
          Terminal rows used by manual geometry [default: 40]
      --cell-width <CELL_WIDTH>
          Manual cell width, or inherited-geometry fallback, in pixels [default: 9]
      --cell-height <CELL_HEIGHT>
          Manual cell height, or inherited-geometry fallback, in pixels [default: 18]
      --term <TERM>
          Terminal identity exposed to the child [default: xterm-ghostty]
      --durability <DURABILITY>
          Canonical output persistence policy [default: bounded] [possible values: strict, bounded, throughput]
      --durability-max-interval-ms <DURABILITY_MAX_INTERVAL_MS>
          Maximum unsynchronized interval in bounded mode [default: 50]
      --durability-max-uncommitted-bytes <DURABILITY_MAX_UNCOMMITTED_BYTES>
          Maximum unsynchronized PTY bytes in bounded mode [default: 65536]
      --provenance <PROVENANCE>
          Execution provenance collection policy [default: minimal] [possible values: minimal, reproducible, custom]
      --provenance-env <PROVENANCE_ENVIRONMENT>
          Custom environment rule as NAME=value|hash|name|not-captured
  -h, --help
          Print help
```

## `status`

```text
Print machine-readable live session status

Usage: termtrace status [OPTIONS] <SESSION>

Arguments:
  <SESSION>  Session name or canonical path

Options:
      --log <LOG>                [env: TERMTRACE_LOG=] [default: warn]
      --wait-complete            Wait for durable completion instead of returning live counters
      --timeout-ms <TIMEOUT_MS>  Maximum completion wait in milliseconds [default: 30000]
  -h, --help                     Print help
```

## `send`

```text
Send text or file bytes to a live child PTY

Usage: termtrace send [OPTIONS] <ATOM|--stdin> <SESSION>

Arguments:
  <SESSION>  Live session name or canonical path
  [ATOM]...  Named keys, text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH atoms

Options:
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --stdin
          Read one exact raw burst from standard input
      --pace <PACE>
          Delay between atoms; paced text is split by Unicode scalar [default: 0ms]
      --intent <INTENT>
          Human-readable reason retained with the input event
      --if-state <EXPECTED_PRESENTED_STATE_HASH>
          Apply every input atom only if the latest presented state has this BLAKE3 hash
  -h, --help
          Print help
```

## `wait`

```text
Wait for exact UTF-8 text in canonical PTY output

Usage: termtrace wait [OPTIONS] <SCREEN|--output <OUTPUT>|--screen-absent <SCREEN_ABSENT>|--screen-regex <SCREEN_REGEX>|--row-contains <ROW_CONTAINS>|--region-contains <REGION_CONTAINS>|--cursor-at <CURSOR_AT>|--cursor-visible <CURSOR_VISIBLE>|--cursor-shape <CURSOR_SHAPE>|--mode <MODE>|--mode-disabled <MODE_DISABLED>|--state-changed <STATE_CHANGED>|--state-unchanged <STATE_UNCHANGED>> <SESSION>

Arguments:
  <SESSION>  Live session name or canonical path
  [SCREEN]   Exact UTF-8 text required in the latest presented viewport

Options:
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --output <OUTPUT>
          Exact UTF-8 text to find in canonical output history
      --screen-absent <SCREEN_ABSENT>
          Exact UTF-8 text required to be absent from the presented viewport
      --screen-regex <SCREEN_REGEX>
          Bounded linear-time regular expression required in the viewport
      --row-contains <ROW_CONTAINS>
          Required row substring as `ROW:TEXT`, with a zero-based row
      --region-contains <REGION_CONTAINS>
          Required region substring as `LEFT,TOP,RIGHT,BOTTOM:TEXT` with half-open bounds
      --cursor-at <CURSOR_AT>
          Require the cursor at zero-based `COLUMN,ROW`
      --cursor-visible <CURSOR_VISIBLE>
          Require visible (`true`) or hidden (`false`) cursor state [possible values: true, false]
      --cursor-shape <CURSOR_SHAPE>
          Require one cursor shape [possible values: bar, block, underline, hollow-block]
      --mode <MODE>
          Require one stable terminal mode to be enabled [possible values: alternate-screen, bracketed-paste, wraparound, synchronized-output]
      --mode-disabled <MODE_DISABLED>
          Require one stable terminal mode to be disabled [possible values: alternate-screen, bracketed-paste, wraparound, synchronized-output]
      --state-changed <STATE_CHANGED>
          Require the presented state to differ from this BLAKE3 hash
      --state-unchanged <STATE_UNCHANGED>
          Require the presented state to equal this BLAKE3 hash
      --stable-for <STABLE_FOR>
          Require an unchanged matching presented state for this duration [default: 0ms]
      --timeout-ms <TIMEOUT_MS>
          Maximum wait duration in milliseconds [default: 30000]
  -h, --help
          Print help
```

## `show`

```text
Show a truth-labeled live or completed terminal viewport

Usage: termtrace show [OPTIONS] <SESSION|--input <INPUT>|COMMAND>

Arguments:
  [SESSION]     Live session name or canonical completed session path
  [COMMAND]...  One-off command to record to completion, following `--`

Options:
      --input <INPUT>            ANSI/VT byte stream path, or `-` for standard input
      --log <LOG>                [env: TERMTRACE_LOG=] [default: warn]
      --record-out <RECORD_OUT>  Promote an ANSI input stream into a new canonical import session
      --cols <COLS>              ANSI stream columns [default: 80]
      --rows <ROWS>              ANSI stream rows [default: 24]
      --out <OUT>                New canonical evidence path for a one-off command
      --discard-evidence         Remove auto-allocated one-off evidence only after successful output
      --state <STATE>            Exact semantic state identifier
      --event-seq <EVENT_SEQ>    Latest state at or before this event sequence
      --at <AT>                  Latest state at or before this session offset
      --at-marker <AT_MARKER>    State at a unique marker label
      --include-hidden           Permit an explicitly selected synchronized-output hidden state
      --stable-for <STABLE_FOR>  Require an unchanged live presented state for this duration [default: 0ms]
      --timeout-ms <TIMEOUT_MS>  Server-side deadline for a live stability request [default: 30000]
      --metadata                 Write a concise provenance header to standard error
      --renderer <RENDERER>      Renderer used by image formats; native never falls back to semantic [default: semantic] [possible values: semantic, native]
      --manifest <MANIFEST>      Optional no-replace sidecar manifest for binary image output
      --recover-prefix           Explicitly observe an integrity-checked incomplete event prefix offline
      --wait <WAIT>              For one-off commands, capture when exact presented text appears, then stop
      --wait-regex <WAIT_REGEX>  For one-off commands, capture when a bounded presented regex matches, then stop
      --detach                   Explicitly leave a one-off recorder running and return its canonical path
      --format <FORMAT>          Output representation [default: text] [possible values: text, json, json-compact, frame-json, vt, html, svg, png]
  -h, --help                     Print help
```

## `output`

```text
Write exact canonical child PTY output bytes

Usage: termtrace output [OPTIONS] <--raw|--strip-controls> <SESSION>

Arguments:
  <SESSION>  Completed session name or canonical path

Options:
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --raw
          Write exact concatenated child PTY bytes without decoding
      --strip-controls
          Emit a named lossy projection retaining printable bytes and line controls
      --manifest <MANIFEST>
          New no-replace JSON report path; unavailable while following
      --follow
          Continue emitting newly durable output until the session completes
      --from-event-seq <FROM_EVENT_SEQ>
          Inclusive canonical event sequence
      --to-event-seq <TO_EVENT_SEQ>
          Exclusive canonical event sequence
      --from <OFFSET>
          Inclusive source offset, such as `90s`, `2m`, or `00:01:30.5`
      --from-marker <LABEL>
          Inclusive source marker; the label must occur exactly once
      --to <OFFSET>
          Exclusive source offset, in the same format as `--from`
      --to-marker <LABEL>
          Exclusive source marker; the label must occur exactly once
  -h, --help
          Print help
```

## `events`

```text
Stream filtered canonical event projections as JSONL

Usage: termtrace events [OPTIONS] <SESSION>

Arguments:
  <SESSION>  Completed session name or canonical path

Options:
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --type <EVENT_TYPES>
          Comma-separated canonical event variant names
      --metadata-only
          Elide exact byte payloads while retaining their lengths and hashes
      --follow
          Continue streaming newly durable records until completion
      --from-event-seq <FROM_EVENT_SEQ>
          Inclusive canonical event sequence
      --to-event-seq <TO_EVENT_SEQ>
          Exclusive canonical event sequence
      --from <OFFSET>
          Inclusive source offset, such as `90s`, `2m`, or `00:01:30.5`
      --from-marker <LABEL>
          Inclusive source marker; the label must occur exactly once
      --to <OFFSET>
          Exclusive source offset, in the same format as `--from`
      --to-marker <LABEL>
          Exclusive source marker; the label must occur exactly once
  -h, --help
          Print help
```

## `mark`

```text
Add a marker to a live session timeline

Usage: termtrace mark [OPTIONS] --label <LABEL> <SESSION>

Arguments:
  <SESSION>  Live session name or canonical path

Options:
      --label <LABEL>
          Marker label
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --if-state <EXPECTED_PRESENTED_STATE_HASH>
          Apply only if the actor's latest presented state has this BLAKE3 hash
  -h, --help
          Print help
```

## `timeline`

```text
List canonical operations and semantic state transitions in order

Usage: termtrace timeline [OPTIONS] <SESSION>

Arguments:
  <SESSION>  Completed session name or canonical path

Options:
      --all-states           Include synchronized-output states that were not presentation eligible
      --log <LOG>            [env: TERMTRACE_LOG=] [default: warn]
      --controls             Include lossless framed VT controls and printable runs
      --jsonl                Emit one stable JSON object per line
      --from <OFFSET>        Inclusive source offset, such as `90s`, `2m`, or `00:01:30.5`
      --from-marker <LABEL>  Inclusive source marker; the label must occur exactly once
      --to <OFFSET>          Exclusive source offset, in the same format as `--from`
      --to-marker <LABEL>    Exclusive source marker; the label must occur exactly once
  -h, --help                 Print help
```

## `diff`

```text
Compare two canonical semantic state points

Usage: termtrace diff [OPTIONS] <--from-state <FROM_STATE>|--from-event-seq <FROM_EVENT_SEQ>|--from-at <FROM_AT>|--from-marker <FROM_MARKER>> <--to-state <TO_STATE>|--to-event-seq <TO_EVENT_SEQ>|--to-at <TO_AT>|--to-marker <TO_MARKER>> <SESSION>

Arguments:
  <SESSION>  Completed session name or canonical path

Options:
      --from-state <FROM_STATE>
          Earlier exact semantic state identifier
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --from-event-seq <FROM_EVENT_SEQ>
          Earlier state at or before this event sequence
      --from-at <FROM_AT>
          Earlier state at or before this session offset
      --from-marker <FROM_MARKER>
          Earlier state at a unique marker
      --to-state <TO_STATE>
          Later exact semantic state identifier
      --to-event-seq <TO_EVENT_SEQ>
          Later state at or before this event sequence
      --to-at <TO_AT>
          Later state at or before this session offset
      --to-marker <TO_MARKER>
          Later state at a unique marker
      --expect-no-change
          Fail when any semantic or presentation value changed
      --format <FORMAT>
          Output representation [default: text] [possible values: text, json, json-compact, svg, png]
  -h, --help
          Print help
```

## `explain`

```text
Explain exact VT and semantic evidence around one state point

Usage: termtrace explain [OPTIONS] <--state <STATE>|--event-seq <EVENT_SEQ>|--at <AT>|--marker <MARKER>> <SESSION>

Arguments:
  <SESSION>  Completed session name or canonical path

Options:
      --log <LOG>              [env: TERMTRACE_LOG=] [default: warn]
      --state <STATE>          Exact semantic state identifier
      --event-seq <EVENT_SEQ>  Latest state at or before this event sequence
      --at <AT>                Latest state at or before this session offset
      --marker <MARKER>        State at a unique marker
      --context <CONTEXT>      Canonical event radius before and after the target [default: 8]
      --format <FORMAT>        Output representation [default: text] [possible values: text, json, json-compact]
  -h, --help                   Print help
```

## `diagnose`

```text
Run conservative evidence-backed rendering diagnostics

Usage: termtrace diagnose [OPTIONS] <SESSION>

Arguments:
  <SESSION>  Completed session name or canonical path

Options:
      --check <CHECK>
          Comma-separated detector groups; omit to run every detector
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --broad-change-percent <BROAD_CHANGE_PERCENT>
          Minimum changed-cell percentage for a broad repaint [default: 50]
      --json
          Emit one stable structured report
      --fail-on-finding
          Exit unsuccessfully when at least one finding is emitted
  -h, --help
          Print help
```

## `screenshot`

```text
Render one deterministic semantic screenshot artifact directory

Usage: termtrace screenshot [OPTIONS] --out <OUT> <SESSION>

Arguments:
  <SESSION>  Completed session name or canonical path

Options:
      --log <LOG>              [env: TERMTRACE_LOG=] [default: warn]
      --out <OUT>              New artifact directory containing `screen.svg` and its manifest
      --state <STATE>          Exact semantic state identifier
      --event-seq <EVENT_SEQ>  Latest state at or before this event sequence
      --at <AT>                Latest state at or before this session offset
      --at-marker <AT_MARKER>  State at a unique marker
      --include-hidden         Permit an explicitly selected synchronized-output hidden state
      --format <FORMAT>        Semantic image representation [default: png] [possible values: svg, png]
      --renderer <RENDERER>    Semantic state renderer or actual Ghostty native frame [default: semantic] [possible values: semantic, native]
      --viewport <VIEWPORT>    Strict whole native viewport as `WIDTHxHEIGHT`
      --fit-viewport           Explicitly request the default exact-grid viewport solver
      --strict                 Emphasize that a supplied viewport is an exact evidentiary assertion
  -h, --help                   Print help
```

## `stop`

```text
Request graceful termination of a live session child

Usage: termtrace stop [OPTIONS] <SESSION>

Arguments:
  <SESSION>  Live session name or canonical path

Options:
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --wait
          Wait for and validate durable completion after stop is acknowledged
      --timeout-ms <TIMEOUT_MS>
          Maximum completion wait in milliseconds [default: 30000]
      --if-state <EXPECTED_PRESENTED_STATE_HASH>
          Apply only if the actor's latest presented state has this BLAKE3 hash
  -h, --help
          Print help
```

## `verify`

```text
Verify a completed session's structure and evidence bindings offline

Usage: termtrace verify [OPTIONS] <SESSION>

Arguments:
  <SESSION>  Completed canonical session directory

Options:
      --level <LEVEL>
          Verification depth [default: content] [possible values: structure, content, durability-record, signature]
      --log <LOG>
          [env: TERMTRACE_LOG=] [default: warn]
      --json
          Emit the stable machine-readable report
      --allowed-signers <ALLOWED_SIGNERS>
          OpenSSH allowed-signers file used to establish signer trust
  -h, --help
          Print help
```

## `bundle`

```text
Publish a deterministic sensitivity-labeled handoff directory

Usage: termtrace bundle [OPTIONS] --out <OUT> <SESSION>

Arguments:
  <SESSION>  Completed canonical session

Options:
      --log <LOG>          [env: TERMTRACE_LOG=] [default: warn]
      --profile <PROFILE>  Fixed content and sensitivity policy [default: screen] [possible values: screen, debug, forensic]
      --out <OUT>          New owner-only destination directory
      --dry-run            Print the selected paths and sensitivities without writing
      --archive            Publish a deterministic compressed archive instead of a directory
  -h, --help               Print help
```

## `test`

```text
Execute one strict evidence-native terminal scenario

Usage: termtrace test [OPTIONS] [SCENARIO]...

Arguments:
  [SCENARIO]...  One or more strict versioned scenario JSON documents

Options:
      --artifacts <ARTIFACTS>      Owner-only result, failure, and canonical evidence root [default: termtrace-results]
      --log <LOG>                  [env: TERMTRACE_LOG=] [default: warn]
      --update-snapshots           Explicitly replace normalized semantic frame baselines
      --junit <JUNIT>              New JUnit XML result path
      --jobs <JOBS>                Maximum concurrent fresh-session workers [default: 1]
      --shard-index <SHARD_INDEX>  Zero-based deterministic shard index
      --shard-count <SHARD_COUNT>  Total deterministic shard count
      --reruns <RERUNS>            Fresh-session reruns after a failed attempt [default: 0]
  -h, --help                       Print help
```

## `review`

```text
Navigate validated evidence in a read-only semantic terminal UI

Usage: termtrace review [OPTIONS] [SESSION]

Arguments:
  [SESSION]  Completed canonical session or registered name

Options:
      --around-marker <AROUND_MARKER>  Start at the first semantic state at or after this unique marker
      --log <LOG>                      [env: TERMTRACE_LOG=] [default: warn]
      --search <SEARCH>                Exact semantic viewport substring to find and navigate with n/N
      --export-state <EXPORT_STATE>    Export one exact semantic state to a new owner-only directory
      --state <STATE>                  Exact state identifier for `--export-state`
      --commands                       Print scriptable reviewer commands and safety policy
      --json                           Print a validated read-only source summary without opening a TUI
  -h, --help                           Print help
```

## `markers`

```text
List canonical marker positions and nominal source frames

Usage: termtrace markers [OPTIONS] <SESSION>

Arguments:
  <SESSION>  Completed canonical session

Options:
      --fps <FPS>  Nominal frame rate used only for source-time mapping [default: 24]
      --log <LOG>  [env: TERMTRACE_LOG=] [default: warn]
  -h, --help       Print help
```

## `video`

```text
Plan or export presentation edits over immutable native evidence

Usage: termtrace video [OPTIONS] <COMMAND>

Commands:
  plan     Resolve clips, speeds, and holds without rendering
  preview  Render an atomic contact sheet and low-resolution review video
  export   Export an H.264 MP4 and frame provenance into a new directory
  help     Print this message or the help of the given subcommand(s)

Options:
      --log <LOG>  [env: TERMTRACE_LOG=] [default: warn]
  -h, --help       Print help
```
