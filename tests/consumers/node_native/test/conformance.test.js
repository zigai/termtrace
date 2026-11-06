"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const { Termtrace } = require("@termtrace/core");

const repo = path.resolve(__dirname, "../../../..");
const fixture = JSON.parse(
    fs.readFileSync(path.join(repo, "fixtures/conformance/sdk/cases/foundation.json")),
);
const binary = process.env.TERMTRACE_BIN || path.join(repo, "target/debug/termtrace");

function processIsGone(pidFile) {
    const pid = Number(fs.readFileSync(pidFile, "utf8"));
    return pid > 1 && !fs.existsSync(`/proc/${pid}`);
}

test("foundation case matches the shared fixture", async (t) => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "termtrace-conformance-"));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    const tt = new Termtrace(binary, path.join(root, "evidence"));
    const rec = await tt.record(
        [...fixture.command, path.join(root, "child.pid")],
        fixture.rows,
        fixture.cols,
    );
    let frame;
    let stale;
    let page;
    let failed = false;
    let primary;
    try {
        await rec.screen().waitForText(fixture.text, fixture.timeout_ms);
        frame = await rec.screen().frame();
        const action = rec.actions();
        action.ifState(fixture.guard);
        action.sendText("FORBIDDEN\n");
        await assert.rejects(
            () => action.execute(),
            (error) => {
                stale = {
                    kind: error.kind,
                    code: error.code,
                    effect: error.effect,
                    retry: error.retry,
                };
                return true;
            },
        );
        page = await rec.readOutput(0n, 4096);
    } catch (error) {
        failed = true;
        primary = error;
    }
    try {
        await rec.stop();
    } catch (error) {
        if (failed) console.error("Secondary SDK cleanup failure:", error);
        else {
            failed = true;
            primary = error;
        }
        try {
            rec.forceClose();
        } catch (secondary) {
            console.error("SDK force-close failure:", secondary);
        }
    }
    if (failed) throw primary;

    assert.deepEqual([frame.rows, frame.cols], [fixture.rows, fixture.cols]);
    assert.ok(frame.text.includes(fixture.text));
    assert.deepEqual(stale, {
        kind: "stale_state",
        code: "stale_observation",
        effect: { kind: "not_applied" },
        retry: "new_request",
    });
    const output = Buffer.from(page.bytes);
    assert.equal(output.toString("hex"), fixture.output_hex);
    assert.equal(rec.isActive(), false);

    const completion = JSON.parse(fs.readFileSync(path.join(rec.path, "completion.json")));
    assert.equal(completion.output_bytes, output.length);
    assert.equal(completion.failure ?? null, null);
    const kinds = fs
        .readFileSync(path.join(rec.path, "events.jsonl"), "utf8")
        .split("\n")
        .slice(1)
        .filter(Boolean)
        .map((line) => JSON.parse(line).event?.type);
    for (const kind of ["input", "input_intent", "input_result"]) {
        assert.ok(!kinds.includes(kind), `stale guard emitted ${kind}`);
    }
    assert.ok(processIsGone(path.join(root, "child.pid")), "child was not reaped");
    assert.ok(processIsGone(path.join(root, "child.pid.recorder")), "recorder was not reaped");
});
