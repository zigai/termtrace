const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const { Termtrace, Instrumentation, Registry } = require('@termtrace/core');
const repo = path.resolve(__dirname, '../../../..');
const binary = process.env.TERMTRACE_BIN || path.join(repo, 'target/debug/termtrace');
const fixture = JSON.parse(fs.readFileSync(path.join(repo, 'fixtures/conformance/sdk/diagnostics-evidence.json'), 'utf8'));
const bytes = value => Buffer.from(value);
function hashes(root) {
  return Object.fromEntries(fs.readdirSync(root, { withFileTypes: true }).filter(entry => entry.isFile()).map(entry => [entry.name, crypto.createHash('sha256').update(fs.readFileSync(Buffer.isBuffer(root) ? Buffer.concat([root, Buffer.from('/' + entry.name)]) : path.join(root, entry.name))).digest('hex')]));
}
function normalize(value) { return { ...value, resolution: 'canonical_replay' }; }
async function completed() {
  const root = fs.mkdtempSync(path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), 'node-diag-'));
  const client = new Termtrace(binary, path.join(root, 'sessions'));
  const rec = await client.launch(['/bin/sh', '-c', 'printf READY; read line; printf DONE; read line'], { cols: 40, rows: 8 });
  let incomplete;
  try {
    await rec.output().waitFor(bytes('READY'), 5000);
    await rec.mark('cut');
    const emitter = rec.instrumentation();
    const ack = await emitter.event('fixture', fixture.application_attributes);
    const span = await emitter.startSpan('complete');
    const ended = await emitter.endSpan('complete', span.span_id);
    incomplete = await emitter.startSpan('incomplete', span.span_id);
    assert.equal(ended.span_id, span.span_id);
    assert.equal(ack.process_instance_id, emitter.processInstanceId);
    assert.ok(ack.boundaries.durable.next_event_seq > 0n);
    await assert.rejects(async () => emitter.event('unsafe', { nested: { inode: 9007199254740992 } }), /safe-integer/);
    assert.throws(() => emitter.event('unsafe-symbol', { inode: Symbol('invalid') }), TypeError);
    const cyclic = {}; cyclic.self = cyclic;
    const cyclicArray = []; cyclicArray.push(cyclicArray);
    let deep = null; for (let i = 0; i < 130; ++i) deep = [deep];
    for (const invalid of [cyclic, { array: cyclicArray }, { deep }, { large: 'x'.repeat(65536) }, { many: Array(65537).fill(null) }]) {
      await assert.rejects(async () => emitter.event('unsafe', invalid));
    }
    await rec.actions().sendText('go\n').untilScreen('DONE', 5000).run();
    await rec.stop();
  } finally {
    if (rec.isActive()) await rec.stop();
  }
  return { root, client, rec, incomplete };
}

test('native typed analysis, all save formats/state selectors, source/profile/tamper policy and cache invariance', async () => {
  const { root, client, rec, incomplete } = await completed();
  const diag = rec.analysis(), evidence = rec.evidence();
  const attached = await client.attach(rec.path), before = hashes(rec.path);
  const observation = normalize(await attached.screen().observe());
  assert.deepEqual(normalize(await attached.screen().observe()), observation);
  const page = await diag.timeline(0n, 1000, false, true);
  const states = page.entries.filter(entry => entry.item.kind === 'semantic_state');
  const last = states.at(-1);
  assert.equal(typeof last.item.state_id, 'bigint');
  assert.equal((await diag.diff({ selector: 'state_id', state_id: 0n }, { selector: 'latest_presented' })).changed, true);
  assert.equal(typeof (await diag.explain()).source_event_count, 'bigint');
  assert.ok(Array.isArray((await diag.diagnose()).findings));
  assert.ok((await diag.lastAction()).diagnostics);
  assert.ok((await diag.markers()).some(marker => marker.label === 'cut'));
  assert.deepEqual(Buffer.from((await diag.reviewSummary()).canonical_session), bytes(rec.path));
  assert.ok(await diag.reviewSearch('READY'));
  assert.deepEqual(Buffer.from((await diag.reviewExport(last.item.state_id, bytes(path.join(root, 'review')))).canonical_session), bytes(rec.path));
  const saved = await evidence.save(bytes(path.join(root, 'all-formats')), fixture.save_formats);
  assert.equal(saved.manifest.files.length, fixture.save_formats.length);
  assert.ok(saved.manifest.semantic_image_resources.length);
  assert.equal(saved.manifest.files.find(file => file.path === 'screen.png').truth_layer, 'semantic_rendering');
  assert.equal(saved.manifest.sensitivity, 'terminal_screen_may_contain_secrets');
  const published = hashes(path.join(root, 'all-formats'));
  await assert.rejects(evidence.save(bytes(path.join(root, 'all-formats')), ['txt']));
  assert.deepEqual(hashes(path.join(root, 'all-formats')), published);
  const points = [{ selector: 'latest_presented' }, { selector: 'state_id', state_id: last.item.state_id },
    { selector: 'state_hash', state_hash: last.item.state_hash }, { selector: 'event_seq', event_seq: last.event_seq },
    { selector: 'monotonic_ns', monotonic_ns: last.monotonic_ns }, { selector: 'marker', label: 'cut' }];
  assert.deepEqual(points.map(point => point.selector), fixture.selectors);
  for (const point of points) {
    const report = await evidence.save(bytes(path.join(root, point.selector)), ['txt'], point);
    assert.deepEqual(report.manifest.state_point, point);
  }
  await evidence.asciicast(bytes(path.join(root, 'session.cast')));
  const other = await client.launch(['/bin/true'], { environment: { base: 'clear' } });
  await other.stop();
  for (const profile of fixture.profiles) {
    const destination = bytes(path.join(root, profile));
    const report = await evidence.bundle(destination, profile);
    assert.equal(report.profile, profile);
    assert.equal(report.includes_canonical_evidence, profile === 'forensic');
    assert.ok(report.members.every(member => member.sensitivity));
    assert.equal((await diag.cachePrune()).remaining.entries, 0n);
    assert.deepEqual(normalize(await attached.screen().observe()), observation);
    assert.deepEqual(await evidence.verifyBundle(destination, profile), report);
    await assert.rejects(client.evidence(bytes(other.path)).verifyBundle(destination, profile));
    await assert.rejects(evidence.verifyBundle(destination, profile === 'screen' ? 'debug' : 'screen'));
    await assert.rejects(evidence.bundle(destination, profile));
    fs.appendFileSync(path.join(root, profile, 'screen.txt'), 'TAMPER');
    await assert.rejects(evidence.verifyBundle(destination, profile));
  }
  assert.throws(() => evidence.bundle(bytes(path.join(root, 'bad-profile')), 'forensik'), TypeError);
  assert.throws(() => evidence.verify('signatur'), TypeError);
  await assert.rejects(diag.timeline(0n, 0));
  await assert.rejects(diag.timeline(0n, 100, false, false, 1n));
  await assert.rejects(diag.timeline(0n, 100, false, false, 2n, 1n));
  assert.deepEqual(hashes(rec.path), before);
  const app = (await rec.events().page(null, 1000, false, ['application'])).events.map(entry => entry.event.data);
  assert.deepEqual(app.find(item => item.name === 'fixture').attributes, fixture.application_attributes);
  assert.ok(!app.some(item => item.name.startsWith('unsafe')));
  assert.ok(app.some(item => item.span_id === incomplete.span_id && item.kind === 'span_start'));
  assert.ok(!app.some(item => item.span_id === incomplete.span_id && item.kind === 'span_end'));
  assert.ok((await rec.events().page(null, 1000, true, ['application'])).events.every(entry => !('attributes' in entry.metadata)));
  await assert.rejects(new Instrumentation(bytes(rec.path)).event('after-completion'));
  fs.writeFileSync(path.join(root, 'diagnostics-observed.json'), JSON.stringify({ source_hashes_before: before, source_hashes_after: hashes(rec.path), profiles: fixture.profiles, selectors: fixture.selectors, incomplete_span_id: incomplete.span_id, formats: fixture.save_formats }, null, 2));
});

test('temporary OpenSSH unsigned, valid-untrusted/trusted/wrong signer and damaged receipt remain distinct', async () => {
  const { root, rec } = await completed(), evidence = rec.integrity();
  assert.equal((await evidence.verify('content')).valid, true);
  const unsigned = await evidence.verify('signature');
  assert.equal(unsigned.valid, false);
  assert.equal(unsigned.signature_receipt_present, false);
  const before = hashes(rec.path), keys = fs.mkdtempSync(path.join(os.tmpdir(), 'tt05-ssh-'));
  const outcomes = { unsigned };
  try {
    const key = path.join(keys, 'key');
    const generated = spawnSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', key]);
    assert.equal(generated.status, 0, 'temporary OpenSSH key generation required');
    const policy = path.join(keys, 'allowed'), wrong = path.join(keys, 'wrong');
    fs.writeFileSync(policy, 'task05 ' + fs.readFileSync(key + '.pub', 'utf8'));
    fs.writeFileSync(wrong, 'wrong-identity ' + fs.readFileSync(key + '.pub', 'utf8'));
    const receipt = await evidence.sign(bytes(key), 'task05'), receiptPath = Buffer.from(receipt);
    const untrusted = await evidence.verify('signature');
    assert.equal(untrusted.signature_cryptographic_valid, true);
    assert.equal(untrusted.signature_signer_trusted, false);
    assert.equal(untrusted.valid, false);
    const trusted = await evidence.verify('signature', bytes(policy));
    assert.equal(trusted.valid, true);
    assert.equal(trusted.session_authenticity_established, true);
    const rejected = await evidence.verify('signature', bytes(wrong));
    assert.equal(rejected.signature_cryptographic_valid, true);
    assert.equal(rejected.signature_signer_trusted, false);
    const original = fs.readFileSync(receiptPath);
    fs.writeFileSync(receiptPath, '{');
    assert.equal((await evidence.verify('signature', bytes(policy))).valid, false);
    // Receipt contains canonical u128 timestamps: edit only the Base64 signature token as text.
    const text = original.toString('utf8').replace(/("value"\s*:\s*")([A-Za-z0-9+/=]+)(")/, (_, left, value, right) => {
      const raw = Buffer.from(value, 'base64'); raw[Math.floor(raw.length / 2)] ^= 1;
      return left + raw.toString('base64') + right;
    });
    fs.writeFileSync(receiptPath, text);
    const tampered = await evidence.verify('signature', bytes(policy));
    assert.equal(tampered.valid, false);
    assert.equal(tampered.session_authenticity_established, false);
    fs.writeFileSync(receiptPath, original);
    assert.equal((await evidence.verify('signature', bytes(policy))).valid, true);
    Object.assign(outcomes, { untrusted, trusted, wrong_signer: rejected, tampered });
  } finally {
    fs.rmSync(keys, { recursive: true, force: true });
  }
  const after = hashes(rec.path);
  assert.deepEqual(Object.fromEntries(Object.keys(before).map(key => [key, after[key]])), before);
  fs.writeFileSync(path.join(root, 'trust-observed.json'), JSON.stringify(outcomes, (_, value) => typeof value === 'bigint' ? value.toString() : value, 2));
});

test('registry exact numeric u128, optional absence/corruption and real GC source protection', async () => {
  const root = fs.mkdtempSync(path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), 'node-registry-'));
  const state = path.join(root, 'state'), registry = new Termtrace(binary).registry(bytes(state));
  assert.equal(await registry.resolve('absent'), null);
  fs.mkdirSync(path.join(state, 'sessions'), { mode: 0o700 });
  const rec = await new Termtrace(binary, path.join(state, 'sessions')).launch(['/bin/sh', '-c', 'printf LIVE; read line']);
  try {
    await rec.output().waitFor(bytes('LIVE'), 5000);
    await registry.register('named', bytes(rec.path));
    assert.equal((await registry.lifecycle('named')).lifecycle, 'live');
    assert.equal((await registry.gc(0n, 0n, 0n, true)).removed_sessions, 0n);
    assert.ok(fs.existsSync(rec.path));
    await rec.stop();
  } finally { if (rec.isActive()) await rec.stop(); }
  const before = hashes(rec.path), file = path.join(state, 'registry/named.json');
  const text = fs.readFileSync(file, 'utf8').replace(/("registered_unix_ns"\s*:\s*)\d+/, '$1' + fixture.u128_max);
  fs.writeFileSync(file, text);
  assert.equal((await registry.resolve('named')).registered_unix_ns, BigInt(fixture.u128_max));
  assert.equal((await registry.list(true))[0].registered_unix_ns, BigInt(fixture.u128_max));
  fs.writeFileSync(file, '{');
  await assert.rejects(registry.resolve('named'));
  const blockedGc = await registry.gc(0n, 0n, 0n, true);
  assert.equal(blockedGc.skipped_corrupt_registry_entries, 1n);
  assert.equal(blockedGc.removed_sessions, 0n);
  assert.ok(fs.existsSync(rec.path));
  fs.writeFileSync(file, text);
  assert.equal((await registry.gc(0n, 0n, 0n)).changes_applied, false);
  await registry.remove('named');
  assert.deepEqual(hashes(rec.path), before);
  assert.ok((await registry.gc(0n, 0n, 0n)).candidates.length);
  assert.equal((await registry.gc(0n, 0n, 0n, true)).removed_sessions, 1n);
  assert.equal(fs.existsSync(rec.path), false);
});


test('native restart report and failed readiness preserve immutable predecessor evidence', async () => {
  const { root, client, rec } = await completed();
  const state = path.join(root, 'restart-state'), registry = client.registry(bytes(state));
  const predecessor = await registry.register('demo', bytes(rec.path)), before = hashes(rec.path);
  const environment = { base: 'clear', set: { FIXTURE_NEW: bytes('yes') } };
  await assert.rejects(registry.restart('demo', { command: [bytes('/bin/true')] }, environment, 5000), /recoverable orphan/);
  assert.equal((await registry.resolve('demo')).session_id, predecessor.session_id);
  const successor = await registry.restart('demo', { command: [bytes('/bin/true')], cols: 66, allow_environment_change: true }, environment, 5000);
  assert.equal(successor.ready, true);
  assert.equal(successor.predecessor.session_id, predecessor.session_id);
  assert.equal((await registry.resolve('demo')).session_id, successor.successor.session_id);
  assert.equal(typeof successor.successor.registered_unix_ns, 'bigint');
  await registry.register('failure', bytes(rec.path));
  const failed = new Registry(bytes(state), bytes('/bin/false'));
  await assert.rejects(failed.restart('failure', null, null, 500), /recoverable orphan/);
  assert.equal((await registry.resolve('failure')).session_id, predecessor.session_id);
  assert.deepEqual(hashes(rec.path), before);
});


test('explicit hidden-state selection and typed system/catalog surfaces', async () => {
  const root = fs.mkdtempSync(path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), 'node-hidden-'));
  const client = new Termtrace(binary, path.join(root, 'sessions'));
  const rec = await client.launch(['/bin/sh', '-c', "printf '\\033[?2026hHIDDEN\\033[?2026lVISIBLE'"]);
  try {
    await rec.output().waitFor(bytes('VISIBLE'), 5000);
    await rec.stop();
  } finally { if (rec.isActive()) await rec.stop(); }
  const hidden = (await rec.analysis().timeline(0n, 1000, true)).entries.find(entry => entry.item.kind === 'semantic_state' && !entry.item.presented);
  assert.ok(hidden);
  const point = { selector: 'state_id', state_id: hidden.item.state_id };
  await assert.rejects(rec.evidence().save(bytes(path.join(root, 'not-published')), ['txt'], point));
  assert.equal(fs.existsSync(path.join(root, 'not-published')), false);
  assert.deepEqual((await rec.evidence().save(bytes(path.join(root, 'hidden')), ['txt'], point, true)).manifest.state_point, point);
  const system = client.system();
  assert.equal((await system.versions()).target_os, 'linux');
  const schemas = await system.schemas();
  assert.ok(schemas.includes('recovery-report.schema.json'));
  for (const schema of schemas) assert.equal(typeof await system.schema(schema), 'object');
  await assert.rejects(system.schema('no-such-schema'));
  assert.equal((await system.capabilities()).capabilities.find(item => item.id === 'receipt-signing').status, 'ready');
});

test('live restart failure retains predecessor completion and exact orphan facts without actor replay', async () => {
  const root = fs.mkdtempSync(path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), 'node-management-'));
  const client = new Termtrace(binary, path.join(root, 'sessions'));
  const rec = await client.launch(['/bin/sh', '-c', 'printf LIVE; read line']);
  const registry = new Registry(bytes(path.join(root, 'registry')), bytes('/bin/false'));
  try {
    await rec.output().waitFor(bytes('LIVE'), 5000);
    const old = await registry.register('live', bytes(rec.path));
    await assert.rejects(registry.restart('live', null, null, 5000), error => {
      assert.equal(error.scope, 'management');
      assert.equal(error.effect, null); assert.equal(error.retry, null);
      assert.equal(error.guidance, 'inspect_no_automatic_retry');
      assert.equal(error.request_id, undefined);
      assert.equal(error.kind, 'recorder_startup'); assert.equal(error.code, 'recorder_startup');
      assert.equal(error.management.operation, 'registry_restart');
      const facts = error.management.facts;
      assert.equal(facts.phase, 'successor_launched');
      assert.deepEqual(Buffer.from(facts.predecessor_path), bytes(rec.path));
      assert.equal(facts.predecessor_completion.session_id, old.session_id);
      assert.ok(facts.predecessor_completion.event_count > 0n);
      assert.equal(typeof facts.predecessor_completion.event_count, 'bigint');
      assert.equal(facts.successor.launched, true); assert.equal(facts.successor.readiness, 'exited');
      for (const key of ['session', 'stdout_log', 'stderr_log']) assert.ok(facts.successor[key] instanceof Uint8Array);
      return true;
    });
    assert.equal((await registry.resolve('live')).session_id, old.session_id);
    const completed = await client.registry(bytes(path.join(root, 'completed'))).register('done', bytes(rec.path));
    assert.equal(completed.recorder_process_start_ticks, null);
  } finally { await rec.stop(); }
});

test('restart alias CAS failure exposes the ready orphan and preserves another alias owner', async () => {
  const root = fs.mkdtempSync(path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), 'node-cas-'));
  const client = new Termtrace(binary, path.join(root, 'sessions'));
  const rec = await client.launch(['/bin/sh', '-c', 'printf LIVE; read line']);
  const other = await client.launch(['/bin/true']); await other.stop();
  const state = bytes(path.join(root, 'registry')), plain = new Registry(state, bytes(binary));
  const entered = path.join(root, 'entered'), release = path.join(root, 'release'), helper = path.join(root, 'helper');
  const q = s => "'" + s.replaceAll("'", "'\\''") + "'";
  fs.writeFileSync(helper, `#!/bin/sh\n: > ${q(entered)}\nwhile [ ! -e ${q(release)} ]; do sleep 0.01; done\nexec ${q(binary)} "$@"\n`, {mode:0o700});
  let outcome;
  try {
    await rec.output().waitFor(bytes('LIVE'), 5000);
    const old = await plain.register('race', bytes(rec.path));
    outcome = new Registry(state, bytes(helper)).restart('race', {command:[bytes('/bin/true')], allow_environment_change:true}, {base:'clear'}, 5000).then(value => ({value}), error => ({error}));
    const deadline = Date.now()+5000;
    while (!fs.existsSync(entered) && Date.now()<deadline) await new Promise(r => setTimeout(r,10));
    assert.ok(fs.existsSync(entered));
    await plain.remove('race'); const winner = await plain.register('race', bytes(other.path));
    fs.writeFileSync(release, '');
    const result = await outcome; assert.ok(result.error); const error = result.error;
    assert.equal(error.scope, 'management'); assert.equal(error.effect, null); assert.equal(error.retry, null);
    assert.equal(error.kind, 'engine'); const facts = error.management.facts;
    assert.equal(facts.phase, 'alias_retarget'); assert.equal(facts.predecessor_completion.session_id, old.session_id);
    assert.ok(['live','completed'].includes(facts.successor.readiness));
    assert.ok(fs.existsSync(path.join(Buffer.from(facts.successor.session).toString('utf8'),'session.json')));
    assert.equal((await plain.resolve('race')).session_id, winner.session_id);
  } finally {
    fs.writeFileSync(release, ''); if (outcome) await outcome;
    await rec.stop(); await other.stop();
  }
});

test('nonUTF8 source debug/forensic bundles retain exact verification paths and reject wrong source/tamper', async () => {
  const root = fs.mkdtempSync(path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), 'node-verification-path-'));
  const client = new Termtrace(binary, path.join(root, 'sessions'));
  const rec = await client.launch(['/bin/sh', '-c', 'printf BYTEBOUND']); await rec.stop();
  const other = await client.launch(['/bin/true']); await other.stop();
  const before = hashes(rec.path);
  let source = Buffer.concat([bytes(root + '/source-'), Buffer.from([255])]);
  fs.renameSync(bytes(rec.path), source); source = fs.realpathSync.native(source, {encoding:'buffer'});
  const api = client.evidence(source), native = await api.verify('content');
  assert.equal(native.valid, true); assert.deepEqual(Buffer.from(native.session), source);
  const profiles = [];
  for (const profile of ['debug', 'forensic']) {
    const destination = Buffer.concat([bytes(root + '/bundle-'), Buffer.from([254]), bytes('-' + profile)]);
    const report = await api.bundle(destination, profile);
    assert.deepEqual(await api.verifyBundle(destination, profile), report);
    await assert.rejects(client.evidence(bytes(other.path)).verifyBundle(destination, profile));
    const file = Buffer.concat([destination, bytes('/verification.json')]);
    const encoded = fs.readFileSync(file);
    const value = JSON.parse(encoded.toString('utf8'));
    assert.deepEqual(Buffer.from(value.session), source);
    assert.equal(value.valid, true); assert.equal(value.session_id, native.session_id);
    fs.appendFileSync(file, 'TAMPER'); await assert.rejects(api.verifyBundle(destination, profile));
    fs.writeFileSync(file, encoded); assert.deepEqual(await api.verifyBundle(destination, profile), report);
    assert.deepEqual(hashes(source), before); profiles.push(profile);
  }
  fs.writeFileSync(path.join(root, 'verification-path-observed.json'), JSON.stringify({source_base64:source.toString('base64'), profiles, source_hashes_before:before, source_hashes_after:hashes(source)}, null, 2));
});

function ffmpegWithoutVp9(directory) {
  fs.mkdirSync(directory);
  const real = spawnSync('sh', ['-c', 'command -v ffmpeg'], { encoding: 'utf8' }).stdout.trim();
  const wrapper = path.join(directory, 'ffmpeg');
  fs.writeFileSync(wrapper, `#!/bin/sh\ncase " $* " in *libvpx-vp9*) echo 'Unknown encoder libvpx-vp9' >&2; exit 1;; esac\nexec '${real}' "$@"\n`, { mode: 0o700 });
  return directory;
}

test('delivery codec availability follows the ffmpeg on PATH', async (t) => {
  const { Edit, select } = require('@termtrace/core');
  const root = fs.mkdtempSync(path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), 'node-codecs-'));
  const client = new Termtrace(binary, path.join(root, 'sessions'));
  const rec = await client.launch(['/bin/sh', '-c', 'printf READY; IFS= read -r line'], { cols: 30, rows: 6 });
  try {
    await rec.output().waitFor(bytes('READY'), 5000);
    await rec.mark('start');
    await rec.mark('end');
    await rec.stop();
  } finally { if (rec.isActive()) await rec.stop(); }
  const edit = new Edit().clip(select.marker('start'), select.marker('end'));
  const capabilities = async () => Object.fromEntries((await client.system().capabilities()).capabilities.map(item => [item.id, item]));

  const host = await capabilities();
  assert.equal(host['gif-delivery'].status, 'ready');
  assert.equal(host['webm-delivery'].status, 'ready');

  const original = process.env.PATH;
  t.after(() => { process.env.PATH = original; });
  process.env.PATH = `${ffmpegWithoutVp9(path.join(root, 'tools'))}${path.delimiter}${original}`;
  const limited = await capabilities();
  assert.equal(limited['gif-delivery'].status, 'ready');
  assert.equal(limited['webm-delivery'].status, 'blocked');
  assert.equal(limited['webm-delivery'].code, 'derivative_encoder_unavailable');
  assert.ok(limited['webm-delivery'].remediation);

  const out = path.join(root, 'blocked.webm');
  await assert.rejects(client.videoRender(rec.path, edit, out, { renderer: 'semantic' }), /webm-delivery.*derivative_encoder_unavailable/s);
  assert.equal(fs.existsSync(out), false);
  const gif = await client.videoRender(rec.path, edit, path.join(root, 'allowed.gif'), { renderer: 'semantic' });
  assert.ok(fs.statSync(gif.output).isFile());
});
