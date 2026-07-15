# Recording sessions

`termtrace record` runs a command in a PTY and writes a new owner-only session directory.

```bash
termtrace record --out ./example.termtrace --geometry inherit your-command --argument
```

The executable and its arguments follow the recording options. They are passed as an argument vector, not through a shell. Output is mirrored to the current terminal and standard input is forwarded unless `--no-input` is set.

Termtrace refuses to reuse an existing output path.

## Choose terminal geometry

Geometry affects line wrapping and every derived terminal state.

### Inherit the current terminal

Use this for interactive recordings:

```bash
termtrace record --out SESSION --geometry inherit COMMAND
```

### Set a fixed grid

Use manual geometry for deterministic semantic replay without native capture:

```bash
termtrace record --out SESSION --geometry manual --cols 120 --rows 36 --cell-width 9 --cell-height 18 COMMAND
```

### Fit a capture viewport

Use fitted geometry when the recording will become a native screenshot or video:

```bash
termtrace record --out SESSION --geometry fit --viewport 1920x1080 COMMAND
```

This probes the installed terminal application and records the measured grid, cell size, viewport, and effective configuration hash. It requires the native capture dependencies reported by `termtrace doctor`.

Use `--ghostty-config` to select a configuration file. Font, padding, foreground, and background flags override that file. The flattened effective configuration is saved in the session so later capture uses the same profile. Referenced fonts, shaders, and images remain external dependencies.

## Working directory

The child starts in the caller's current directory. Override it with `--cwd`; relative paths are resolved from the caller's directory.

```bash
termtrace record --out SESSION --cwd ./project cargo test
```

The resolved directory is stored in `session.json`.

## Provenance

Use `--provenance` to choose how much execution context is retained:

| Policy | Behavior |
| --- | --- |
| `minimal` | records required terminal values and inventories locale or timezone names |
| `reproducible` | also retains effective values and hashes relevant executable files |
| `custom` | applies explicit `--provenance-env NAME=RULE` rules |

Custom rules are `value`, `hash`, `name`, or `not-captured`. A hash supports equality checks; it is not a secret-protection mechanism and does not preserve the original value.

## Durability

The default `bounded` policy synchronizes output after 50 ms or 65,536 uncommitted bytes. Choose another policy only when its tradeoff is intentional:

| Policy | Synchronization |
| --- | --- |
| `strict` | after every PTY output event |
| `bounded` | at configured time or byte limits |
| `throughput` | at mutation, lifecycle, and completion boundaries |

The completion manifest records the selected policy and observed maxima. It describes recorder behavior; it cannot prove that historical writes reached stable hardware.

## Complete the recording

A command that exits normally completes its own session. For a detached or controlled workload, request graceful termination and wait for durable completion:

```bash
termtrace stop SESSION --wait
```

Use `termtrace status SESSION --wait-complete` to wait without sending a signal.

After completion:

```bash
termtrace index --session SESSION
termtrace capture --session SESSION
termtrace verify SESSION --level content --json
```

Normal indexing and capture require `completion.json`. Use `index --live-snapshot` only when an explicitly incomplete, hash-addressed snapshot is intended.
