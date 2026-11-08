// Scenario suites run through the Node facade against the real recorder.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { Termtrace, scenarioCiFailed, writeScenarioJunit } = require("@zigai/termtrace");

const binary = process.env.TERMTRACE_BIN
    ? path.resolve(process.env.TERMTRACE_BIN)
    : path.resolve(__dirname, "../../../../target/debug/termtrace");

function root(t) {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), "node-scenarios-"));
    t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
    return directory;
}

function writeScenario(file, name, script, steps, { env = {}, matrix, timeoutMs = 5000 } = {}) {
    const document = {
        version: 2,
        name,
        launch: {
            command: ["/bin/sh", "-c", script],
            geometry: { cols: 40, rows: 4 },
            environment: { base: "clear", copy: ["PATH"], set: env },
            timeoutMs,
        },
        steps,
    };
    if (matrix) document.matrix = matrix;
    fs.writeFileSync(file, JSON.stringify(document));
    return file;
}

function readyScenario(file, name, delay = "0") {
    return writeScenario(file, name, `sleep ${delay}; printf READY; read line`, [
        { waitScreen: { contains: "READY" } },
        { stop: {} },
    ]);
}

function matrixScenario(directory) {
    const created = path.join(directory, "created");
    fs.mkdirSync(created);
    const document = writeScenario(
        path.join(directory, "matrix.json"),
        "matrix",
        'printf x > "$CREATED/{{size}}-{{theme}}"; printf READY; read line',
        [{ waitScreen: { contains: "READY" } }, { stop: {} }],
        {
            env: { CREATED: created },
            matrix: { size: ["small", "large"], theme: ["dark", "light"] },
        },
    );
    return { document, created };
}

function client(directory) {
    return new Termtrace(binary, path.join(directory, "sessions"));
}

function attemptTotal(report) {
    return report.cases.flatMap((c) => c.attempts).reduce((sum, a) => sum + a.duration_ms, 0n);
}

// Minimal parser for the attribute-and-text XML subset JUnit uses here.
function parseXml(text) {
    const decode = (value) =>
        value.replace(
            /&(lt|gt|quot|apos|amp|#\d+);/g,
            (_, e) =>
                ({ lt: "<", gt: ">", quot: '"', apos: "'", amp: "&" })[e] ??
                String.fromCodePoint(Number(e.slice(1))),
        );
    const rootNode = { tag: null, attributes: {}, children: [], text: "" };
    const stack = [rootNode];
    const token = /<\?[^>]*\?>|<(\/?)([\w-]+)((?:\s+[\w-]+="[^"]*")*)\s*(\/?)>|([^<]+)/gy;
    let match;
    let consumed = 0;
    while ((match = token.exec(text)) !== null) {
        consumed = token.lastIndex;
        const [whole, closing, tag, attributes, selfClosing, body] = match;
        if (whole.startsWith("<?")) continue;
        const top = stack[stack.length - 1];
        if (body !== undefined) {
            top.text += decode(body);
            continue;
        }
        if (closing) {
            assert.equal(top.tag, tag, "balanced XML");
            stack.pop();
            continue;
        }
        const node = { tag, attributes: {}, children: [], text: "" };
        for (const [, key, value] of attributes.matchAll(/([\w-]+)="([^"]*)"/g))
            node.attributes[key] = decode(value);
        top.children.push(node);
        if (!selfClosing) stack.push(node);
    }
    assert.equal(consumed, text.length, "whole document parsed");
    assert.equal(stack.length, 1, "every element closed");
    assert.equal(rootNode.children.length, 1);
    return rootNode.children[0];
}

test("parallel jobs overlap cases and report every case", async (t) => {
    const directory = root(t);
    const scenarios = [0, 1, 2].map((i) =>
        readyScenario(path.join(directory, `slow-${i}.json`), `slow ${i}`, "1"),
    );
    const tt = client(directory);

    const parallel = await tt.runScenarios(scenarios, {
        failureRoot: path.join(directory, "parallel/failures"),
        jobs: 3,
    });
    assert.equal(parallel.jobs, 3n);
    assert.deepEqual(parallel.cases.map((c) => c.scenario).sort(), [...scenarios].sort());
    assert.ok(parallel.cases.every((c) => c.status === "passed"));
    assert.ok(
        parallel.duration_ms * 3n < attemptTotal(parallel) * 2n,
        `${parallel.duration_ms} vs ${attemptTotal(parallel)}`,
    );

    const serial = await tt.runScenarios(scenarios, {
        failureRoot: path.join(directory, "serial/failures"),
        jobs: 1,
    });
    assert.equal(serial.cases.length, 3);
    assert.ok(serial.duration_ms >= attemptTotal(serial));

    await assert.rejects(
        tt.runScenarios(scenarios, { failureRoot: path.join(directory, "none/failures"), jobs: 0 }),
        /jobs/,
    );
});

test("a matrix document expands to one labelled case per combination", async (t) => {
    const directory = root(t);
    const { document, created } = matrixScenario(directory);

    const report = await client(directory).runScenarios([document], {
        failureRoot: path.join(directory, "out/failures"),
        jobs: 4,
    });

    assert.equal(report.status, "passed");
    assert.deepEqual(fs.readdirSync(created).sort(), [
        "large-dark",
        "large-light",
        "small-dark",
        "small-light",
    ]);
    const names = report.cases.map((c) => c.attempts[0].report.name);
    const identities = report.cases.map((c) => c.scenario);
    for (const size of ["small", "large"]) {
        for (const theme of ["dark", "light"]) {
            const parts = [`size=${size}`, `theme=${theme}`];
            assert.equal(
                names.filter((name) => parts.every((part) => name.includes(part))).length,
                1,
            );
            assert.equal(
                identities.filter((identity) => parts.every((part) => identity.includes(part)))
                    .length,
                1,
            );
        }
    }
    assert.equal(new Set(report.cases.map((c) => c.attempts[0].report.canonical_session)).size, 4);
});

test("shards partition the expanded cases", async (t) => {
    const directory = root(t);
    const { document } = matrixScenario(directory);
    const tt = client(directory);
    const everything = await tt.runScenarios([document], {
        failureRoot: path.join(directory, "all/failures"),
        jobs: 4,
    });

    const shards = [];
    for (const shardIndex of [0, 1]) {
        shards.push(
            await tt.runScenarios([document], {
                failureRoot: path.join(directory, `shard-${shardIndex}/failures`),
                jobs: 2,
                shardIndex,
                shardCount: 2,
            }),
        );
    }

    const selected = shards.map((shard) => new Set(shard.cases.map((c) => c.scenario)));
    assert.deepEqual(
        shards.map((s) => [s.shard_index, s.shard_count]),
        [
            [0n, 2n],
            [1n, 2n],
        ],
    );
    assert.ok(selected.every((s) => s.size > 0));
    assert.ok([...selected[0]].every((identity) => !selected[1].has(identity)));
    assert.deepEqual(
        new Set([...selected[0], ...selected[1]]),
        new Set(everything.cases.map((c) => c.scenario)),
    );

    await assert.rejects(
        tt.runScenarios([document], {
            failureRoot: path.join(directory, "bad/failures"),
            shardIndex: 2,
            shardCount: 2,
        }),
        /shard/,
    );
    await assert.rejects(
        tt.runScenarios([document], {
            failureRoot: path.join(directory, "bad/failures"),
            shardIndex: 0,
        }),
        /shard/,
    );
});

test("a snapshot update writes a baseline that later runs compare", async (t) => {
    const directory = root(t);
    const text = path.join(directory, "screen.txt");
    fs.writeFileSync(text, "alpha beta\n");
    const document = writeScenario(
        path.join(directory, "snapshot.json"),
        "snapshot",
        'IFS= read -r line < "$TEXT"; printf "%s" "$line"; read line',
        [
            { waitScreen: { contains: "alpha" } },
            { matchScreenSnapshot: { name: "greeting", stableForMs: 10 } },
            { stop: {} },
        ],
        { env: { TEXT: text } },
    );
    const artifacts = path.join(directory, "artifacts");
    const failureRoot = path.join(artifacts, "failures");
    const tt = client(directory);

    const updated = await tt.runScenarios([document], { failureRoot, updateSnapshots: true });
    assert.equal(updated.status, "passed");
    const baselines = fs
        .readdirSync(path.join(artifacts, "snapshots"))
        .map((name) => path.join(artifacts, "snapshots", name));
    assert.equal(baselines.length, 1);
    const baseline = fs.readFileSync(baselines[0]);
    assert.ok(baseline.includes("alpha beta"));

    assert.equal((await tt.runScenarios([document], { failureRoot })).status, "passed");

    fs.writeFileSync(text, "alpha gamma\n");
    const changed = await tt.runScenarios([document], { failureRoot });
    const attempt = changed.cases[0].attempts[0].report;
    assert.equal(changed.status, "failed");
    assert.equal(attempt.failure.step_kind, "match_screen_snapshot");
    assert.ok(fs.statSync(attempt.failure_artifacts).isDirectory());
    assert.deepEqual(fs.readFileSync(baselines[0]), baseline);
});

test("reruns keep failed attempts and report recovered cases as flaky", async (t) => {
    const directory = root(t);
    const counter = path.join(directory, "attempts");
    const document = writeScenario(
        path.join(directory, "flaky.json"),
        "passes on the second attempt",
        'n=0; if test -f "$COUNTER"; then IFS= read -r n < "$COUNTER"; fi; n=$((n + 1)); ' +
            'printf "%s\\n" "$n" > "$COUNTER"; if test "$n" -eq 1; then printf BROKEN; else printf DONE; fi; read line',
        [{ waitScreen: { contains: "DONE" } }, { stop: {} }],
        { env: { COUNTER: counter }, timeoutMs: 1000 },
    );
    const tt = client(directory);

    const permissive = await tt.runScenarios([document], {
        failureRoot: path.join(directory, "a/failures"),
        reruns: 2,
    });
    const [recovered] = permissive.cases;
    assert.equal(permissive.status, "flaky");
    assert.equal(recovered.status, "flaky");
    assert.deepEqual(
        recovered.attempts.map((a) => a.report.status),
        ["failed", "passed"],
    );
    assert.notEqual(
        recovered.attempts[0].report.canonical_session,
        recovered.attempts[1].report.canonical_session,
    );
    assert.ok(fs.statSync(recovered.attempts[0].report.failure_artifacts).isDirectory());
    assert.equal(fs.readFileSync(counter, "utf8"), "2\n");
    assert.equal(scenarioCiFailed(permissive), false);

    fs.unlinkSync(counter);
    const strict = await tt.runScenarios([document], {
        failureRoot: path.join(directory, "b/failures"),
        reruns: 2,
        failOnFlaky: true,
    });
    assert.equal(strict.status, "flaky");
    assert.equal(strict.fail_on_flaky, true);
    assert.equal(scenarioCiFailed(strict), true);

    fs.unlinkSync(counter);
    const exhausted = await tt.runScenarios([document], {
        failureRoot: path.join(directory, "c/failures"),
    });
    assert.equal(exhausted.status, "failed");
    assert.equal(exhausted.cases[0].attempts.length, 1);
});

async function mixedReport(directory) {
    const passing = readyScenario(path.join(directory, "a-passing.json"), "shows ready");
    const failing = writeScenario(
        path.join(directory, "b-failing.json"),
        "never shows done",
        "printf READY; read line",
        [{ waitScreen: { contains: "DONE" } }, { stop: {} }],
        { timeoutMs: 500 },
    );
    return client(directory).runScenarios([passing, failing], {
        failureRoot: path.join(directory, "mixed/failures"),
        jobs: 2,
    });
}

test("JUnit has every case and marks failures", async (t) => {
    const directory = root(t);
    const report = await mixedReport(directory);
    const junit = path.join(directory, "junit.xml");

    await writeScenarioJunit(junit, report);

    const suite = parseXml(fs.readFileSync(junit, "utf8"));
    const cases = Object.fromEntries(
        suite.children.filter((c) => c.tag === "testcase").map((c) => [c.attributes.name, c]),
    );
    assert.equal(suite.tag, "testsuite");
    assert.deepEqual(
        [suite.attributes.tests, suite.attributes.failures, suite.attributes.errors],
        ["2", "1", "0"],
    );
    assert.deepEqual(Object.keys(cases).sort(), ["never shows done", "shows ready"]);
    assert.equal(
        cases["shows ready"].children.some((c) => c.tag === "failure"),
        false,
    );
    const failure = cases["never shows done"].children.find((c) => c.tag === "failure");
    assert.ok(failure);
    assert.match(failure.text, /wait_screen/);

    const before = fs.readFileSync(junit);
    await assert.rejects(writeScenarioJunit(junit, report));
    assert.deepEqual(fs.readFileSync(junit), before);
});

test("the CI verdict fails only when a case fails", async (t) => {
    const directory = root(t);
    const report = await mixedReport(directory);
    assert.equal(report.status, "failed");
    assert.deepEqual(
        report.cases.map((c) => c.status),
        ["passed", "failed"],
    );
    assert.equal(scenarioCiFailed(report), true);

    const passing = await client(directory).runScenarios(
        [readyScenario(path.join(directory, "only.json"), "only")],
        { failureRoot: path.join(directory, "ok/failures") },
    );
    assert.equal(passing.status, "passed");
    assert.equal(scenarioCiFailed(passing), false);

    assert.equal(scenarioCiFailed({ ...report, status: "passed" }), true);
});
