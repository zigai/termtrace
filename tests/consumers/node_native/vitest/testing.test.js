import { test, expect } from 'vitest';
import { withTermtrace, assertScreenText, normalizedSnapshot } from '@termtrace/core/testing';
test('ordinary Vitest test: action -> presented observation -> shared assertion', async () => {
  await withTermtrace(async ({ record }) => {
    const rec = await record(['/bin/sh', '-c', 'printf READY; while IFS= read -r line; do printf "INPUT:<%s>" "$line"; done'], 30, 96);
    await rec.screen().waitForText('READY', 5000);
    const action = await rec.actions().sendText('vitest\n').untilScreen('INPUT:<vitest>', 5000).run();
    assertScreenText(action.frame, 'INPUT:<vitest>');
    // Runner-native diffs work with ordinary strings; no global matcher registration required.
    expect(JSON.parse(normalizedSnapshot(action.frame)).geometry).toEqual({ cols: 96, rows: 30 });
  }, { binary: process.env.TERMTRACE_BIN });
});
