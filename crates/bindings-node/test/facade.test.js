const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { ActionResult, Termtrace } = require('..');

const TERMTRACE_BIN = process.env.TERMTRACE_BIN
  ? path.resolve(process.env.TERMTRACE_BIN)
  : path.resolve(__dirname, '../../../target/debug/termtrace');

test('the package entry point keeps the typed, fluent, and sealing facade', async () => {
  assert.throws(() => ActionResult.fromJson('{}'), TypeError);

  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'node-facade-'));
  const tt = new Termtrace(TERMTRACE_BIN, root);
  const rec = await tt.launch([
    '/bin/sh',
    '-c',
    "printf READY; while :; do if IFS= read -r line; then printf '\\r\\nGOT:%s' \"$line\"; fi; done",
  ]);
  try {
    await rec.screen().waitForText('READY', 5000);

    const builder = rec.actions();
    assert.strictEqual(builder.sendText('slow\n'), builder);
    builder.untilScreen('NEVER', 500).allowIncomplete();
    const pending = builder.run();
    assert.throws(() => builder.sendText('late\n'), (error) => error.kind === 'invalid_operation');

    const winner = await Promise.race([pending.then(() => 'action'), new Promise((resolve) => setTimeout(resolve, 20, 'timer'))]);
    assert.strictEqual(winner, 'timer');
    const first = await pending;
    const retry = await builder.run();
    assert.ok(retry instanceof ActionResult);
    assert.strictEqual(retry.requestId, builder.requestId);
    assert.strictEqual(typeof retry.input.bytes_written, 'bigint');
    assert.deepStrictEqual(retry.input, first.input);

    await assert.rejects(rec.pointer().click(0, 0, { strict: true }), (error) => {
      return error.kind === 'control_rejected' && typeof error.request_id === 'string';
    });
  } finally {
    await rec.stop();
    fs.rmSync(root, { recursive: true, force: true });
  }
});
