# Live control

Use `show SESSION --format json-compact` for an immediate presented-state hash,
or `wait-idle SESSION` when the exact stable snapshot is the action precondition.
Pass that hash through `--if-state` on state-dependent
mutations.

`send` admits its complete atom list under one guard and one request journal
entry. Natural atoms include single printable characters, named keys such as
`enter`, modifiers such as `ctrl+f` or `ctrl-down`, and explicit `text:`,
`paste:`, `raw:`, or `raw-file:` values. Run `termtrace control keys` for the complete
grammar. Use `--verbose` only when per-atom canonical event coordinates are
needed.

Default acknowledgements contain operation counts and one compact canonical
boundary. Exact emitted bytes and full actor boundaries remain available through
the SDK protocol.

If the transport reports `safe_same_request_id`, retry the identical request ID
and payload. SDK sequence atom IDs are deterministic for that request ID, so the
same typed atom list rebuilds the same payload. The driver deliberately does not
cache that transport failure. An actor rejection is deterministic and requires
a corrected request with a new ID. `input_sequence_incomplete` includes the
applied count, failed atom, partial byte count, and first unattempted suffix
index.

Use marks for named phases that will be selected later, not as generic mutation
logging. Input, resize, signal, and stop already have canonical intent/result
events.
