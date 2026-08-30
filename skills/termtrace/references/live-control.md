# Live control

Use agent responses throughout an automated workflow:

```sh
termtrace --response agent start app --until-screen Ready -- command
termtrace --response agent act app text:/settings enter --until-idle 100ms
termtrace --response agent screen app --since STATE
```

`--until-screen` observes the presented viewport. It is not a search of all
canonical child output. Use `wait --output` separately only when byte history is
the requirement.

`act` admits its complete atom list under one guard and one request journal
entry, then evaluates the requested postcondition without a caller-visible race.
Natural atoms include single printable characters, named keys such as `enter`,
modifiers such as `ctrl+f` or `ctrl-down`, and explicit `text:`, `paste:`,
`raw:`, or `raw-file:` values. Run `termtrace control keys` for the complete
grammar. Use `--response full` only when forensic terminal internals are
required.

`send` is deliberately lower-level: choose exactly one of `--text`, `--file`,
or `--stdin` to write bytes directly. It has no semantic atom grammar, state
guard, pacing, request ID, or postcondition. Use `act` whenever input depends on
terminal modes or presented state.

Pass the state returned by `start`, `act`, or `screen` through `--if-state` when
input is valid only for that presentation. Use the same state with
`screen --since STATE`; an unchanged response proves that no later presented
screen differs from it without retransmitting the viewport.

An action response reports input application separately from its postcondition.
Treat `applied`, `retry`, and `session_state` as authoritative even when the
overall operation failed. A failed postcondition does not undo successfully
applied input. Follow `retry` exactly: an uncertain transport outcome may require
the identical request ID and payload, while deterministic rejection requires a
corrected request. Never resend a whole sequence merely because its
postcondition was unmet.

Use marks for named phases selected later, not as generic mutation logging.
Input, resize, signal, and stop already have canonical intent/result events.
