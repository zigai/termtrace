// Native Ghostty replay screenshots and videos of completed sessions.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const { Termtrace, Edit, select } = require("@termtrace/core");

const binary = process.env.TERMTRACE_BIN
    ? path.resolve(process.env.TERMTRACE_BIN)
    : path.resolve(__dirname, "../../../../target/debug/termtrace");
const SCRIPT = "printf FIRST; sleep 0.5; printf ' SECOND'; IFS= read -r line";

function root(t) {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), "node-native-replay-"));
    t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
    return directory;
}

function pngSize(file) {
    const header = fs.readFileSync(file).subarray(0, 24);
    assert.deepEqual(
        header.subarray(0, 8),
        Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    );
    return [header.readUInt32BE(16), header.readUInt32BE(20)];
}

async function completed(directory) {
    const client = new Termtrace(binary, path.join(directory, "sessions"));
    const rec = await client.launch(["/bin/sh", "-c", SCRIPT], { cols: 30, rows: 6 });
    try {
        const first = await rec.screen().waitForText("FIRST", 5000);
        await rec.mark("start");
        const second = await rec.screen().waitForText("SECOND", 5000);
        await rec.mark("end");
        await rec.stop();
        return { client, session: rec.path, first: first.hash, second: second.hash };
    } finally {
        if (rec.isActive()) await rec.stop();
    }
}

test("a screenshot captures the selected state as native pixels", async (t) => {
    const directory = root(t);
    const { client, session, first, second } = await completed(directory);

    const selected = await client.native(session).screenshot(path.join(directory, "first"), {
        point: { selector: "state_hash", state_hash: first },
    });
    const latest = await (
        await client.attach(session)
    )
        .native()
        .screenshot(path.join(directory, "latest"));

    for (const [capture, state] of [
        [selected, first],
        [latest, second],
    ]) {
        const manifest = JSON.parse(
            fs.readFileSync(path.join(capture.directory, "manifest.json"), "utf8"),
        );
        assert.equal(capture.video ?? null, null);
        assert.equal(manifest.capture_kind, "screenshot");
        assert.equal(manifest.guarantee_level, "native_capture_environment");
        assert.match(manifest.ghostty_version, /^Ghostty/);
        assert.equal(manifest.selected_state_hash, state);
        assert.deepEqual([manifest.geometry_report.cols, manifest.geometry_report.rows], [30, 6]);
        assert.deepEqual(pngSize(capture.screenshot), [manifest.width, manifest.height]);
        const image = manifest.members.find(
            (member) => member.relationship === "native_final_frame",
        );
        assert.equal(path.join(capture.directory, image.path), capture.screenshot);
    }
    assert.notDeepEqual(fs.readFileSync(selected.screenshot), fs.readFileSync(latest.screenshot));
});

test("a screenshot never replaces an existing destination", async (t) => {
    const directory = root(t);
    const { client, session } = await completed(directory);
    const native = client.native(session);
    const destination = path.join(directory, "shot");
    await native.screenshot(destination);
    const snapshot = () =>
        Object.fromEntries(
            fs
                .readdirSync(destination)
                .map((name) => [name, fs.readFileSync(path.join(destination, name))]),
        );
    const before = snapshot();

    await assert.rejects(
        native.screenshot(destination, { point: { selector: "marker", label: "start" } }),
    );

    assert.deepEqual(snapshot(), before);
});

test("the native renderer replays the session into a native video", async (t) => {
    const directory = root(t);
    const { client, session } = await completed(directory);
    const out = path.join(directory, "native.webm");
    const edit = new Edit().clip(select.marker("start"), select.marker("end"));

    const render = await client.videoRender(session, edit, out, { renderer: "native", fps: 24 });

    assert.equal(render.artifact.provenance.source_kind, "native_replay");
    assert.equal(render.plan.source.kind, "native");
    const probed = JSON.parse(
        execFileSync(
            "ffprobe",
            [
                "-v",
                "error",
                "-count_frames",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,nb_read_frames",
                "-of",
                "json",
                out,
            ],
            { encoding: "utf8" },
        ),
    );
    assert.equal(probed.streams[0].codec_name, "vp9");
    assert.ok(Number(probed.streams[0].nb_read_frames) > 0);
    const capture = JSON.parse(
        fs.readFileSync(path.join(path.dirname(render.plan.source.video), "manifest.json"), "utf8"),
    );
    assert.equal(capture.capture_kind, "video");
    assert.equal(capture.guarantee_level, "native_capture_environment");
});
