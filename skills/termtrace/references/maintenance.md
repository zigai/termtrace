# Skill maintenance

After the CLI graph changes:

- Update every skill example that invokes a changed path.
- Keep only the live `--help` discovery ladder in the skill instructions. Do not
  copy generated command listings or option text into the skill.
- Verify the affected root, group, and leaf `--help` paths against the built
  binary.
- Run the repository's focused skill/CLI test and then the normal test suite.

The refresh is complete only when examples invoke the tested CLI graph and the
skill tells agents to consult the installed binary for version-specific details.
