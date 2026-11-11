const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { Tmux } = require("@zigai/termtrace");
const binary =
    process.env.TERMTRACE_BIN || path.resolve(__dirname, "../../../../target/debug/termtrace");
function command(socket, ...args) {
    return spawnSync("/usr/bin/tmux", ["-S", socket, ...args], { encoding: "utf8" });
}
async function until(operation, predicate) {
    const deadline = Date.now() + 8000;
    for (;;) {
        try {
            const value = await operation();
            if (predicate(value)) {
                return value;
            }
        } catch (error) {
            // A new private server creates its socket after record returns.
            if (Date.now() >= deadline) {
                throw error;
            }
        }
        assert.ok(Date.now() < deadline, "private tmux condition did not become true");
        await new Promise((resolve) => setTimeout(resolve, 25));
    }
}

test("native Node private-socket track lifecycle selects Termtrace, never the Node host", async () => {
    const root = fs.mkdtempSync(
        path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), "node-tmux-"),
    );
    const socket = path.join(root, "private.sock");
    const helper = path.join(root, "selected ' helper");
    fs.symlinkSync(binary, helper);
    const api = new Tmux(
        Buffer.from(path.join(root, "state ' quoted")),
        Buffer.from(helper),
        Buffer.from("/usr/bin/tmux"),
        Buffer.from(socket),
    );
    let started = false;
    try {
        const report = await api.record({
            name: "native",
            new_server: true,
            ghostty_profile: "off",
            command: [
                Buffer.from("/usr/bin/tmux"),
                Buffer.from("new-session"),
                Buffer.from("-s"),
                Buffer.from("native"),
            ],
        });
        started = true;
        const audit = await until(
            () => api.status(),
            (value) => value.recorded_panes >= 1n,
        );
        assert.deepEqual(
            Buffer.from(audit.configured_executable),
            Buffer.from(fs.realpathSync(binary)),
        );
        assert.equal(audit.enabled, true);
        assert.equal(audit.default_command_matches, true);
        const tracks = await until(
            () => api.catalog("native"),
            (value) => value.entries.length >= 1,
        );
        assert.equal(tracks.kind, "tracks");
        const entry = tracks.entries[0];
        assert.equal(typeof entry.track.linked_unix_ns, "bigint");
        assert.ok(entry.track.session instanceof Uint8Array);
        await api.track(report.manifest, entry.track.pane_id, entry.track.session);
        await assert.rejects(api.track(report.manifest, "%987654321", entry.track.session));
        await assert.rejects(
            api.track(
                report.manifest,
                entry.track.pane_id,
                Buffer.from(path.join(root, "missing-source")),
            ),
        );
        const gaps = await api.catalog("native", true);
        assert.equal(gaps.kind, "gaps");
        assert.ok(gaps.entries.length >= 1);
        const history = await api.events("native", 1n, 1000);
        assert.ok(history.page.events.length);
        const ended = await api.stop("native", true, 10000);
        assert.equal(ended.lifecycle, "completed");
        const again = await api.stop("native", true, 10000);
        assert.deepEqual(again.lifecycle_journal, ended.lifecycle_journal);
        const closed = await api.catalog("native");
        assert.equal(closed.summary.unfinished_tracks, 0n);
        assert.ok(closed.entries.every((item) => item.completed));
        const bundlePath = Buffer.from(path.join(root, "aggregate-bundle"));
        const bundle = await api.bundle(report.manifest, bundlePath);
        assert.ok(bundle.track_count >= 2n);
        assert.deepEqual(await api.verifyBundle(report.manifest, bundlePath), bundle);
        await assert.rejects(api.bundle(report.manifest, bundlePath));
        const mismatchDir = path.join(root, "mismatch", "native");
        fs.cpSync(path.dirname(Buffer.from(report.manifest).toString("utf8")), mismatchDir, {
            recursive: true,
        });
        const mismatchFile = path.join(mismatchDir, "manifest.json");
        const altered = fs
            .readFileSync(mismatchFile, "utf8")
            .replace(/("recording_id"\s*:\s*")[^"]+"/, '$100000000-0000-0000-0000-000000000000"');
        fs.writeFileSync(mismatchFile, altered);
        await assert.rejects(api.verifyBundle(Buffer.from(mismatchFile), bundlePath));
        fs.writeFileSync(
            path.join(root, "tmux-observed.json"),
            JSON.stringify(
                {
                    selected_helper_hex: Buffer.from(audit.configured_executable).toString("hex"),
                    track_count: closed.entries.length,
                    gap_count: gaps.entries.length,
                    lifecycle: ended.lifecycle,
                },
                null,
                2,
            ),
        );
    } finally {
        if (started) await api.stop("native", true, 10000);
        command(socket, "kill-server");
    }
    assert.notEqual(command(socket, "list-sessions").status, 0);
});

test("native Node existing-server track admission failure restores integration options", async () => {
    const root = fs.mkdtempSync(
        path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), "node-tmux-rollback-"),
    );
    const socket = path.join(root, "rollback.sock");
    assert.equal(
        command(socket, "-f", "/dev/null", "new-session", "-d", "-s", "external").status,
        0,
    );
    const original = command(socket, "show-option", "-gv", "default-command").stdout;
    const api = new Tmux(
        Buffer.from(path.join(root, "state")),
        Buffer.from("/bin/false"),
        Buffer.from("/usr/bin/tmux"),
        Buffer.from(socket),
    );
    try {
        await assert.rejects(
            api.record({
                name: "failed",
                ghostty_profile: "off",
                command: [
                    Buffer.from("/usr/bin/tmux"),
                    Buffer.from("attach-session"),
                    Buffer.from("-t"),
                    Buffer.from("external"),
                ],
            }),
        );
        assert.equal(command(socket, "show-option", "-gv", "default-command").stdout, original);
        assert.equal(
            command(socket, "show-option", "-gqv", "@termtrace-recording-version").stdout.trim(),
            "",
        );
        assert.equal(fs.existsSync(path.join(root, "state/tmux-recordings/failed")), false);
        assert.equal(command(socket, "has-session", "-t", "external").status, 0);
    } finally {
        command(socket, "kill-server");
    }
    assert.notEqual(command(socket, "list-sessions").status, 0);
});

test("nonUTF8 pane map values and committed aggregate stop have exact non-actor facts", async () => {
    const { Registry } = require("@zigai/termtrace");
    const root = fs.mkdtempSync(path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), "n-tm-"));
    const socket = path.join(root, "s");
    const state = Buffer.concat([Buffer.from(root + "/state-"), Buffer.from([255])]);
    const api = new Tmux(
        state,
        Buffer.from(binary),
        Buffer.from("/usr/bin/tmux"),
        Buffer.from(socket),
    );
    let paused;
    try {
        const start = await api.record({
            name: "partial",
            new_server: true,
            ghostty_profile: "off",
            command: [
                Buffer.from("/usr/bin/tmux"),
                Buffer.from("new-session"),
                Buffer.from("-s"),
                Buffer.from("native"),
            ],
        });
        await until(
            () => api.status(),
            (v) => v.recorded_panes >= 1n,
        );
        const manifest = await until(
            () => api.recordingStatus("partial"),
            (v) => Object.keys(v.pane_tracks).length,
        );
        for (const value of [
            ...Object.values(manifest.pane_tracks),
            ...Object.values(start.recording.pane_tracks),
        ]) {
            assert.ok(value instanceof Uint8Array);
            assert.ok(Buffer.from(value).includes(255));
        }
        const gaps = await api.catalog("partial", true);
        assert.ok(gaps.entries.some((v) => v.gap.resolved_unix_ns === null));
        paused = (await new Registry(state, Buffer.from(binary)).resolve("tmux-partial-composite"))
            .recorder_pid;
        assert.equal(typeof paused, "number");
        process.kill(paused, "SIGSTOP");
        await assert.rejects(api.stop("partial", true, 30), (error) => {
            assert.equal(error.scope, "management");
            assert.equal(error.effect, null);
            assert.equal(error.retry, null);
            assert.equal(error.guidance, "inspect_no_automatic_retry");
            assert.equal(error.management.operation, "tmux");
            const facts = error.management.facts;
            assert.equal(facts.stop_requested, true);
            assert.equal(facts.phase, "stop_requested");
            assert.deepEqual(Buffer.from(facts.manifest), Buffer.from(start.manifest));
            assert.deepEqual(facts.completed_track_stops, []);
            assert.equal(error.kind, "control_timeout");
            return true;
        });
    } finally {
        if (paused) process.kill(paused, "SIGCONT");
        try {
            await api.stop("partial", true, 10000);
        } finally {
            command(socket, "kill-server");
        }
    }
});
