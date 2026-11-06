const os = require("os");
const path = require("path");
const fs = require("fs");
const native = require("./index");
const { Termtrace } = native;

/**
 * Execute a test function within an isolated Termtrace environment.
 * Stops all tracked recordings and deletes evidence only after complete success.
 * On failure, reports the retained evidence path without replacing the primary error.
 * Extensible errors also receive `termtrace: { evidenceRoot, cleanupErrors }`.
 *
 * @param {Function} fn - async function ({ termtrace, record, evidenceRoot })
 * @param {Object} [options]
 * @param {string} [options.binary] - Optional path to termtrace binary
 */
async function withTermtrace(fn, options = {}) {
    const policy = options.evidencePolicy ?? "on_failure";
    const timeout = options.finalizationTimeoutMs;
    if (!["on_failure", "always", "manual"].includes(policy)) {
        throw new TypeError("invalid evidencePolicy");
    }
    if (timeout !== undefined && (!Number.isSafeInteger(timeout) || timeout < 0)) {
        throw new RangeError("finalizationTimeoutMs must be a non-negative safe integer");
    }
    const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), "termtrace-test-"));
    const activeRecordings = [];
    const pending = new Set();
    let closing = false;
    let finalization;
    let report;
    const finalize = (outcome = "passed") => {
        if (finalization) return finalization;
        if (!["passed", "assertion_failed", "error", "panic", "timed_out"].includes(outcome)) {
            throw new TypeError("invalid finalization outcome");
        }
        closing = true;
        finalization = (async () => {
            await Promise.allSettled([...pending]);
            report = await native.finalizeRecordings(
                activeRecordings,
                tmpdir,
                cleanupErrors.length ? "error" : outcome,
                policy,
                timeout,
            );
            for (const diagnostic of report.secondary_failures) {
                const error = new Error(`${diagnostic.phase}: ${diagnostic.message}`);
                error.phase = diagnostic.phase;
                cleanupErrors.push(error);
            }
            return report;
        })();
        return finalization;
    };
    const cleanupErrors = [];
    let failed = false;
    let failure;
    let result;

    try {
        const tt = new Termtrace(options.binary, tmpdir);
        const record = (...args) => {
            if (closing) return Promise.reject(new Error("terminal test is finalized"));
            const worker = Promise.resolve()
                .then(() => tt.record(...args))
                .then((rec) => {
                    activeRecordings.push(rec);
                    return rec;
                });
            pending.add(worker);
            worker.then(
                () => pending.delete(worker),
                (error) => {
                    pending.delete(worker);
                    cleanupErrors.push(error);
                },
            );
            return worker;
        };
        result = await fn({
            termtrace: tt,
            record,
            evidenceRoot: tmpdir,
            finalize,
            get report() {
                return report;
            },
        });
    } catch (error) {
        failed = true;
        failure = error;
    }

    try {
        await finalize(failed ? "error" : "passed");
    } catch (error) {
        cleanupErrors.push(error);
    }

    if (failed || cleanupErrors.length > 0) {
        const error = failed
            ? failure
            : new AggregateError(cleanupErrors, "Termtrace test cleanup failed");
        const diagnostics = {
            evidenceRoot: fs.existsSync(tmpdir) ? tmpdir : null,
            cleanupErrors,
            report,
        };

        // Frozen errors, primitive throws, and existing runner metadata must not change the
        // test's primary failure. The stderr path remains available in all of these cases.
        try {
            if (
                error !== null &&
                (typeof error === "object" || typeof error === "function") &&
                Object.isExtensible(error) &&
                !Object.hasOwn(error, "termtrace")
            ) {
                Object.defineProperty(error, "termtrace", {
                    value: diagnostics,
                    configurable: true,
                });
            }
        } catch (_) {
            // Attaching optional diagnostics is best-effort; the original failure is authoritative.
        }

        console.error(
            `Termtrace test evidence: ${diagnostics.evidenceRoot ?? "no surviving evidence"}`,
        );
        for (const cleanupError of cleanupErrors) {
            console.error("Termtrace test cleanup error:", cleanupError);
        }
        throw error;
    }

    return result;
}

/** Negate only a Termtrace expectation mismatch; infrastructure failures are never success. */
function assertNot(expectation) {
    let result;
    try {
        result = expectation();
    } catch (error) {
        if (error instanceof native.ExpectationMismatchError) return;
        throw error;
    }
    if (result && typeof result.then === "function") {
        return result.then(
            () => {
                native.failNegation();
            },
            (error) => {
                if (!(error instanceof native.ExpectationMismatchError)) throw error;
            },
        );
    }
    native.failNegation();
}
module.exports = { withTermtrace, assertNot };
// Static assignments also expose these names to Node ESM synthetic named imports.
module.exports.ExpectationMismatchError = native.ExpectationMismatchError;
module.exports.normalizedSnapshot = native.normalizedSnapshot;
module.exports.maskedSnapshot = native.maskedSnapshot;
module.exports.assertSnapshotFile = native.assertSnapshotFile;
module.exports.assertMaskedSnapshotFile = native.assertMaskedSnapshotFile;
module.exports.assertScreenText = native.assertScreenText;
module.exports.assertCursor = native.assertCursor;
module.exports.assertNoWrappedRows = native.assertNoWrappedRows;
module.exports.assertNeutralRegion = native.assertNeutralRegion;
module.exports.assertNoPresentedTransient = native.assertNoPresentedTransient;
module.exports.assertExists = native.assertExists;
module.exports.assertNotExists = native.assertNotExists;
module.exports.assertCount = native.assertCount;
module.exports.assertMatchText = native.assertMatchText;
module.exports.assertMatchStyle = native.assertMatchStyle;
