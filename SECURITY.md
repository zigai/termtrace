# Security

Termtrace recordings and artifacts may contain terminal output, typed input, source code, paths, host details, clipboard contents, and secrets.

- Session directories are created with mode `0700`; logs and control sockets use `0600`. Existing directories are never reused.
- Linux control connections are additionally authenticated with `SO_PEERCRED`; the peer UID must equal the recording owner's effective UID.
- Control messages are capped at 1 MiB, use bounded I/O timeouts, and expose only an allowlist of non-`SIGKILL` signals.
- State-changing requests have caller-owned UUIDs and actor-side deduplication. A retry with the same UUID cannot inject input twice.
- Raw input capture must be explicit and visibly reported.
- Artifacts must never be uploaded automatically.
- Environment metadata is allowlisted rather than captured wholesale.
- Diagnostic reports must redact configured secret values while preserving structural context.

Report vulnerabilities privately through GitHub's security advisory interface for this repository.
