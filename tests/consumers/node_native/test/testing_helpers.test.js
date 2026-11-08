const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const { ScreenFrame, LocatorSpec, Termtrace } = require("@zigai/termtrace");
const t = require("@zigai/termtrace/testing");
const repo = path.resolve(__dirname, "../../../..");
const binary = process.env.TERMTRACE_BIN ?? path.join(repo, "target/debug/termtrace");
const source = fs.readFileSync(
    path.join(repo, "fixtures/conformance/locators/parity-frame.json"),
    "utf8",
);
const corpus = JSON.parse(
    fs.readFileSync(path.join(repo, "fixtures/conformance/testing/assertions.json"), "utf8"),
);
const command = [
    "/bin/sh",
    "-c",
    'trap "exit 0" TERM; printf READY; while IFS= read -r line; do printf "INPUT:<%s>" "$line"; done',
];

test("all immutable assertion families and normalized/masked bytes match the shared Rust corpus", () => {
    const f = ScreenFrame.fromJson(source),
        found = f.query(LocatorSpec.text("Save"));
    const none = f.query(LocatorSpec.text("ABSENT")),
        selected = found.first();
    for (const [profile, expected] of Object.entries(corpus.normalized)) {
        assert.equal(
            t.normalizedSnapshot(f, { profile, includeCursor: true, includeModes: true }),
            expected,
        );
    }
    assert.equal(t.maskedSnapshot(f, corpus.mask_options), corpus.masked);
    t.assertScreenText(f, "Save");
    t.assertExists(found);
    t.assertNotExists(none);
    t.assertCount(found, 2);
    t.assertMatchText(selected, "Save");
    t.assertMatchStyle(selected, {});
    t.assertCursor(f, f.cursor);
    t.assertNoPresentedTransient(f);
    const bad = source.replace('"presented": true', '"presented": false');
    const calls = {
        screen_text: () => t.assertScreenText(f, "ABSENT"),
        exists: () => t.assertExists(none),
        not_exists: () => t.assertNotExists(found),
        count: () => t.assertCount(found, 1),
        match_text: () => t.assertMatchText(selected, "OTHER"),
        match_style: () => t.assertMatchStyle(selected, { bold: true }),
        cursor: () => t.assertCursor(f, { ...f.cursor, row: 0 }),
        no_wrapped_rows: () => t.assertNoWrappedRows(f),
        neutral_region: () => t.assertNeutralRegion(f, { top: 0, left: 0, right: 1, bottom: 1 }),
        no_presented_transient: () => t.assertNoPresentedTransient(ScreenFrame.fromJson(bad)),
    };
    for (const [name, call] of Object.entries(calls)) {
        assert.throws(call, (error) => {
            assert.ok(error instanceof t.ExpectationMismatchError);
            const fields = Object.fromEntries(
                Object.keys(corpus.mismatches[name]).map((key) => [key, error[key]]),
            );
            assert.deepEqual(fields, corpus.mismatches[name]);
            return true;
        });
        t.assertNot(call);
    }
    for (const operation of [
        () => f.query(LocatorSpec.regex("[")),
        () => found.single(),
        () => none.first(),
    ]) {
        assert.throws(
            () => t.assertNot(operation),
            (e) => !(e instanceof t.ExpectationMismatchError),
        );
    }
    assert.equal(f.toJson(), ScreenFrame.fromJson(source).toJson());
});

test("snapshots never update implicitly; invalid masks and filesystem errors cannot pass negation", () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "task04-snapshots-"));
    try {
        const frame = ScreenFrame.fromJson(source),
            file = path.join(root, "baseline.json");
        assert.throws(() => t.assertSnapshotFile(frame, file), t.ExpectationMismatchError);
        t.assertSnapshotFile(frame, file, { update: true });
        t.assertSnapshotFile(frame, file);
        const original = fs.readFileSync(file, "utf8");
        assert.equal(original, t.normalizedSnapshot(frame) + "\n");
        const link = path.join(root, "link");
        fs.symlinkSync(file, link);
        for (const update of [false, true]) {
            assert.throws(
                () => t.assertNot(() => t.assertSnapshotFile(frame, link, { update })),
                (e) => !(e instanceof t.ExpectationMismatchError),
            );
        }
        const invalid = { mask_regions: [{ top: 1, left: 0, right: 1, bottom: 2 }] };
        assert.throws(
            () => t.assertMaskedSnapshotFile(frame, file, invalid, true),
            (e) => e.kind === "invalid_query",
        );
        assert.equal(fs.readFileSync(file, "utf8"), original);
        const masked = path.join(root, "masked.json");
        t.assertMaskedSnapshotFile(frame, masked, corpus.mask_options, true);
        t.assertMaskedSnapshotFile(frame, masked, corpus.mask_options);
        const none = source.slice(0, source.indexOf('"data":')) + '"data":{"detail":"none"}}';
        assert.throws(
            () => t.assertNot(() => t.assertScreenText(ScreenFrame.fromJson(none), "Save")),
            (e) => e.kind === "insufficient_frame_detail",
        );
    } finally {
        fs.rmSync(root, { recursive: true, force: true });
    }
});

for (const policy of ["on_failure", "always", "manual"]) {
    test(`real action, presentation, assertion and ${policy} retention with exactly-once report`, async () => {
        let root, rec, report;
        await t.withTermtrace(
            async (ctx) => {
                root = ctx.evidenceRoot;
                rec = await ctx.record(command);
                await rec.screen().waitForText("READY", 5000);
                const result = await rec
                    .actions()
                    .sendText("go\n")
                    .untilScreen("INPUT:<go>", 5000)
                    .run();
                t.assertScreenText(result.frame, "INPUT:<go>");
                report = await ctx.finalize();
                assert.equal(await ctx.finalize("error"), report);
                assert.equal(ctx.report, report);
                await assert.rejects(ctx.record(command), /finalized/);
            },
            { binary, evidencePolicy: policy },
        );
        assert.deepEqual(report.secondary_failures, []);
        assert.equal(report.retained, policy !== "on_failure");
        assert.equal(fs.existsSync(root), report.retained);
        assert.equal(rec.isActive(), false);
        if (report.retained) {
            const verification = await new Termtrace(binary).verifySession(rec.path, "content");
            assert.ok(verification.valid);
            if (process.env.TASK04_EVIDENCE) {
                fs.mkdirSync(process.env.TASK04_EVIDENCE, { recursive: true });
                fs.writeFileSync(
                    path.join(process.env.TASK04_EVIDENCE, `node-${policy}.json`),
                    JSON.stringify({ report, verification }),
                );
            }
        }
    });
}

for (const [name, failure] of [
    ["error", new Error("PRIMARY")],
    ["frozen", Object.freeze(new Error("PRIMARY"))],
    ["primitive", "PRIMARY"],
]) {
    test(`real retained evidence preserves ${name} identity`, async () => {
        let rec, root;
        try {
            await t.withTermtrace(
                async (ctx) => {
                    root = ctx.evidenceRoot;
                    rec = await ctx.record(command);
                    await rec.screen().waitForText("READY", 5000);
                    t.assertScreenText(await rec.screen().frame(), "READY");
                    throw failure;
                },
                { binary },
            );
            assert.fail("must fail");
        } catch (error) {
            assert.equal(error, failure);
        }
        assert.ok(fs.existsSync(path.join(rec.path, "completion.json")));
        assert.equal(rec.isActive(), false);
        const verification = await new Termtrace(binary).verifySession(rec.path, "content");
        assert.ok(verification.valid);
        assert.ok(fs.existsSync(root));
    });
}

test("cleanup failure fails a passing body, retains paths and continues siblings", async () => {
    let second;
    await assert.rejects(
        t.withTermtrace(
            async (ctx) => {
                const first = await ctx.record(command);
                second = await ctx.record(command);
                await first.screen().waitForText("READY", 5000);
                await second.screen().waitForText("READY", 5000);
                first.forceClose();
            },
            { binary, finalizationTimeoutMs: 100 },
        ),
        (error) => {
            assert.ok(error instanceof AggregateError);
            assert.ok(error.termtrace.report.secondary_failures.length > 0);
            assert.ok(error.termtrace.report.retained);
            for (const file of error.termtrace.report.verified_artifacts)
                assert.ok(fs.existsSync(file));
            return true;
        },
    );
    assert.equal(second.isActive(), false);
    assert.ok(fs.existsSync(path.join(second.path, "completion.json")));
});

test("a late unawaited launch is owned until teardown", async () => {
    let worker;
    await t.withTermtrace(
        async (ctx) => {
            worker = ctx.record(command);
        },
        { binary },
    );
    const rec = await worker;
    assert.equal(rec.isActive(), false);
    assert.equal(fs.existsSync(rec.path), false);
});

test("async negation propagates operational rejections and matches only typed mismatches", async () => {
    const frame = ScreenFrame.fromJson(source);
    await t.assertNot(async () => t.assertScreenText(frame, "ABSENT"));
    const operational = new Error("ORIGINAL_OPERATIONAL");
    await assert.rejects(
        t.assertNot(async () => {
            throw operational;
        }),
        (e) => e === operational,
    );
    await assert.rejects(
        t.assertNot(async () => t.assertScreenText(frame, "Save")),
        t.ExpectationMismatchError,
    );
});

test("invalid explicit outcome cannot prevent automatic finalization", async () => {
    let rec;
    await assert.rejects(
        t.withTermtrace(
            async (ctx) => {
                rec = await ctx.record(command);
                await rec.screen().waitForText("READY", 5000);
                ctx.finalize("invalid");
            },
            { binary },
        ),
        /invalid finalization outcome/,
    );
    assert.equal(rec.isActive(), false);
    assert.ok(fs.existsSync(path.join(rec.path, "completion.json")));
});

test("native finalize transport also returns its first report on duplicate calls", async () => {
    const { finalizeRecordings } = require("@zigai/termtrace");
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "task04-native-finalize-"));
    const rec = await new Termtrace(binary, root).record(command);
    await rec.screen().waitForText("READY", 5000);
    const first = await finalizeRecordings([rec], root, "passed", "manual", 1000);
    const again = await finalizeRecordings([rec], root, "error", "manual", 1000);
    assert.deepEqual(again, first);
    assert.equal(
        fs.readdirSync(root).filter((name) => name.startsWith(".test-artifacts-")).length,
        1,
    );
    await assert.rejects(
        t.assertNot(async () => t.assertScreenText(ScreenFrame.fromJson(source), "Save")),
        (e) =>
            e instanceof t.ExpectationMismatchError &&
            e.kind === "expectation_mismatch" &&
            e.assertion === "not" &&
            e.frame === null,
    );
});

test("partial launch failure keeps its original error and finalizes prior sessions", async () => {
    let rec, original;
    await assert.rejects(
        t.withTermtrace(
            async (ctx) => {
                rec = await ctx.record(command);
                await rec.screen().waitForText("READY", 5000);
                try {
                    await ctx.record(
                        command,
                        undefined,
                        undefined,
                        undefined,
                        "/termtrace-task04-missing-cwd",
                    );
                } catch (error) {
                    original = error;
                    throw error;
                }
            },
            { binary },
        ),
        (error) => error === original && error.termtrace.report.retained,
    );
    assert.equal(rec.isActive(), false);
    assert.ok(fs.existsSync(path.join(rec.path, "completion.json")));
});

test("partial deletion reports surviving paths, never deleted canonical evidence", async () => {
    assert.notEqual(
        process.getuid(),
        0,
        "permission failure conformance requires an unprivileged verifier",
    );
    let rec, locked, root;
    try {
        await assert.rejects(
            t.withTermtrace(
                async (ctx) => {
                    root = ctx.evidenceRoot;
                    rec = await ctx.record(command);
                    await rec.screen().waitForText("READY", 5000);
                    locked = path.join(root, "locked");
                    fs.mkdirSync(locked);
                    fs.writeFileSync(path.join(locked, "entry"), "survives");
                    fs.chmodSync(locked, 0);
                },
                { binary },
            ),
            (error) => {
                assert.equal(fs.existsSync(rec.path), false);
                assert.equal(error.termtrace.report.retained, fs.existsSync(root));
                assert.equal(error.termtrace.report.evidence_directory, root);
                for (const file of error.termtrace.report.verified_artifacts)
                    assert.ok(fs.existsSync(file));
                return true;
            },
        );
    } finally {
        if (locked && fs.existsSync(locked)) fs.chmodSync(locked, 0o700);
    }
});

test("artifact preparation failure cannot leak owned sessions or stop sibling cleanup", async () => {
    let first, second;
    await assert.rejects(
        t.withTermtrace(
            async (ctx) => {
                first = await ctx.record(command);
                second = await ctx.record(command);
                await first.screen().waitForText("READY", 5000);
                await second.screen().waitForText("READY", 5000);
                fs.writeFileSync(
                    path.join(ctx.evidenceRoot, `.test-artifacts-${first.sessionId}`),
                    "collision",
                );
            },
            { binary },
        ),
        (error) =>
            error.termtrace.report.secondary_failures.some((e) => e.phase === "artifact_directory"),
    );
    for (const rec of [first, second]) {
        assert.equal(rec.isActive(), false);
        assert.ok(fs.existsSync(path.join(rec.path, "completion.json")));
    }
});

test("direct client recordings remain caller-owned and are never discarded", async () => {
    let owned, foreign, report;
    try {
        await t.withTermtrace(
            async (ctx) => {
                owned = await ctx.record(command);
                foreign = await ctx.termtrace.record(command);
                await owned.screen().waitForText("READY", 5000);
                await foreign.screen().waitForText("READY", 5000);
                report = await ctx.finalize();
            },
            { binary },
        );
        assert.equal(owned.isActive(), false);
        assert.equal(foreign.isActive(), true);
        assert.ok(fs.existsSync(path.join(foreign.path, "session.json")));
        assert.ok(report.retained);
        assert.equal(report.verified_artifacts.includes(foreign.path), false);
    } finally {
        if (foreign) await foreign.stop();
    }
});

for (const operation of ["stop", "completion"]) {
    test(`abandoned stalled ${operation} cannot hide force-close authority`, async () => {
        const failure = Object.freeze(new Error("ORIGINAL_LIFECYCLE_PRIMARY"));
        const pidRoot = fs.mkdtempSync(path.join(os.tmpdir(), "task04-lifecycle-pids-"));
        const pidCommand = [
            "/bin/sh",
            "-c",
            'printf "%s" "$PPID" > "$1"; printf "%s" $$ > "$1.child"; printf READY; while IFS= read -r line; do :; done',
            "owned-lifecycle",
        ];
        let rec, sibling, worker, ctx;
        const started = performance.now();
        try {
            await assert.rejects(
                t.withTermtrace(
                    async (context) => {
                        ctx = context;
                        rec = await ctx.record([...pidCommand, path.join(pidRoot, "actor")], 3, 20);
                        sibling = await ctx.record(
                            [...pidCommand, path.join(pidRoot, "sibling")],
                            3,
                            20,
                        );
                        await rec.screen().waitForText("READY", 5000);
                        await sibling.screen().waitForText("READY", 5000);
                        const pid = Number(fs.readFileSync(path.join(pidRoot, "actor"), "utf8"));
                        assert.ok(
                            fs.readFileSync(`/proc/${pid}/cmdline`).includes(Buffer.from(rec.path)),
                        );
                        process.kill(pid, "SIGSTOP");
                        worker =
                            operation === "stop" ? rec.stop(30000) : rec.waitForCompletion(30000);
                        worker.catch(() => {}); // preserve the original callback failure, never an unhandled rejection
                        for (let index = 0; rec.isActive() && index < 1000; index++)
                            await new Promise((resolve) => setTimeout(resolve, 1));
                        assert.equal(rec.isActive(), false, "lifecycle worker began");
                        throw failure;
                    },
                    { binary, finalizationTimeoutMs: 250 },
                ),
                (error) => error === failure,
            );
            const elapsed = performance.now() - started;
            assert.ok(elapsed < 8000, `${elapsed}ms: ${JSON.stringify(ctx.report)}`);
            await Promise.allSettled([worker]);
            assert.equal(rec.isActive(), false);
            assert.equal(sibling.isActive(), false);
            assert.ok(ctx.report.retained);
            assert.ok(fs.existsSync(path.join(sibling.path, "completion.json")));
            for (const name of ["actor", "actor.child", "sibling", "sibling.child"]) {
                const pid = Number(fs.readFileSync(path.join(pidRoot, name), "utf8"));
                assert.equal(fs.existsSync(`/proc/${pid}`), false, `${name} ${pid} reaped`);
            }
            fs.writeFileSync(
                path.join(pidRoot, "proof.json"),
                JSON.stringify({ operation, elapsed, report: ctx.report, pids_absent: true }),
            );
        } finally {
            if (rec) rec.forceClose();
            if (sibling) sibling.forceClose();
        }
    });
}

for (const [name, child, fatal] of [
    ["spawn_failed", ["/termtrace-task04-no-such-child"], true],
    ["exit7", ["/bin/sh", "-c", "exit 7"], false],
]) {
    test(`recorder completion ${name} is classified truthfully`, async () => {
        let ctx;
        const promise = t.withTermtrace(
            async (context) => {
                ctx = context;
                const rec = await ctx.record(child, 3, 20);
                const completion = await rec.waitForCompletion(5000);
                if (fatal) assert.equal(completion.failure.code, "spawn_failed");
                else {
                    assert.equal(completion.exit_code, 7);
                    assert.equal(completion.failure, null);
                }
            },
            { binary },
        );
        if (fatal) {
            await assert.rejects(
                promise,
                (error) =>
                    error instanceof AggregateError &&
                    error.termtrace.report.secondary_failures.some(
                        (f) => f.phase === "session_failure" && f.message.includes("spawn_failed"),
                    ),
            );
            assert.ok(ctx.report.retained);
        } else {
            await promise;
            assert.equal(ctx.report.retained, false);
            assert.deepEqual(ctx.report.secondary_failures, []);
        }
    });
}

test("acknowledged stop wait zero keeps Applied and restores the original handle", async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "task04-stop-effect-"));
    const rec = await new Termtrace(binary, root).record([
        "/bin/sh",
        "-c",
        "trap 'sleep .3; exit 0' TERM; printf READY; while :; do sleep .05; done",
    ]);
    try {
        await rec.screen().waitForText("READY", 5000);
        await assert.rejects(rec.waitForCompletion(0), (error) => {
            assert.equal(error.kind, "engine");
            assert.deepEqual(error.effect, { kind: "not_applied" });
            assert.equal(error.retry, "new_request");
            return true;
        });
        const requestId = require("node:crypto").randomUUID();
        await assert.rejects(rec.requestStop({ requestId, waitMs: 0 }), (error) => {
            assert.equal(error.kind, "engine");
            assert.equal(error.code, "engine");
            assert.deepEqual(error.effect, { kind: "applied" });
            assert.equal(error.retry, "do_not_retry");
            assert.equal(error.request_id, requestId);
            assert.equal(error.details, null);
            return true;
        });
        assert.equal(rec.isActive(), true, "completion failure restores the original handle");
        const completion = await rec.waitForCompletion(5000);
        assert.equal(completion.failure, null);
        assert.equal(rec.isActive(), false);
        assert.ok(fs.existsSync(path.join(rec.path, "completion.json")));
    } finally {
        if (rec.isActive()) rec.forceClose();
    }
});
