# Security

Termtrace recordings and artifacts may contain terminal output, typed input, source code, paths, host details, clipboard contents, and secrets.

- Session directories and control sockets must be accessible only to their owner.
- Raw input capture must be explicit and visibly reported.
- Artifacts must never be uploaded automatically.
- Environment metadata is allowlisted rather than captured wholesale.
- Diagnostic reports must redact configured secret values while preserving structural context.

Report vulnerabilities privately through GitHub's security advisory interface for this repository.

