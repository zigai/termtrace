# Documentation

Termtrace records terminal execution as verifiable events and derives semantic or visual views from that recording.

## Get started

1. [Install Termtrace](installation.md).
2. [Record and complete a session](recording.md).
3. [Inspect or control the terminal](agent-workflows.md).

For a working example, run the [end-to-end demo](../examples/README.md).

## Guides

| Topic | Use it when you want to |
| --- | --- |
| [Recording](recording.md) | choose geometry, provenance, and durability settings |
| [Full tmux recording](tmux.md) | record a composite client track and linked pane evidence |
| [Test isolation](test-isolation.md) | control child environment, limits, and atomic idle capture |
| [Live control](agent-workflows.md) | synchronize, act, inspect screens, or automate a session |
| [Language clients](language-clients.md) | use the typed Python and TypeScript APIs or regenerate their contracts |
| [Semantic replay](semantic-replay.md) | understand states, synchronized output, and ranged replay |
| [Troubleshooting](troubleshooting.md) | diagnose build, control, replay, or capture failures |

## Reference

| Topic | Contents |
| --- | --- |
| [Artifacts and verification](formats.md) | canonical files, derived files, schemas, and trust boundaries |
| [Architecture](architecture.md) | truth layers, runtime components, and failure policy |

For flags and subcommands, use the installed CLI:

```bash
termtrace --help
termtrace COMMAND --help
```
