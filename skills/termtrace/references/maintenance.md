# Command reference maintenance

After the CLI graph changes, run this from the skill directory:

```sh
python3 scripts/generate-command-reference.py --binary PATH --out references/commands.md
```

The refresh is complete only when the checked reference matches generated
`--help`, its example commands pass, and the repository tests succeed. Commit the
regenerated reference only after this criterion is met.
