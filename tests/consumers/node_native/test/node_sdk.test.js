const test = require('node:test');
const assert = require('node:assert');
const os = require('node:os');
const path = require('node:path');
const fs = require('node:fs');

const { Termtrace, version } = require('@termtrace/core');
const { withTermtrace } = require('@termtrace/core/testing');

const TERMTRACE_BIN = process.env.TERMTRACE_BIN
  ? path.resolve(process.env.TERMTRACE_BIN)
  : path.resolve(__dirname, '../../../../target/debug/termtrace');

test('version returns core semver', () => {
  assert.strictEqual(version(), '0.1.0');
});

test('sync and async recording lifecycle', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-consumer-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo NODE_CONSUMER_OK'], 24, 80);
    assert.strictEqual(rec.isActive(), true);
    assert.ok(rec.sessionId.length > 0);

    const screen = rec.screen();
    const state = await screen.waitForText('NODE_CONSUMER_OK', 5000);
    assert.ok(state.text.includes('NODE_CONSUMER_OK'));
    assert.ok(state.cursorRow >= 0);

    await rec.stop();
    assert.strictEqual(rec.isActive(), false);
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('screen frame geometry and text retrieval', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-frame-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo FRAME_GEOMETRY_OK'], 32, 96);
    const screen = rec.screen();
    await screen.waitForText('FRAME_GEOMETRY_OK', 5000);

    const frame = await screen.frame();
    assert.strictEqual(frame.rows, 32);
    assert.strictEqual(frame.cols, 96);
    assert.ok(frame.text.includes('FRAME_GEOMETRY_OK'));

    const plainText = await screen.text();
    assert.ok(plainText.includes('FRAME_GEOMETRY_OK'));

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('action builder state-guarded input', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-actions-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc'], 24, 80);
    const screen = rec.screen();
    const readyState = await screen.waitForText('$', 5000);

    const actions = rec.actions();
    actions.ifState(readyState.hash);
    actions.sendText('echo ACTION_NODE_SUCCESS');
    actions.sendKey('Enter');
    actions.untilScreen('ACTION_NODE_SUCCESS', 5000);
    const outcome = await actions.execute();

    assert.ok(outcome);
    assert.ok(outcome.text.includes('ACTION_NODE_SUCCESS'));

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('pointer actions (click, scroll, focus)', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-pointer-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc'], 24, 80);
    await rec.screen().waitForText('$', 5000);

    await rec.pointerClick(10, 5, 'left');
    await rec.pointerScroll(10, 5, 2);
    await rec.pointerFocus(true);

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('visual snapshot save (PNG, SVG, HTML, TXT, JSON)', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-save-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo VISUAL_SAVE_NODE'], 24, 80);
    await rec.screen().waitForText('VISUAL_SAVE_NODE', 5000);

    const saveDir = path.join(tmpdir, 'snapshot_out');
    const manifestJson = await rec.save(saveDir, ['png', 'svg', 'html', 'txt', 'json']);
    const manifest = JSON.parse(manifestJson);

    assert.ok(manifest.format);
    assert.ok(fs.existsSync(saveDir));
    assert.ok(manifest.files.length > 0);
    const firstFile = path.join(saveDir, manifest.files[0].path);
    assert.ok(fs.existsSync(firstFile));

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('raw output streaming and wait', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-output-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo RAW_OUTPUT_NODE'], 24, 80);
    await rec.screen().waitForText('RAW_OUTPUT_NODE', 5000);

    const page = await rec.readOutput(0, 4096);
    assert.ok(page.bytes.length > 0);
    const textOut = Buffer.from(page.bytes).toString('utf8');
    assert.ok(textOut.includes('RAW_OUTPUT_NODE'));

    const matched = await rec.waitForOutput('RAW_OUTPUT_NODE', 5000);
    assert.strictEqual(matched, true);

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('event log reading and pagination', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-events-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo EVENT_NODE_TEST'], 24, 80);
    await rec.screen().waitForText('EVENT_NODE_TEST', 5000);

    const eventsJson = await rec.readEvents(0, 50);
    const events = JSON.parse(eventsJson);
    assert.ok(Array.isArray(events));
    assert.ok(events.length > 0);

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('replay analysis services (explain, diagnose, last_action, timeline)', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-analysis-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc'], 24, 80);
    const state = await rec.screen().waitForText('$', 5000);

    const actions = rec.actions();
    actions.sendText('echo ACTION_FOR_ANALYSIS');
    actions.sendKey('Enter');
    actions.untilScreen('ACTION_FOR_ANALYSIS', 5000);
    await actions.execute();

    const session = rec.session();
    const stateHash = state.hash;
    await rec.stop();

    // Replay analysis on completed session
    const explanation = JSON.parse(await session.explain(stateHash, 1));
    assert.ok(explanation.target);

    const diagnosis = JSON.parse(await session.diagnose(20));
    assert.ok(diagnosis.findings);

    const lastAct = JSON.parse(await session.lastAction());
    assert.ok(typeof lastAct === 'object');

    const timeline = JSON.parse(await session.timelinePage(0, 10));
    assert.ok(timeline.entries);
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('system capability probe', async () => {
  const tt = new Termtrace(TERMTRACE_BIN);
  const capsJson = await tt.probeCapabilities();
  const caps = JSON.parse(capsJson);
  assert.ok(Array.isArray(caps.capabilities));
});

test('registry operations (register, resolve, list, remove, gc)', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-registry-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo REGISTRY_NODE'], 24, 80);
    const sessionPath = rec.path;
    const alias = `node-alias-${process.pid}`;

    const entry = JSON.parse(await tt.registryRegister(alias, sessionPath));
    assert.strictEqual(entry.alias, alias);

    const resolved = await tt.registryResolve(alias);
    assert.strictEqual(resolved, sessionPath);

    const entries = JSON.parse(await tt.registryList(true));
    assert.ok(entries.some(e => e.alias === alias));

    const removed = await tt.registryRemove(alias);
    assert.strictEqual(removed, true);

    const gcReport = JSON.parse(await tt.registryGc(1, 10, undefined, false));
    assert.ok(gcReport.state_root);

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('session integrity verification', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-integrity-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo INTEGRITY_NODE'], 24, 80);
    await rec.screen().waitForText('INTEGRITY_NODE', 5000);
    const sessionPath = rec.path;
    await rec.stop();
    const report = JSON.parse(await tt.verifySession(sessionPath, 'structure'));
    assert.strictEqual(report.valid, true);
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('withTermtrace testing helper lifecycle', async () => {
  await withTermtrace(
    async ({ record }) => {
      const rec = await record(['/bin/sh', '-norc', '-c', 'echo HELPER_TEST_OK']);
      const screen = rec.screen();
      const state = await screen.waitForText('HELPER_TEST_OK', 5000);
      assert.ok(state.text.includes('HELPER_TEST_OK'));
    },
    { binary: TERMTRACE_BIN }
  );
});

test('asciicast export writes valid playback file', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-export-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo EXPORT_NODE_OK'], 24, 80);
    const screen = rec.screen();
    await screen.waitForText('EXPORT_NODE_OK', 5000);

    const session = rec.session();
    const castPath = path.join(tmpdir, 'playback.cast');
    await session.exportAsciicast(castPath);

    assert.ok(fs.existsSync(castPath));
    assert.ok(fs.statSync(castPath).size > 0);

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('failures carry typed kind, code, effect, and retry', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-timeout-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'sleep 10'], 24, 80);
    const screen = rec.screen();

    await assert.rejects(
      async () => {
        await screen.waitForText('ABSENT_PATTERN_123', 200);
      },
      (error) => {
        assert.match(error.message, /timed out waiting for pattern/);
        assert.equal(error.kind, 'wait_unsatisfied');
        assert.deepEqual(error.effect, { kind: 'not_applied' });
        assert.equal(error.retry, 'new_request');
        return true;
      }
    );

    const guarded = rec.actions();
    guarded.ifState('0'.repeat(64));
    guarded.sendKey('ctrl+shift+F5');
    await assert.rejects(
      () => guarded.execute(),
      (error) => {
        assert.equal(error.kind, 'stale_state');
        assert.equal(error.code, 'stale_observation');
        assert.deepEqual(error.effect, { kind: 'not_applied' });
        assert.equal(error.retry, 'new_request');
        return true;
      }
    );

    await rec.stop();
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});
