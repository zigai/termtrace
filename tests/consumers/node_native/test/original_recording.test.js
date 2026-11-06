// Original recordings against the real Ghostty, Xvfb and FFmpeg toolchain.
const test = require("node:test");
const assert = require("node:assert");
const os = require("node:os");
const path = require("node:path");
const fs = require("node:fs");

const { Termtrace } = require("@termtrace/core");

const TERMTRACE_BIN = process.env.TERMTRACE_BIN
    ? path.resolve(process.env.TERMTRACE_BIN)
    : path.resolve(__dirname, "../../../../target/debug/termtrace");
const OPTIONS = {
    grid: { cols: 80, rows: 24 },
    canvas: { width: 1280, height: 720 },
    startup_timeout_ms: 60000,
};
const LOOP = "trap 'echo winch $(stty size)' WINCH; echo ready; while :; do sleep 0.05; done";

function root(t) {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), "node-original-"));
    t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
    return directory;
}

function children(pid) {
    return fs
        .readdirSync("/proc")
        .filter((name) => /^\d+$/.test(name))
        .filter((name) => {
            try {
                const stat = fs.readFileSync(`/proc/${name}/stat`, "utf8");
                return Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]) === pid;
            } catch {
                return false;
            }
        })
        .map(Number);
}

function alive(pid) {
    try {
        const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
        return stat[stat.lastIndexOf(")") + 2] !== "Z";
    } catch {
        return false;
    }
}

async function waitForOutput(original, text) {
    const deadline = Date.now() + 20000;
    while (Date.now() < deadline) {
        if (await original.session().waitForOutput(text, 200)) {
            return;
        }
    }
    assert.fail(`${text} did not appear`);
}

test("resizes inside the canvas and publishes the capture", async (t) => {
    const tt = new Termtrace(TERMTRACE_BIN, root(t));
    const original = await tt.recordOriginal(["/bin/sh", "-c", LOOP], OPTIONS);
    assert.strictEqual((await original.captureStatus()).state, "live");
    await waitForOutput(original, "ready");

    await original.session().resize(100, 30);
    await waitForOutput(original, "winch 30 100");
    await assert.rejects(original.session().resize(400, 200), (error) => {
        assert.strictEqual(error.code, "exceeds_native_canvas");
        assert.deepStrictEqual(error.effect, { kind: "not_applied" });
        return true;
    });

    const result = await original.finish(60000);
    assert.deepStrictEqual(result.capture.provenance, { kind: "original_execution" });
    assert.strictEqual(typeof result.capture.source.boundary.output_bytes, "bigint");
    assert.strictEqual(result.completion.session_id, original.sessionId);
    assert.strictEqual(result.canonical_success, false);
    assert.strictEqual(original.isActive(), false);
});

test("a failing application still publishes its capture", async (t) => {
    const tt = new Termtrace(TERMTRACE_BIN, root(t));
    const original = await tt.recordOriginal(["/bin/sh", "-c", "echo failing; exit 7"], OPTIONS);

    const result = await original.waitFinished(60000);
    assert.strictEqual(result.completion.exit_code, 7);
    assert.strictEqual(result.canonical_success, false);
});

test("a killed encoder rejects with canonical completion", async (t) => {
    const tt = new Termtrace(TERMTRACE_BIN, root(t));
    const original = await tt.recordOriginal(["/bin/sh", "-c", LOOP], OPTIONS);
    const supervisor = Number((await original.captureStatus()).supervisor_pid);
    const encoders = children(supervisor)
        .flatMap(children)
        .filter((pid) => fs.readFileSync(`/proc/${pid}/comm`, "utf8").startsWith("ffmpeg"));
    assert.strictEqual(encoders.length, 1);
    process.kill(encoders[0], "SIGKILL");

    await assert.rejects(original.finish(60000), (error) => {
        assert.strictEqual(error.kind, "original_capture_failed");
        assert.strictEqual(error.code, "acquisition_failed");
        assert.strictEqual(error.scope, "management");
        assert.notStrictEqual(error.completion, null);
        assert.ok(error.evidence.some((item) => item.kind === "session"));
        return true;
    });
    assert.strictEqual(alive(supervisor), false);
});

test("aborting startup tears the capture down before rejecting", async (t) => {
    const directory = root(t);
    const tt = new Termtrace(TERMTRACE_BIN, directory);
    const controller = new AbortController();
    const started = tt.recordOriginal(["/bin/sh", "-c", "sleep 30"], {
        ...OPTIONS,
        signal: controller.signal,
    });
    const deadline = Date.now() + 30000;
    while (!fs.readdirSync(directory).some((name) => name.endsWith(".original-capture.json"))) {
        assert.ok(Date.now() < deadline, "acquisition never started");
        await new Promise((resolve) => setTimeout(resolve, 10));
    }
    controller.abort();

    await assert.rejects(started, (error) => {
        assert.strictEqual(error.code, "cancelled");
        return true;
    });
    const sidecar = fs
        .readdirSync(directory)
        .find((name) => name.endsWith(".original-capture.json"));
    const status = JSON.parse(fs.readFileSync(path.join(directory, sidecar), "utf8"));
    assert.strictEqual(alive(Number(status.supervisor_pid)), false);
});

test("conflicting geometry is rejected before anything starts", async (t) => {
    const tt = new Termtrace(TERMTRACE_BIN, root(t));
    await assert.rejects(
        tt.recordOriginal(["/bin/true"], {
            grid: { cols: 80, rows: 24 },
            viewport: { width: 800, height: 600 },
        }),
        (error) => error.kind === "invalid_operation",
    );
    await assert.rejects(tt.recordOriginal(["/bin/true"], { rows: 24 }), TypeError);
});

const { spawnSync } = require("node:child_process");

function startNamed(state, session, alias, script = LOOP) {
    // The shell turns the octal escape into the exact non-UTF-8 byte the path needs.
    const started = spawnSync(
        "/bin/sh",
        [
            "-c",
            'exec "$0" record --original-native --original-grid 80x24 --original-canvas 1280x720 -d "$1" --out "$(printf "$2")" -- /bin/sh -c "$3"',
            TERMTRACE_BIN,
            alias,
            session,
            script,
        ],
        { env: { ...process.env, TERMTRACE_STATE_DIR: state }, timeout: 60000 },
    );
    assert.strictEqual(started.status, 0, String(started.stderr));
}

function rewriteStatus(sidecar, status) {
    const staged = Buffer.concat([sidecar, Buffer.from(".rewrite")]);
    fs.writeFileSync(staged, JSON.stringify(status), { mode: 0o600, flag: "wx" });
    fs.renameSync(staged, sidecar);
}

test("a named background recording attaches by alias for control and finish", async (t) => {
    const directory = root(t);
    const state = path.join(directory, "state");
    const tt = new Termtrace(TERMTRACE_BIN, path.join(directory, "evidence"));
    const started = await tt.recordOriginal(["/bin/sh", "-c", LOOP], {
        ...OPTIONS,
        alias: "node-attach",
        state_root: state,
    });
    const sessionId = started.sessionId;
    started.detach();

    const named = await tt.attachOriginalTarget("name:node-attach", state);
    assert.strictEqual(named.sessionId, sessionId);
    const status = await named.captureStatus();
    assert.strictEqual(status.state, "live");
    assert.strictEqual(typeof status.supervisor_pid, "bigint");
    await waitForOutput(named, "ready");
    await named.session().resize(100, 30);
    await waitForOutput(named, "winch 30 100");

    const byBytes = await tt.attachOriginalTarget(Buffer.from(named.pathBytes));
    assert.deepStrictEqual(Buffer.from(byBytes.pathBytes), Buffer.from(named.pathBytes));
    await assert.rejects(tt.attachOriginalTarget("name:node-missing", Buffer.from(state)));

    const result = await named.finish(60000);
    assert.strictEqual(result.completion.session_id, sessionId);
    assert.deepStrictEqual(result.capture.provenance, { kind: "original_execution" });
    assert.strictEqual((await byBytes.captureStatus()).state, "published");
});

const CONTROL_LOOP =
    "(trap 'echo winch $(stty size </dev/tty)' WINCH; while :; do sleep 0.05; done) & echo ready; while read line; do echo got $line; done";

test("an exact byte path attaches, controls and finishes a non-UTF-8 session", async (t) => {
    const directory = root(t);
    const state = path.join(directory, "state");
    const evidence = Buffer.concat([Buffer.from(`${directory}/evidence-`), Buffer.from([0xff])]);
    fs.mkdirSync(evidence);
    const session = Buffer.concat([evidence, Buffer.from("/session")]);
    startNamed(state, `${directory}/evidence-\\377/session`, "node-bytes", CONTROL_LOOP);
    const tt = new Termtrace(TERMTRACE_BIN, directory);

    const exact = await tt.attachOriginalTarget(session);
    assert.deepStrictEqual(Buffer.from(exact.pathBytes), session);
    assert.strictEqual((await exact.captureStatus()).state, "live");
    const control = exact.session();
    assert.deepStrictEqual(Buffer.from(control.pathBytes), session);
    const screen = control.screen();
    const presented = await screen.waitForText("ready", 20000);

    const actions = control.actions();
    actions.ifState(presented.hash);
    actions.sendText("hello");
    actions.sendKey("Enter");
    actions.untilScreen("got hello", 20000);
    assert.ok((await actions.execute()).text.includes("got hello"));

    await control.resize(100, 30);
    await screen.waitForText("winch 30 100", 20000);
    const named = await tt.attachOriginalTarget("node-bytes", state);
    assert.strictEqual(named.sessionId, exact.sessionId);

    const result = await exact.finish(60000);
    const captured = Buffer.from(result.capture.directory_bytes);
    assert.deepStrictEqual(
        captured.subarray(0, session.length + 1),
        Buffer.concat([session, Buffer.from("/")]),
    );
    assert.strictEqual((await named.captureStatus()).state, "published");
});

test("capture status carries exact bigint counters and rejects malformed decimals", async (t) => {
    const tt = new Termtrace(TERMTRACE_BIN, root(t));
    const original = await tt.recordOriginal(["/bin/sh", "-c", "echo done"], OPTIONS);
    await original.waitFinished(60000);
    const sidecar = Buffer.concat([
        Buffer.from(original.pathBytes),
        Buffer.from(".original-capture.json"),
    ]);
    const published = fs.readFileSync(sidecar, "utf8");

    const wide = JSON.parse(published);
    wide.supervisor_pid = "18446744073709551615";
    wide.supervisor_start_ticks = "9007199254740993";
    wide.result.source.boundary.output_bytes = "18446744073709551615";
    wide.result.members[0].byte_length = "9007199254740993";
    rewriteStatus(sidecar, wide);
    let status = await original.captureStatus();
    assert.strictEqual(status.supervisor_pid, 18446744073709551615n);
    assert.strictEqual(status.supervisor_start_ticks, 9007199254740993n);
    assert.strictEqual(status.result.source.boundary.output_bytes, 18446744073709551615n);
    assert.strictEqual(status.result.members[0].byte_length, 9007199254740993n);
    assert.strictEqual(status.result.members[0].schema_type, null);
    assert.ok(status.result.directory_bytes instanceof Uint8Array);

    const failed = JSON.parse(published);
    Object.assign(failed, {
        state: "failed",
        result: null,
        failure: {
            code: "recording_failed",
            reason: "18446744073709551615",
            surviving: [],
            cleanup: null,
        },
    });
    rewriteStatus(sidecar, failed);
    status = await original.captureStatus();
    assert.strictEqual(status.result, null);
    assert.strictEqual(status.failure.reason, "18446744073709551615");
    assert.strictEqual(status.failure.cleanup, null);

    for (const bad of ["01", "-1", "18446744073709551616", 7]) {
        const malformed = JSON.parse(published);
        malformed.supervisor_pid = bad;
        rewriteStatus(sidecar, malformed);
        await assert.rejects(original.captureStatus());
    }
});

function parentOf(pid) {
    const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
    return Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
}

function sidecarOf(session) {
    return Buffer.concat([Buffer.from(session), Buffer.from(".original-capture.json")]);
}

test("a sidecar swapped after attach is a capture failure", async (t) => {
    const tt = new Termtrace(TERMTRACE_BIN, root(t));
    const first = await tt.recordOriginal(["/bin/sh", "-c", "echo first"], OPTIONS);
    await first.waitFinished(60000);
    const second = await tt.recordOriginal(["/bin/sh", "-c", "echo second"], OPTIONS);
    await second.waitFinished(60000);
    const other = fs.readFileSync(sidecarOf(first.pathBytes));
    const own = fs.readFileSync(sidecarOf(second.pathBytes));

    const attached = await tt.attachOriginalTarget(Buffer.from(second.pathBytes));
    rewriteStatus(sidecarOf(second.pathBytes), JSON.parse(other));
    await assert.rejects(attached.captureStatus());
    await assert.rejects(attached.waitFinished(60000), (error) => {
        assert.strictEqual(error.kind, "original_capture_failed");
        assert.strictEqual(error.code, "publication_failed");
        assert.strictEqual(error.completion.session_id, second.sessionId);
        return true;
    });

    const redirected = JSON.parse(own);
    redirected.result.directory_bytes = JSON.parse(other).result.directory_bytes;
    rewriteStatus(sidecarOf(second.pathBytes), JSON.parse(own));
    const moved = await tt.attachOriginalTarget(Buffer.from(second.pathBytes));
    rewriteStatus(sidecarOf(second.pathBytes), redirected);
    await assert.rejects(moved.waitFinished(60000), (error) => error.code === "publication_failed");

    rewriteStatus(sidecarOf(second.pathBytes), JSON.parse(own));
    const restored = await tt.attachOriginalTarget(Buffer.from(second.pathBytes));
    const result = await restored.waitFinished(60000);
    assert.strictEqual(result.capture.source.session_id, second.sessionId);
});

test("the owner stops a HUP-ignoring application after supervisor loss", async (t) => {
    const tt = new Termtrace(TERMTRACE_BIN, root(t));
    const original = await tt.recordOriginal(
        ["/bin/sh", "-c", "trap '' HUP TERM; echo stubborn; while :; do sleep 0.05; done"],
        OPTIONS,
    );
    const supervisor = Number((await original.captureStatus()).supervisor_pid);
    const status = await original.session().status();
    assert.strictEqual(status.kind, "live");
    const application = status.pid;
    for (const pid of [supervisor, parentOf(application)]) {
        process.kill(pid, "SIGKILL");
    }

    const deadline = Date.now() + 15000;
    while (alive(application)) {
        assert.ok(Date.now() < deadline, "the application outlived its supervisor");
        await new Promise((resolve) => setTimeout(resolve, 50));
    }
    await assert.rejects(
        original.waitFinished(60000),
        (error) => error.code === "supervisor_abandoned",
    );
});
