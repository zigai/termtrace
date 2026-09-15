const os = require('os');
const path = require('path');
const fs = require('fs');
const { Termtrace } = require('./index');

/**
 * Execute a test function within an isolated Termtrace environment.
 * Automatically cleans up temporary directories and stops running sessions.
 *
 * @param {Function} fn - async function ({ termtrace, record })
 * @param {Object} [options]
 * @param {string} [options.binary] - Optional path to termtrace binary
 */
async function withTermtrace(fn, options = {}) {
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'termtrace-test-'));
  const activeRecordings = [];

  const tt = new Termtrace(options.binary, tmpdir);

  const record = async (...args) => {
    const rec = await tt.record(...args);
    activeRecordings.push(rec);
    return rec;
  };

  try {
    return await fn({ termtrace: tt, record, evidenceRoot: tmpdir });
  } finally {
    for (const rec of activeRecordings) {
      if (rec.isActive()) {
        try {
          await rec.stop();
        } catch (_) {}
      }
    }
    try {
      fs.rmSync(tmpdir, { recursive: true, force: true });
    } catch (_) {}
  }
}

module.exports = {
  withTermtrace,
};
