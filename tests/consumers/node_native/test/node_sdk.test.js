const test = require('node:test');
const assert = require('node:assert');
const os = require('node:os');
const path = require('node:path');
const fs = require('node:fs');

const { Termtrace } = require('@termtrace/core');
const { withTermtrace } = require('@termtrace/core/testing');

const TERMTRACE_BIN = process.env.TERMTRACE_BIN
  ? path.resolve(process.env.TERMTRACE_BIN)
  : path.resolve(__dirname, '../../../../target/debug/termtrace');

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
    const manifest = (manifestJson);

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

    const page = await rec.readOutput(0n, 4096);
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

    const eventsJson = await rec.readEvents(0n, 50);
    const events = (eventsJson);
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
    const explanation = (await session.explain(stateHash, 1));
    assert.ok(explanation.target);

    const diagnosis = (await session.diagnose(20));
    assert.ok(diagnosis.findings);

    const lastAct = (await session.lastAction());
    assert.ok(typeof lastAct === 'object');

    const timeline = (await session.timelinePage(0, 10));
    assert.ok(timeline.entries);
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('system capability probe', async () => {
  const tt = new Termtrace(TERMTRACE_BIN);
  const capsJson = await tt.probeCapabilities();
  const caps = (capsJson);
  assert.ok(Array.isArray(caps.capabilities));
});

test('registry operations (register, resolve, list, remove, gc)', async () => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-registry-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(['/bin/sh', '-norc', '-c', 'echo REGISTRY_NODE'], 24, 80);
    const sessionPath = rec.path;
    const alias = `node-alias-${process.pid}`;

    const entry = (await tt.registryRegister(alias, sessionPath));
    assert.strictEqual(entry.alias, alias);

    const resolved = await tt.registryResolve(alias);
    assert.strictEqual(resolved, sessionPath);

    const entries = (await tt.registryList(true));
    assert.ok(entries.some(e => e.alias === alias));

    const removed = await tt.registryRemove(alias);
    assert.strictEqual(removed, true);

    const gcReport = (await tt.registryGc(1, 10, undefined, false));
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
    const report = (await tt.verifySession(sessionPath, 'structure'));
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

test('withTermtrace retains real session evidence after a test failure', async () => {
  const failure = new Error('deliberate test failure');
  let evidenceRoot;
  let recording;
  try {
    await assert.rejects(
      withTermtrace(async ({ record, evidenceRoot: root }) => {
        evidenceRoot = root;
        recording = await record([
          '/bin/sh', '-c',
          'trap "exit 0" TERM; printf HELPER_FAILURE_READY; while :; do sleep 0.1; done',
        ]);
        await recording.screen().waitForText('HELPER_FAILURE_READY', 5000);
        throw failure;
      }, { binary: TERMTRACE_BIN }),
      (error) => error === failure,
    );
    assert.ok(fs.existsSync(evidenceRoot));
    assert.ok(fs.existsSync(path.join(recording.path, 'events.jsonl')));
    assert.ok(fs.existsSync(path.join(recording.path, 'completion.json')));
    assert.equal(recording.isActive(), false);
    assert.equal(failure.termtrace.evidenceRoot, evidenceRoot);
  } finally {
    if (recording && recording.isActive()) recording.forceClose();
    if (evidenceRoot) fs.rmSync(evidenceRoot, { recursive: true, force: true });
  }
});

const { CancelToken, LocatorSpec, ScreenFrame } = require('@termtrace/core');
const CAPTURE_QUANTUM_MS = 250;
const locatorFixtures = path.resolve(__dirname, '../../../../fixtures/conformance/locators');
const parity = JSON.parse(fs.readFileSync(path.join(locatorFixtures, 'parity.json'), 'utf8'));

function paritySpec(description) {
  let locator;
  if ('text' in description) locator = LocatorSpec.text(description.text);
  else if ('exact' in description) locator = LocatorSpec.exact(description.exact);
  else if ('regex' in description) locator = LocatorSpec.regex(description.regex);
  else locator = new LocatorSpec();
  if ('row' in description) locator = locator.inRow(description.row);
  if ('region' in description) locator = locator.inRegion(description.region);
  if ('style' in description) locator = locator.withStyle(description.style);
  if (description.cross_soft_wraps) locator = locator.acrossSoftWraps();
  return locator;
}

const triples = (ranges) => ranges.map((r) => [r.row, r.start_column, r.end_column]);

function checkParityCase(frame, testCase) {
  const { name, expect } = testCase;
  let found;
  try {
    const result = frame.query(paritySpec(testCase.spec));
    if (testCase.operation === undefined) found = result.all();
    else if (testCase.operation === 'nth') found = [result.nth(testCase.index)];
    else found = [result[testCase.operation]()];
  } catch (error) {
    assert.ok(expect.error, `${name}: ${error.message}`);
    assert.strictEqual(error.kind, expect.error.kind, name);
    for (const field of ['count', 'index', 'state_hash']) {
      if (field in expect.error && !(field === 'count' && expect.error.kind === 'no_match')) assert.strictEqual(error[field], field === 'state_hash' ? expect.error[field] : BigInt(expect.error[field]), `${name}: ${field}`);
    }
    if (expect.error.ranges) assert.deepStrictEqual(triples(error.ranges), expect.error.ranges, name);
    return;
  }
  assert.ok(expect.matches, `${name}: unexpected success`);
  assert.deepStrictEqual(
    found.map((m) => ({ text: m.text, ranges: triples(m.ranges), widths: m.cells.map((c) => c.width) })),
    expect.matches,
    name,
  );
}

test('live locator parity, rich frames, and waits', async () => {
  const live = parity.live;
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-locators-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.record(live.command, live.rows, live.cols);
    try {
      const screen = rec.screen();
      const ready = await screen.getByText(live.ready).wait(10000, 20);
      for (const testCase of live.cases) checkParityCase(ready.frame, testCase);

      const frame = await screen.frame();
      assert.strictEqual(frame.rows, live.rows);
      assert.strictEqual(frame.cols, live.cols);
      assert.ok(frame.text.includes('END'));
      assert.strictEqual(frame.content.detail, 'spans');
      assert.strictEqual(frame.content.rows.length, live.rows);
      assert.strictEqual(typeof frame.source.observed_monotonic_ns, 'bigint');
      assert.strictEqual(typeof frame.source.boundaries.observed.next_event_seq, 'bigint');
      assert.strictEqual(frame.modes.wraparound, true);
      assert.strictEqual(frame.colors.palette.length, 256);
      assert.strictEqual(frame.content.rows[5].soft_wrapped, true);
      assert.strictEqual(frame.content.rows[6].wrap_continuation, true);

      const region = { top: 1, left: 0, bottom: 2, right: 6 };
      const cells = await screen.frame({ detail: 'cells', region });
      assert.deepStrictEqual(cells.returnedRegion, region);
      assert.deepStrictEqual(cells.content.rows[0].cells.slice(0, 2).map((c) => c.wide_kind), ['wide', 'spacer_tail']);
      await assert.rejects(screen.frame({ detail: 'cells' }));

      const retained = frame.query(LocatorSpec.text('Save'));
      assert.strictEqual(retained.frame.frameHash, frame.frameHash);
      assert.strictEqual(retained.count(), 2);

      const state = await screen.waitForText('END', 5000);
      assert.strictEqual(typeof state.cursorRow, 'number');
      assert.strictEqual(state.cursor.row, state.cursorRow);
      assert.strictEqual(state.frame.stateHash, state.hash);

      assert.strictEqual(await screen.getByText('Save').count(), 2);
      assert.strictEqual((await screen.getByText('Save').waitFor({ condition: { count: 2 }, timeoutMs: 5000 })).matches.length, 2);
      assert.strictEqual((await screen.getByText('Cancel').waitStable(5000, 20, 50)).matches.length, 1);

      const unmatched = await screen.waitFor({ predicate: 'contains', text: 'NEVER-SHOWN' }, { timeoutMs: 300 });
      assert.strictEqual(unmatched.matched, false);
      assert.strictEqual(unmatched.satisfied, false);
      assert.strictEqual(unmatched.quality, 'deadline');
      assert.ok(unmatched.frame instanceof ScreenFrame);
      const matched = await screen.waitFor({ predicate: 'row_contains', row: 7, text: 'END' });
      assert.strictEqual(matched.satisfied, true);
      assert.strictEqual(typeof matched.first_match_monotonic_ns, 'bigint');

      const capture = await screen.captureStable({ stableForMs: 50, timeoutMs: 5000 });
      assert.strictEqual(capture.quality, 'stable');
      assert.ok(capture.semantic_state_zstd instanceof Uint8Array);
      assert.strictEqual(capture.stable_for_ns, 50000000n);
      const observation = await screen.observe();
      assert.strictEqual(typeof observation.boundary.next_event_seq, 'bigint');

      const locator = screen.getByText('NEVER-SHOWN');
      for (const timeoutMs of [-1, 1.5, Number.NaN, 3600001]) {
        await assert.rejects(locator.waitFor({ timeoutMs }), RangeError);
      }
      await assert.rejects(locator.waitFor({ timeoutMs: 0 }), (error) => error.kind === 'invalid_query');
      await assert.rejects(locator.waitFor({ timeoutMs: 1000, stableForMs: 2000 }), (error) => error.kind === 'invalid_query');
      await assert.rejects(locator.waitFor({ condition: 'present' }), TypeError);
      assert.throws(() => screen.getByRegex(/x/), TypeError);

      const started = Date.now();
      await assert.rejects(locator.waitFor({ timeoutMs: 1000, intervalMs: 20 }), (error) => {
        assert.strictEqual(error.kind, 'locator_timeout');
        assert.deepStrictEqual(error.effect, { kind: 'not_applied' });
        assert.ok(error.frame instanceof ScreenFrame);
        assert.strictEqual(error.state_hash, error.frame.stateHash);
        return true;
      });
      assert.ok(Date.now() - started <= 1000 + CAPTURE_QUANTUM_MS + 500);

      const controller = new AbortController();
      const waiting = locator.waitFor({ timeoutMs: 30000, intervalMs: 20, signal: controller.signal });
      await new Promise((resolve) => setTimeout(resolve, 300));
      const abortedAt = Date.now();
      controller.abort();
      await assert.rejects(waiting, (error) => {
        assert.strictEqual(error.kind, 'cancelled');
        assert.ok(error.frame instanceof ScreenFrame);
        return true;
      });
      const latency = Date.now() - abortedAt;
      assert.ok(latency < 1500, `cancellation took ${latency} ms`);
      console.log(`node cancellation latency: ${latency} ms`);
      // The native worker has returned, so a follow-up capture is served immediately.
      assert.ok((await screen.frame()).text.includes('END'));

      const preAborted = AbortSignal.abort();
      await assert.rejects(locator.waitFor({ timeoutMs: 30000, signal: preAborted }), (error) => error.kind === 'cancelled' && error.frame === null);
      const token = new CancelToken();
      const tokenWait = locator.waitFor({ timeoutMs: 30000, intervalMs: 20, cancel: token });
      token.cancel();
      await assert.rejects(tokenWait, (error) => error.kind === 'cancelled');
      assert.strictEqual(token.cancelled, true);
      await assert.rejects(screen.frameAt({ selector: 'state_id', state_id: 0n }));
    } finally {
      await rec.stop();
    }

    const completed = (await tt.attach(rec.path)).screen();
    const first = await completed.frameAt({ selector: 'state_id', state_id: 0n }, { includeHidden: true });
    assert.strictEqual(first.captureQuality, 'completed');
    assert.strictEqual(first.source.source, 'resolved');
    assert.strictEqual(first.source.state_id, 0n);
    assert.strictEqual(first.source.source_completed, true);
    const observed = await completed.observe({ point: { selector: 'state_id', state_id: 0n }, includeHidden: true });
    assert.strictEqual(observed.state_id, 0n);
    assert.ok(observed.canonical_session instanceof Uint8Array);
    // Argument conversion fails before the promise exists.
    assert.throws(() => completed.frameAt({ selector: 'state_id', state_id: -1n }), TypeError);
    const final = await completed.frame();
    assert.ok(final.presented && final.text.includes('END'));
    assert.strictEqual(await completed.getByText('Save').count(), 2);

    const erasing = await tt.record(['/bin/sh', '-c', "printf LOADING; sleep 1; printf '\\r\\033[K'; sleep 30"], 5, 40);
    try {
      const loading = erasing.screen().getByText('LOADING');
      await loading.wait(5000);
      const gone = await loading.waitFor({ condition: 'absent', timeoutMs: 5000, intervalMs: 20 });
      assert.deepStrictEqual(gone.matches, []);
      assert.ok(!gone.frame.text.includes('LOADING'));
    } finally {
      await erasing.stop();
    }
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

const crypto = require('node:crypto');
const { ActionResult } = require('@termtrace/core');

const INTERACTION = JSON.parse(
  fs.readFileSync(path.resolve(__dirname, '../../../../fixtures/conformance/actions/interaction-control.json'), 'utf8'),
);

function errorFacts(error) {
  return { kind: error.kind, code: error.code, effect: error.effect, retry: error.retry };
}

function uuidV5(namespace, name) {
  const digest = crypto
    .createHash('sha1')
    .update(Buffer.from(namespace.replace(/-/g, ''), 'hex'))
    .update(name, 'utf8')
    .digest()
    .subarray(0, 16);
  digest[6] = (digest[6] & 0x0f) | 0x50;
  digest[8] = (digest[8] & 0x3f) | 0x80;
  const hex = digest.toString('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function sendStepId(sendRequestId, index) {
  const name = INTERACTION.send_step_name.replace('{send}', sendRequestId).replace('{index}', String(index));
  return uuidV5(INTERACTION.send_step_namespace, name);
}

async function withFixture(commandName, options, body) {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-interaction-'));
  const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
  const rec = await tt.launch(INTERACTION[commandName], options);
  try {
    await rec.screen().waitForText(INTERACTION.ready_text, 5000);
    await body(rec, tmpdir);
  } finally {
    if (rec.isActive()) await rec.stop();
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
}

function count(text, needle) {
  return text.split(needle).length - 1;
}

test('run seals the request and replays it with the same identity', async () => {
  await withFixture('echo_command', undefined, async (rec) => {
    const chosen = crypto.randomUUID();
    const builder = rec.actions().withRequestId(chosen);
    assert.strictEqual(builder.requestId, chosen);
    assert.throws(() => rec.actions().withRequestId('not-a-uuid'), (error) => error.kind === 'invalid_operation');

    const pending = builder
      .sendText('once')
      .sendKey('enter')
      .untilScreenWhere({ predicate: 'contains', text: 'GOT:once' }, 5000)
      .run();
    assert.strictEqual(builder.sealed, true);
    for (const change of [
      () => builder.sendText('more'),
      () => builder.withRequestId(crypto.randomUUID()),
      () => builder.allowIncomplete(),
      () => builder.execute(),
    ]) {
      assert.throws(change, (error) => {
        assert.deepStrictEqual(errorFacts(error), INTERACTION.errors.sealed);
        return true;
      });
    }
    const result = await pending;
    assert.ok(result instanceof ActionResult);
    assert.strictEqual(result.requestId, chosen);
    assert.strictEqual(result.input.sequence_id, chosen);
    assert.strictEqual(typeof result.input.bytes_written, 'bigint');
    assert.strictEqual(result.matched, true);
    assert.strictEqual(result.stateHash, result.postcondition.state_hash);
    assert.ok(result.frame.text.includes('GOT:once'));

    const retry = await builder.run();
    assert.deepStrictEqual(retry.input, result.input);
    assert.strictEqual(count(await rec.screen().text(), 'GOT:once'), 1);
    const restored = ActionResult.fromJson(result.toJson());
    assert.deepStrictEqual(restored.input, result.input);
    assert.deepStrictEqual(restored.postcondition, result.postcondition);
  });
});

test('legacy execute sends a fresh request each call', async () => {
  await withFixture('echo_command', undefined, async (rec) => {
    const builder = rec.actions();
    const firstId = builder.requestId;
    builder.sendText('one\n').untilScreen('GOT:one', 5000);
    const state = await builder.execute();
    assert.strictEqual(typeof state.cursorRow, 'number');
    const secondId = builder.requestId;
    assert.notStrictEqual(secondId, firstId);
    assert.strictEqual(builder.sealed, false);
    await builder.sendText('two\n').untilScreen('GOT:two', 5000).execute();
    assert.ok(![firstId, secondId].includes(builder.requestId));
    const text = await rec.screen().text();
    assert.strictEqual(count(text, 'GOT:one'), 1);
    assert.strictEqual(count(text, 'GOT:two'), 1);
  });
});

test('mutation failures match the shared fixture', async () => {
  await withFixture('echo_command', undefined, async (rec) => {
    const staleBuilder = rec.actions().ifState('0'.repeat(64)).sendText('stale\n');
    await assert.rejects(staleBuilder.run(), (error) => {
      assert.deepStrictEqual(errorFacts(error), INTERACTION.errors.stale);
      assert.strictEqual(error.request_id, staleBuilder.requestId);
      assert.strictEqual(typeof error.details.boundaries.durable.next_event_seq, 'bigint');
      return true;
    });

    const first = rec.actions().sendText('first\n');
    await first.run();
    await assert.rejects(rec.actions().withRequestId(first.requestId).sendText('other\n').run(), (error) => {
      assert.deepStrictEqual(errorFacts(error), INTERACTION.errors.collision);
      return true;
    });

    await assert.rejects(rec.pointer().click(0, 0, { strict: true }), (error) => {
      assert.deepStrictEqual(errorFacts(error), INTERACTION.errors.strict_pointer);
      assert.ok(error.request_id);
      return true;
    });

    await assert.rejects(rec.actions().sendText('late\n').untilScreen('NEVER', 200).run(), (error) => {
      assert.deepStrictEqual(errorFacts(error), INTERACTION.errors.incomplete);
      assert.ok(error.result instanceof ActionResult);
      assert.strictEqual(error.result.input.atom_count, 1);
      assert.strictEqual(error.result.matched, false);
      return true;
    });

    const text = (await rec.screen().waitForText('GOT:late', 5000)).text;
    assert.ok(text.includes('GOT:first'));
    assert.ok(!text.includes('GOT:stale'));
    assert.ok(!text.includes('GOT:other'));
  });
});

test('send reports derived step identities and partial failures', async () => {
  await withFixture('echo_command', undefined, async (rec) => {
    const sendId = crypto.randomUUID();
    const result = await rec.send({
      steps: [
        { step: 'mark', label: 'before' },
        { step: 'input', atoms: [{ text: 'sent' }, { key: 'enter' }] },
      ],
      request_id: sendId, until: { condition: 'screen', predicate: { predicate: 'contains', text: 'GOT:sent' } },
    });
    assert.strictEqual(result.request_id, sendId);
    assert.strictEqual(result.last.kind, 'action');
    assert.strictEqual(result.last.step, 1n);
    assert.strictEqual(result.last.request_id, sendStepId(sendId, 1));
    assert.ok(result.last.result instanceof ActionResult);

    const failingId = crypto.randomUUID();
    await assert.rejects(
      rec.send({
        steps: [
          { step: 'mark', label: 'applied' },
          { step: 'pointer', column: 0, row: 0, operation: { kind: 'click', button: 'left' }, strict: true },
        ],
        request_id: failingId,
      }),
      (error) => {
        assert.strictEqual(error.step, 1n);
        assert.strictEqual(error.send_request_id, failingId);
        assert.strictEqual(error.request_id, sendStepId(failingId, 1));
        assert.deepStrictEqual(errorFacts(error), INTERACTION.errors.strict_pointer);
        return true;
      },
    );
    const labels = (await rec.readEvents(0n, 1000))
      .filter((event) => event.type === 'marker')
      .map((event) => event.label);
    assert.deepStrictEqual(labels, ['before', 'applied']);
  });
});

test('raw bytes and wide integers stay exact', async () => {
  let report;
  await withFixture('raw_command', undefined, async (rec) => {
    const raw = Buffer.from(INTERACTION.raw_bytes_hex, 'hex');
    assert.throws(() => rec.send({ steps: [{ step: 'input', atoms: [{ bytes: 'not bytes!' }] }] }), TypeError);
    const result = await rec.actions().sendBytes(new Uint8Array(raw)).run();
    assert.strictEqual(result.input.bytes_written, BigInt(raw.length));
    await rec.screen().waitForText(INTERACTION.raw_bytes_echo, 5000);
    assert.ok((await rec.readEvents(0n, 1000)).some(event => event.type === 'input_intent' && Buffer.from(event.data ?? []).equals(raw)));
    report = JSON.parse(result.toJson());
  });
  report.input.bytes_written = '__WIDE__';
  const wide = JSON.stringify(report).replace('"__WIDE__"', INTERACTION.wide_decimal);
  assert.strictEqual(ActionResult.fromJson(wide).input.bytes_written, BigInt(INTERACTION.wide_decimal));
  for (const malformed of INTERACTION.malformed_decimals) {
    report.input.bytes_written = malformed;
    assert.throws(() => ActionResult.fromJson(JSON.stringify(report)), TypeError);
  }

  await withFixture('raw_command', undefined, async (rec) => {
    await rec.actions().sendBytes([...Buffer.from(INTERACTION.raw_bytes_hex, 'hex')]).run();
    await rec.screen().waitForText(INTERACTION.raw_bytes_echo, 5000);
  });
});

test('typed pointer operations report their outcomes', async () => {
  await withFixture('mouse_command', undefined, async (rec) => {
    const pointer = rec.pointer();
    const multi = await pointer.multiClick(3, 0, 2, { strict: true });
    assert.strictEqual(multi.kind, 'applied');
    assert.ok(multi.bytes instanceof Uint8Array);
    assert.strictEqual(count(Buffer.from(multi.bytes).toString('latin1'), '\x1b[<'), 4);
    assert.strictEqual(multi.bytes_written, BigInt(multi.bytes.length));
    const drag = await pointer.drag(1, 0, 5, 0, { steps: 3, strict: true });
    assert.strictEqual(drag.kind, 'applied');
    assert.ok(Buffer.from(drag.bytes).toString('latin1').includes('<32;'));
    const activated = await pointer.activate(INTERACTION.activation_text, { unique: true, strict: true });
    assert.strictEqual(activated.kind, 'activated');
    assert.deepStrictEqual([activated.text, activated.start_column, activated.row], [INTERACTION.activation_text, 0, 0]);
    await assert.rejects(pointer.click(-1, 0), RangeError);
  });
  await withFixture('echo_command', undefined, async (rec) => {
    const noop = await rec.pointer().click(0, 0);
    assert.strictEqual(noop.kind, 'noop');
    assert.strictEqual(noop.reason, 'pointer_reporting_disabled');
    assert.strictEqual(await rec.pointerClick(0, 0), undefined);
  });
});

test('session controls and status', async () => {
  await withFixture('echo_command', undefined, async (rec) => {
    const status = await rec.status();
    assert.strictEqual(status.kind, 'live');
    assert.strictEqual(typeof status.boundaries.durable.next_event_seq, 'bigint');
    const resized = await rec.resize(100, 30);
    assert.ok(resized.request_id);
    const frame = await rec.screen().frame();
    assert.deepStrictEqual([frame.cols, frame.rows], [100, 30]);
    await rec.signal('interrupt');
    await rec.screen().waitForText('SIGNAL:INT', 5000);
    const marked = await rec.mark('checkpoint');
    assert.strictEqual(marked.label, 'checkpoint');
    assert.strictEqual(typeof marked.event_seq, 'bigint');
    const stopId = crypto.randomUUID();
    const stopped = await rec.requestStop({ requestId: stopId, waitMs: 5000 });
    assert.strictEqual(stopped.kind, 'completed');
    assert.strictEqual(stopped.request_id, stopId);
    assert.strictEqual(rec.isActive(), false);
    assert.strictEqual((await rec.status()).kind, 'completed');
  });
});

test('launch environment is isolated and exact', async () => {
  process.env.TT_COPY = INTERACTION.copy_value;
  process.env.TT_SECRET = INTERACTION.secret_value;
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-environment-'));
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.launch(INTERACTION.environment_command, {
      environment: { base: 'clear', set: { TT_SET: new Uint8Array(Buffer.from(INTERACTION.set_bytes_hex, 'hex')) }, copy: ['TT_COPY'] },
    });
    try {
      const text = (await rec.screen().waitForText('DONE', 5000)).text;
      assert.ok(text.includes(`SET= ${INTERACTION.set_bytes_echo}`));
      assert.ok(text.includes(`COPY=${INTERACTION.copy_value}`));
      assert.ok(text.includes('SECRET=absent'));
      assert.match(text, /TERM=\S+/);
    } finally {
      await rec.stop();
    }
    assert.throws(() => tt.launch(['/bin/true'], { environment: { set: { TT_SET: 'not bytes!' } } }), TypeError);
    const secret = Buffer.from(INTERACTION.secret_value);
    const files = fs.readdirSync(tmpdir, { recursive: true }).map((name) => path.join(tmpdir, name));
    for (const file of files.filter((name) => fs.statSync(name).isFile())) {
      assert.ok(!fs.readFileSync(file).includes(secret), file);
    }
  } finally {
    delete process.env.TT_COPY;
    delete process.env.TT_SECRET;
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
});

test('launch limits end the session', async () => {
  for (const limit of ['max_duration', 'idle_timeout']) {
    const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-limit-'));
    try {
      const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
      const rec = await tt.launch(['/bin/sh', '-c', 'printf READY; sleep 30'], { [`${limit}_ms`]: 400 });
      const completion = await rec.waitForCompletion(10000);
      assert.strictEqual(completion.resource_limit.kind, limit);
      assert.strictEqual(completion.resource_limit.limit, 400n);
      assert.strictEqual((await rec.status()).kind, 'completed');
    } finally {
      fs.rmSync(tmpdir, { recursive: true, force: true });
    }
  }
});

test('abandoning a run promise keeps the admitted action', async () => {
  await withFixture('echo_command', undefined, async (rec) => {
    const builder = rec
      .actions()
      .sendText('slow\n')
      .untilScreenWhere({ predicate: 'contains', text: 'NEVER' }, 800)
      .allowIncomplete();
    const requestId = builder.requestId;
    const original = builder.run();
    const winner = await Promise.race([original.then(() => 'action'), new Promise((resolve) => setTimeout(resolve, 50, 'timer'))]);
    assert.strictEqual(winner, 'timer');
    const settled = await original;
    const retry = await builder.run();
    assert.strictEqual(retry.requestId, requestId);
    assert.deepStrictEqual(retry.input, settled.input);
    assert.strictEqual(retry.matched, false);
    assert.strictEqual(count(await rec.screen().text(), 'GOT:slow'), 1);
  });
});

test('shared atomic-action fixture matches the Rust summary', async () => {
  const fixture = JSON.parse(
    fs.readFileSync(path.resolve(__dirname, '../../../../fixtures/conformance/actions/atomic-action.json'), 'utf8'),
  );
  const { region } = fixture;
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-atomic-'));
  let summary;
  try {
    const tt = new Termtrace(TERMTRACE_BIN, tmpdir);
    const rec = await tt.launch(fixture.command, { rows: region.height, cols: region.width });
    try {
      assert.ok(await rec.waitForOutput(fixture.ready_text, 5000));
      const initial = await rec.screen().frame();
      const builder = rec.actions().withRequestId(fixture.request_id).ifState(initial.stateHash);
      for (const atom of fixture.input) {
        if (atom.kind === 'text') builder.sendText(atom.text);
        else builder.sendKey(atom.key);
      }
      builder.untilScreenWhere(fixture.predicate, 5000).projection(fixture.frame_detail, {
        top: region.top, left: region.left, right: region.left + region.width, bottom: region.top + region.height,
      });
      const result = await builder.run();
      const retry = await builder.run();
      const stable = await rec.screen().captureStable({ stableForMs: 20, timeoutMs: 2000 });
      const { frame } = result;
      summary = {
        state_hash_valid: frame.stateHash.length === 64,
        atom_count: result.input.atom_count,
        matched: result.matched && ['matched', 'matched_unsettled'].includes(result.postcondition.quality) && frame.text.includes(fixture.expected_text),
        presented: frame.presented,
        retry_exact: retry.toJson() === result.toJson(),
        stable_quality: stable.quality === 'stable',
        actor_boundaries: frame.source.source === 'actor' && JSON.stringify(frame.source.boundaries, (_, v) => (typeof v === 'bigint' ? v.toString() : v)) === JSON.stringify(result.boundaries, (_, v) => (typeof v === 'bigint' ? v.toString() : v)),
      };
    } finally {
      await rec.stop();
    }
  } finally {
    fs.rmSync(tmpdir, { recursive: true, force: true });
  }
  assert.deepStrictEqual(summary, {
    state_hash_valid: true, atom_count: fixture.expected_atom_count, matched: true, presented: true,
    retry_exact: true, stable_quality: true, actor_boundaries: true,
  });
});

test('nested frame JSON keeps exact integer tokens through Rust', async () => {
  let encoded;
  await withFixture('echo_command', undefined, async (rec) => {
    const result = await rec.actions().sendText('framed\n').untilScreenWhere({ predicate: 'contains', text: 'GOT:framed' }, 5000).run();
    encoded = result.toJson();
  });
  for (const value of [INTERACTION.wide_decimal, INTERACTION.u64_max]) {
    const changed = encoded.replace(/"observed_monotonic_ns":\d+/, '"observed_monotonic_ns":' + value);
    const decoded = ActionResult.fromJson(changed);
    assert.strictEqual(decoded.frame.source.observed_monotonic_ns, BigInt(value));
    assert.strictEqual(decoded.toJson(), changed);
  }
  for (const value of ['-1', '1.5', '18446744073709551616', '"1"']) {
    assert.throws(() => ActionResult.fromJson(encoded.replace(/"observed_monotonic_ns":\d+/, '"observed_monotonic_ns":' + value)), TypeError);
  }
});

test('a rejected stop keeps the recording attached', async () => {
  await withFixture('echo_command', undefined, async (rec) => {
    const used = crypto.randomUUID();
    await rec.mark('before-stop', { requestId: used });
    await assert.rejects(rec.requestStop({ requestId: used, waitMs: 5000 }), (error) => {
      assert.deepStrictEqual(errorFacts(error), INTERACTION.errors.collision);
      assert.strictEqual(error.request_id, used);
      return true;
    });
    assert.strictEqual(rec.isActive(), true);
    assert.strictEqual((await rec.status()).kind, 'live');
    const after = await rec.actions().sendText('after\n').untilScreen('GOT:after', 5000).run();
    assert.strictEqual(after.matched, true);
    await rec.stop();
    assert.strictEqual(rec.isActive(), false);
    assert.strictEqual((await rec.status()).kind, 'completed');
  });
});

const VISUAL_PROFILES = JSON.parse(
  fs.readFileSync(
    path.resolve(__dirname, '../../../../fixtures/conformance/sdk/visual-profiles.json'),
    'utf8',
  ),
);
const SEMANTIC_PROFILE = { version: 1, semantic: { preset: 'bundled-v1' } };

test('visual profile vectors agree with the shared contract', async (t) => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-visual-profile-'));
  t.after(() => fs.rmSync(tmpdir, { recursive: true, force: true }));
  const tt = new Termtrace(TERMTRACE_BIN, tmpdir);

  for (const vector of VISUAL_PROFILES.positive) {
    const report = await tt.preflightVisualProfile(vector.profile, 40, 10);
    assert.strictEqual(report.version, 1);
    assert.deepStrictEqual(report.profile, vector.normalized, vector.name);
    assert.strictEqual(report.profile_hash, vector.profile_hash, vector.name);
    if (vector.native) {
      assert.strictEqual(report.native.dpi, 96);
      assert.deepStrictEqual(report.native.discovered_font_families, ['JetBrains Mono']);
      assert.deepStrictEqual(
        [report.native.geometry.required_cols, report.native.geometry.required_rows],
        [40, 10],
      );
    } else {
      assert.strictEqual(report.native, null);
    }
    if ('semantic' in vector.normalized) {
      assert.deepStrictEqual([report.semantic.cell_width, report.semantic.cell_height], [12, 24]);
    } else {
      assert.strictEqual(report.semantic, null);
    }
  }

  for (const vector of VISUAL_PROFILES.invalid) {
    await assert.rejects(tt.preflightVisualProfile(vector.profile), (error) => {
      assert.strictEqual(error.kind, 'invalid_operation', vector.name);
      assert.strictEqual(error.code, 'invalid_visual_profile', vector.name);
      return true;
    });
  }

  for (const vector of VISUAL_PROFILES.malformed) {
    assert.throws(
      () => tt.preflightVisualProfile(vector.profile),
      (error) => error.message.includes(vector.message),
      vector.name,
    );
  }
});

test('a semantic visual profile records without native tools', async (t) => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-semantic-profile-'));
  const searchPath = process.env.PATH;
  t.after(() => {
    process.env.PATH = searchPath;
    fs.rmSync(tmpdir, { recursive: true, force: true });
  });
  process.env.PATH = '';
  const tt = new Termtrace(TERMTRACE_BIN, tmpdir);

  const report = await tt.preflightVisualProfile(SEMANTIC_PROFILE);
  assert.strictEqual(report.native, null);
  const rec = await tt.launch(['/bin/sh', '-c', 'echo SEMANTIC_PROFILE'], {
    visual_profile: SEMANTIC_PROFILE,
  });
  assert.ok(await rec.waitForOutput('SEMANTIC_PROFILE', 5000));
  await rec.stop();
  const manifest = JSON.parse(fs.readFileSync(path.join(rec.path, 'session.json'), 'utf8'));
  assert.strictEqual(manifest.version, 13);
  assert.strictEqual(manifest.visual_profile.profile_hash, report.profile_hash);
  assert.strictEqual(manifest.ghostty_visual, null);

  await assert.rejects(tt.preflightVisualProfile(VISUAL_PROFILES.positive[1].profile), (error) => {
    assert.strictEqual(error.code, 'native_profile_unavailable');
    return true;
  });
});

test('native visual profile failures stop before the application starts', async (t) => {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'node-native-profile-'));
  t.after(() => fs.rmSync(tmpdir, { recursive: true, force: true }));
  const root = path.join(tmpdir, 'evidence');
  const marker = path.join(tmpdir, 'started');
  const tt = new Termtrace(TERMTRACE_BIN, root);
  const command = ['/bin/sh', '-c', 'printf started > "$1"', 'probe', marker];

  await assert.rejects(
    tt.launch(command, {
      visual_profile: { version: 1, native: { font_family: 'Termtrace Missing Family 7c1d' } },
    }),
    (error) => {
      assert.deepStrictEqual([error.kind, error.code], ['engine', 'font_unavailable']);
      return true;
    },
  );
  await assert.rejects(
    tt.launch(command, { visual_profile: VISUAL_PROFILES.invalid[2].profile }),
    (error) => {
      assert.strictEqual(error.code, 'invalid_visual_profile');
      return true;
    },
  );
  assert.strictEqual(fs.existsSync(marker), false);
  assert.ok(!fs.existsSync(root) || fs.readdirSync(root).length === 0);
});
