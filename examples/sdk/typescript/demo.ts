// Record the terminal showcase once with original pixels, then edit it into MP4, GIF and WebM.
// Usage: `node demo.js OUTPUT_DIRECTORY` (the directory must not exist). Requires Ghostty, Xvfb,
// dbus-run-session, FFmpeg and the JetBrains Mono font. Set TERMTRACE_BIN to the CLI to use.

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import {
    Edit,
    select,
    Termtrace,
    type ProcessSignal,
    type Session,
    type VisualProfile,
} from "@termtrace/core";

// Compiled into tests/consumers/node_native/.examples/ by tsconfig.examples.json.
const repository = path.resolve(__dirname, "../../../..");
const showcase = path.join(repository, "examples/apps/terminal_showcase.py");
const profile: VisualProfile = {
    version: 1,
    native: { font_family: "JetBrains Mono", font_size_points: "12", dpi: 96 },
};
const timeoutMs = 20_000;
const signals: Array<[ProcessSignal, string]> = [
    ["user1", "SIGUSR1"],
    ["user2", "SIGUSR2"],
    ["hangup", "SIGHUP"],
    ["interrupt", "SIGINT"],
];

function readJson(file: string): any {
    return JSON.parse(fs.readFileSync(file, "utf8"));
}

function canonicalHashes(session: string): Map<string, string> {
    const names = ["session.json", "events.jsonl", "completion.json"];
    for (const name of fs.readdirSync(path.join(session, "visual-original")).sort()) {
        names.push(path.join("visual-original", name));
    }
    const hashes = new Map<string, string>();
    for (const name of names) {
        const file = path.join(session, name);
        if (fs.statSync(file).isFile()) {
            hashes.set(name, createHash("sha256").update(fs.readFileSync(file)).digest("hex"));
        }
    }
    return hashes;
}

async function pause(): Promise<void> {
    // Presentation pacing so each step stays on screen; the waits synchronize.
    await new Promise((resolve) => setTimeout(resolve, 400));
}

async function drive(live: Session): Promise<void> {
    const screen = live.screen();
    const ready = await screen.waitForText("SHOWCASE_READY", timeoutMs);
    await live.mark("showcase-ready");
    // The showcase reads its terminal raw, so the line ends with a newline, not Enter.
    await live
        .actions()
        .ifState(ready.hash)
        .sendText("continue-from-file\n")
        .untilScreen("WAITING_FOR_SIGNALS", timeoutMs)
        .run();
    for (const [signal, name] of signals) {
        await pause();
        await live.signal(signal);
        await screen.waitForText(`SIGNAL:${name}`, timeoutMs);
    }
    await pause();
    await live.mark("signals-observed");
}

async function main(output: string): Promise<void> {
    fs.mkdirSync(output, { mode: 0o700 });
    const tt = new Termtrace(process.env.TERMTRACE_BIN, output);

    const report = await tt.preflightVisualProfile(profile, 80, 24);
    assert.ok(report.native);
    assert.equal(report.native.dpi, 96);
    assert.ok(report.native.discovered_font_families.includes("JetBrains Mono"));
    assert.deepEqual(
        [report.native.geometry.required_cols, report.native.geometry.required_rows],
        [80, 24],
    );

    const original = await tt.recordOriginal(["python3", showcase], {
        grid: { cols: 80, rows: 24 },
        canvas: { width: 1280, height: 720 },
        fps: 24,
        visual_profile: profile,
        startup_timeout_ms: 60_000n,
    });
    try {
        await drive(original.session());
    } catch (error) {
        await original.forceClose();
        throw error;
    }

    const recording = await original.finish(60_000);
    assert.deepEqual(recording.capture.provenance, { kind: "original_execution" });
    const session = original.path;
    assert.equal(
        readJson(path.join(session, "session.json")).visual_profile.profile_hash,
        report.profile_hash,
    );
    const completion = readJson(path.join(session, "completion.json"));
    const captureManifest = readJson(path.join(session, "visual-original/manifest.json"));
    assert.equal(captureManifest.capture_kind, "original_execution");
    assert.equal(captureManifest.effective_config_hash, report.native.effective_config_hash);
    assert.deepEqual(
        [captureManifest.geometry_report.cols, captureManifest.geometry_report.rows],
        [80, 24],
    );
    const originalVideoHash = captureManifest.members.find(
        (member: { path: string }) => member.path === "session.mkv",
    ).blake3;
    const before = canonicalHashes(session);

    const edit = new Edit()
        .clip(select.marker("showcase-ready"), select.marker("signals-observed"), {
            speed: { numerator: 1, denominator: 1 },
            hold_ms: 500,
            caption: "State-guarded input and terminal signals",
        })
        .captionLayout("footer");
    const renders = path.join(output, "renders");
    fs.mkdirSync(renders, { mode: 0o700 });
    const options = { renderer: "original_execution" as const, fps: 24 };
    const preview = await tt.videoPreview(session, edit, path.join(renders, "preview"), options);
    for (const extension of ["mp4", "gif", "webm"]) {
        const render = await tt.videoRender(
            session,
            edit,
            path.join(renders, `demo.${extension}`),
            options,
        );
        assert.equal(render.plan.edit_plan_hash, preview.plan.edit_plan_hash);
        if (render.artifact) {
            assert.deepEqual(render.artifact.provenance, {
                kind: "presentation_output",
                source_kind: "original_execution",
            });
            assert.equal(
                render.artifact.source.session_manifest_hash,
                completion.session_manifest_hash,
            );
        } else {
            // The MP4 sidecar names its source by path and hash; it must be the original capture.
            assert.equal(render.plan.source.kind, "native");
            assert.equal(
                render.plan.source.kind === "native" && render.plan.source.video_hash,
                originalVideoHash,
            );
        }
    }

    assert.deepEqual(canonicalHashes(session), before);
    console.log(
        JSON.stringify(
            {
                session,
                profile_hash: report.profile_hash,
                edit_plan_hash: preview.plan.edit_plan_hash,
                contact_sheet: preview.contact_sheet,
                outputs: ["mp4", "gif", "webm"].map((extension) =>
                    path.join(renders, `demo.${extension}`),
                ),
            },
            null,
            2,
        ),
    );
}

const output = process.argv[2];
if (process.argv.length !== 3 || !output) {
    console.error("usage: demo.js OUTPUT_DIRECTORY");
    process.exit(2);
}
main(output).catch((error) => {
    console.error(error);
    process.exitCode = 1;
});
