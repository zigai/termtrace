# Semantic replay

The canonical event log is replayed byte by byte through the exact pinned libghostty-vt engine. A snapshot boundary is distinct when any complete semantic content changes: grid cells, grapheme clusters, styles, resolved colors, palette, cursor, active SGR style, known ANSI/DEC modes, terminal title, working directory, or geometry.

Ghostty dirty flags are evidence and optimization hints, not identity. State hashes exclude transient dirty bookkeeping and use a domain-separated BLAKE3 Merkle layout: stable metadata plus one hash per complete row. This permits efficient updates while the final hash still commits to the entire viewport.

Each state records its canonical event sequence and byte offset inside that event. Resize state is applied only after the matching successful resize result. Input, signal, marker, and lifecycle events remain in the execution timeline but do not invent visual changes. State records are stored as compressed JSONL in `states.jsonl.zst`; the lightweight `state-index.jsonl` remains directly streamable.

## Synchronized output

DEC mode 2026 suppresses presentation, not execution. Intermediate changes are indexed with `presented: false`. The state that disables synchronized output is indexed as presented. This keeps transient debugging evidence without claiming native Ghostty displayed it. `index --presented-only` is a derived storage mode that omits suppressed records; it never removes bytes from canonical execution truth or changes the default full-fidelity index.

## Delta reconstruction

The first state includes full geometry, metadata, and every cell. Later records include only changed metadata and changed cells grouped by row. Consumers resize their grid when `delta.geometry` appears, then apply cell changes by row and column. `state-index.jsonl` is the lightweight event-to-state lookup table.

## Ranged replay

`termtrace index --from OFFSET --to OFFSET` selects the half-open interval `[from, to)`. Unique marker labels can select the same boundaries with `--from-marker` and `--to-marker`. Earlier output and applied resizes are replayed without emitting states. At the inclusive start, Termtrace snapshots the fully reconstructed terminal as dense state `0` with no responsible event and a complete delta. Events exactly at the start are then included; events exactly at the end are excluded. Resize intents that begin before the boundary remain paired with results inside it. Markers and lifecycle events retain timing but do not invent semantic changes, and synchronized-output presentation flags continue to follow DEC mode 2026.

The range manifest records absolute monotonic nanosecond bounds. State timestamps remain absolute source timestamps, while state identifiers are dense within the derived range. `final.txt`, `final.vt`, and `final.html` describe the terminal at the exclusive end boundary. Invalid, zero-duration, and out-of-session bounds fail rather than being clamped.

## Determinism

Replay outputs contain no generation timestamps. Given the same validated event log, pinned Ghostty revision, and artifact schema version, state records, hashes, final text, VT, and HTML are byte-for-byte deterministic. Tests replay the same recording twice and compare every output.
