'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { Termtrace } = require('@termtrace/core');

test('foundation public Node SDK adapter', async () => {
  const repo = path.resolve(__dirname, '../../../..');
  const root = process.env.SDK_CONFORMANCE_ROOT || fs.mkdtempSync(path.join(os.tmpdir(), 'sdk-conformance-node-'));
  fs.mkdirSync(root, { recursive: true });
  const f = JSON.parse(fs.readFileSync(process.env.SDK_CONFORMANCE_FIXTURE || path.join(repo, 'fixtures/conformance/sdk/cases/foundation.json')));
  const tt = new Termtrace(process.env.TERMTRACE_BIN || path.join(repo, 'target/debug/termtrace'), path.join(root, 'evidence'));
  const rec = await tt.record([...f.command, path.join(root, 'child.pid')], f.rows, f.cols);
  let observation;
  let primary;
  let failed = false;
  try {
    await rec.screen().waitForText(f.text, f.timeout_ms);
    const frame = await rec.screen().frame();
    const action = rec.actions(); action.ifState(f.guard); action.sendText('FORBIDDEN\n');
    let stale;
    await assert.rejects(() => action.execute(), error => { stale = { kind: error.kind, code: error.code, effect: error.effect, retry: error.retry }; return true; });
    const page = await rec.readOutput(0n, 4096);
    observation = { version: 1, language: 'node', case_id: 'foundation', session_path: rec.path,
      rows: frame.rows, cols: frame.cols, text: frame.text, output_hex: Buffer.from(page.bytes).toString('hex'), stale };
  } catch (error) { failed = true; primary = error; }
  try { await rec.stop(); } catch (error) {
    if (failed) console.error('Secondary SDK adapter cleanup failure:', error);
    else { failed = true; primary = error; }
    try { rec.forceClose(); } catch (secondary) { console.error('SDK adapter force-close failure:', secondary); }
  }
  if (failed) throw primary;
  observation.active = rec.isActive();
  fs.writeFileSync(path.join(root, 'observation.json'), JSON.stringify(observation, null, 2));
});
