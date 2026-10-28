// Ordinary node:test workflow: guarded input, assertion, reviewed snapshot, retained evidence.
// TERMTRACE_WORKFLOW_MISMATCH=1 makes the line test expect a value the fixture never prints; it
// then fails and reports where its evidence was kept.

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {
  ExpectationMismatchError, assertScreenText, assertSnapshotFile, withTermtrace,
} from '@termtrace/core/testing';

// Compiled into tests/consumers/node_native/.examples/ by tsconfig.examples.json.
const repository = path.resolve(__dirname, '../../../..');
const lineFixture = path.join(repository, 'fixtures/programs/terminal/line-command.sh');
const snapshot = path.join(repository, 'examples/sdk/snapshots/line-value.json');
const failurePrefix = 'TERMTRACE_WORKFLOW_FAILURE ';

test('line workflow', async () => {
  const expected = process.env.TERMTRACE_WORKFLOW_MISMATCH ? 'LINE_VALUE=goodbye' : 'LINE_VALUE=hello';

  const report = await withTermtrace(async (ctx) => {
    const rec = await ctx.record([lineFixture], 24, 80);
    const ready = await rec.screen().waitForText('LINE_READY', 5000);
    const result = await rec.actions()
      .ifState(ready.hash)
      .sendText('hello')
      .sendKey('enter')
      .untilScreen('LINE_VALUE=', 5000)
      .run();
    const frame = result.frame;
    assert.ok(frame);
    try {
      assertScreenText(frame, expected);
    } catch (error) {
      if (error instanceof ExpectationMismatchError) {
        const failure = {
          assertion: error.message,
          session: rec.path,
          observation: frame.text,
          evidence: ctx.evidenceRoot,
        };
        console.error(failurePrefix + JSON.stringify(failure));
      }
      throw error;
    }

    assertSnapshotFile(frame, snapshot, {
      profile: 'text', update: Boolean(process.env.TERMTRACE_UPDATE_SNAPSHOTS),
    });
    const finalized = await ctx.finalize('passed');
    assert.deepEqual(await ctx.finalize('error'), finalized);
    return finalized;
  }, { binary: process.env.TERMTRACE_BIN, evidencePolicy: 'always' });

  const session = report.verified_artifacts.find((artifact) => {
    return fs.existsSync(path.join(artifact, 'completion.json'));
  });
  assert.ok(session, 'no verified session evidence was retained');
  const completion = JSON.parse(fs.readFileSync(path.join(session, 'completion.json'), 'utf8'));
  assert.equal(completion.exit_code, 0);
});

test('mismatch fails with the assertion, session, observation and evidence', () => {
  const evidenceRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'workflow-mismatch-'));
  const env: NodeJS.ProcessEnv = {
    ...process.env, TERMTRACE_WORKFLOW_MISMATCH: '1', TMPDIR: evidenceRoot,
  };
  delete env.TERMTRACE_UPDATE_SNAPSHOTS;
  // A child that inherits the runner's context reports to it instead of running the file.
  delete env.NODE_TEST_CONTEXT;
  const run = spawnSync(process.execPath, ['--test-name-pattern=^line workflow$', __filename], {
    env, encoding: 'utf8',
  });
  const output = run.stdout + run.stderr;
  assert.notEqual(run.status, 0, output);

  const line = output.split('\n').find((candidate) => candidate.startsWith(failurePrefix));
  assert.ok(line, output);
  const failure = JSON.parse(line.slice(failurePrefix.length));
  assert.match(failure.assertion, /LINE_VALUE=goodbye/);
  assert.match(failure.observation, /LINE_VALUE=hello/);

  assert.equal(fs.statSync(failure.evidence).mode & 0o777, 0o700);
  assert.ok(failure.session.startsWith(failure.evidence + path.sep), output);
  const completion = JSON.parse(fs.readFileSync(path.join(failure.session, 'completion.json'), 'utf8'));
  const manifest = JSON.parse(fs.readFileSync(path.join(failure.session, 'session.json'), 'utf8'));
  assert.equal(completion.session_id, manifest.session_id);
});
