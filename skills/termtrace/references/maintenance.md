# Skill maintenance

After the CLI graph changes:

- Update every skill example that invokes a changed path.
- Keep discovery shallow: root help and the direct leaf help for the workflow.
  Do not copy generated command listings into the skill.
- Verify affected root and direct leaf help against the built binary. Do not
  recursively traverse help or pipe it through shell text filters.
- Run the repository's focused skill/CLI check when it is lightweight. Run
  broader tests once at the repository level rather than as part of a skill
  refresh.

The refresh is complete only when examples invoke the tested CLI graph and the
skill explains clean text and JSON output, presented-screen truth, Python SDK
selection, and mutation/retry authority.
