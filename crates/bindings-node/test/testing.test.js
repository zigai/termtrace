const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../testing.js'), 'utf8');

function harness(t, { recordings = [], constructorError, removeError } = {}) {
  const roots = [];
  const messages = [];
  class Termtrace {
    constructor(binary, root) {
      roots.push(root);
      fs.writeFileSync(path.join(root, 'evidence.txt'), 'canonical test evidence');
      if (constructorError) throw constructorError;
    }
    async record() {
      assert.ok(recordings.length > 0, 'unexpected recording');
      return recordings.shift();
    }
  }
  // Fake only the native finalizer boundary. The runtime helper has no JS cleanup engine.
  async function finalizeRecordings(recordings, root, outcome, policy) {
    const cleanupErrors = [];
    for (const rec of recordings) {
      try { if (rec.isActive()) await rec.stop(); }
      catch (error) {
        cleanupErrors.push(error);
        try { await rec.forceClose(); } catch (error) { cleanupErrors.push(error); }
      }
    }
    if (outcome === 'passed' && cleanupErrors.length === 0 && policy === 'on_failure') {
      try {
        if (removeError) throw removeError;
        fs.rmSync(root, { recursive: true, force: true });
      } catch (error) { cleanupErrors.push(error); }
    }
    return { outcome, evidence_directory: fs.existsSync(root) ? root : null,
      retained: fs.existsSync(root), verified_artifacts: [], secondary_failures: cleanupErrors.map(error => ({ phase: 'finalization', message: error.message, truncated: false })) };
  }
  const module = { exports: {} };
  vm.runInNewContext(source, {
    module,
    AggregateError,
    console: { error: (...args) => messages.push(args) },
    require(name) {
      if (name === './index') return { Termtrace, finalizeRecordings };
      if (name === 'fs' && removeError) {
        return { ...fs, rmSync() { throw removeError; } };
      }
      return require(name);
    },
  }, { filename: 'testing.js' });
  t.after(() => {
    for (const root of roots) fs.rmSync(root, { recursive: true, force: true });
  });
  return { ...module.exports, roots, messages };
}

function recording({ stopError, closeError, active = true } = {}) {
  return {
    stops: 0,
    forceCloses: 0,
    active,
    isActive() { return this.active; },
    async stop() {
      this.stops++;
      if (stopError) throw stopError;
      this.active = false;
    },
    forceClose() {
      this.forceCloses++;
      if (closeError) throw closeError;
      this.active = false;
    },
  };
}

function assertEvidence(root) {
  assert.equal(fs.readFileSync(path.join(root, 'evidence.txt'), 'utf8'), 'canonical test evidence');
}

test('successful callback returns its value, stops recordings and discards evidence', async (t) => {
  const rec = recording();
  const h = harness(t, { recordings: [rec] });
  const result = await h.withTermtrace(async ({ record }) => {
    assert.equal(await record(['app']), rec);
    return 42;
  });
  assert.equal(result, 42);
  assert.equal(rec.stops, 1);
  assert.equal(rec.forceCloses, 0);
  assert.equal(fs.existsSync(h.roots[0]), false);
  assert.equal(h.messages.length, 0);
});

test('callback failure retains evidence and rethrows the original error', async (t) => {
  const failure = new Error('assertion failed');
  const rec = recording();
  const h = harness(t, { recordings: [rec] });
  await assert.rejects(h.withTermtrace(async ({ record }) => {
    await record(['app']);
    throw failure;
  }), (error) => error === failure);
  assert.equal(rec.stops, 1);
  assertEvidence(h.roots[0]);
  assert.equal(failure.termtrace.evidenceRoot, h.roots[0]);
  assert.equal(failure.termtrace.cleanupErrors.length, 0);
  assert.match(h.messages[0][0], new RegExp(h.roots[0]));
});

test('cleanup failure forces teardown, retains evidence and fails an otherwise passing test', async (t) => {
  const stopError = new Error('stop failed');
  const rec = recording({ stopError });
  const other = recording();
  const h = harness(t, { recordings: [rec, other] });
  await assert.rejects(h.withTermtrace(async ({ record }) => {
    await record(['first']);
    await record(['second']);
  }), (error) => {
    assert.ok(error instanceof AggregateError);
    assert.deepEqual(Array.from(error.errors, error => error.message), ['finalization: stop failed']);
    assert.equal(error.termtrace.evidenceRoot, h.roots[0]);
    return true;
  });
  assert.equal(rec.forceCloses, 1);
  assert.equal(other.stops, 1);
  assertEvidence(h.roots[0]);
  assert.equal(h.messages[1][1].message, 'finalization: stop failed');
});

test('cleanup failures stay secondary to the original test failure', async (t) => {
  const failure = new Error('primary failure');
  const stopError = new Error('stop failed');
  const closeError = new Error('force close failed');
  const rec = recording({ stopError, closeError });
  const h = harness(t, { recordings: [rec] });
  await assert.rejects(h.withTermtrace(async ({ record }) => {
    await record(['app']);
    throw failure;
  }), (error) => error === failure);
  assert.deepEqual(Array.from(failure.termtrace.cleanupErrors, error => error.message), ['finalization: stop failed', 'finalization: force close failed']);
  assertEvidence(h.roots[0]);
  assert.equal(h.messages.length, 3);
});

for (const [name, failure] of [
  ['frozen error', Object.freeze(new Error('frozen'))],
  ['primitive', 'failure'],
  ['undefined', undefined],
]) {
  test(`${name} is rethrown unchanged and its evidence location is reported`, async (t) => {
    const h = harness(t);
    let caught = false;
    try {
      await h.withTermtrace(async () => { throw failure; });
    } catch (error) {
      caught = true;
      assert.equal(error, failure);
    }
    assert.equal(caught, true);
    assertEvidence(h.roots[0]);
    assert.ok(h.messages[0][0].includes(h.roots[0]));
  });
}

test('client construction failures also report their retained evidence', async (t) => {
  const failure = new Error('client initialization failed');
  const h = harness(t, { constructorError: failure });
  await assert.rejects(h.withTermtrace(async () => {
    assert.fail('callback must not run');
  }), (error) => error === failure);
  assertEvidence(h.roots[0]);
  assert.equal(failure.termtrace.evidenceRoot, h.roots[0]);
});

test('filesystem cleanup failures are surfaced instead of silently passing', async (t) => {
  const removeError = new Error('cannot remove evidence');
  const h = harness(t, { removeError });
  await assert.rejects(h.withTermtrace(async () => {}), (error) => {
    assert.ok(error instanceof AggregateError);
    assert.deepEqual(Array.from(error.errors, error => error.message), ['finalization: cannot remove evidence']);
    return true;
  });
  assertEvidence(h.roots[0]);
});

test('already stopped recordings are not stopped again', async (t) => {
  const rec = recording({ active: false });
  const h = harness(t, { recordings: [rec] });
  await h.withTermtrace(async ({ record }) => { await record(['app']); });
  assert.equal(rec.stops, 0);
  assert.equal(rec.forceCloses, 0);
  assert.equal(fs.existsSync(h.roots[0]), false);
});
