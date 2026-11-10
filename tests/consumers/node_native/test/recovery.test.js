const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const crypto = require("node:crypto");
const { Termtrace, Events } = require("@zigai/termtrace");
const binary =
    process.env.TERMTRACE_BIN || path.resolve(__dirname, "../../../../target/debug/termtrace");
const join = (base, name) =>
    Buffer.concat([Buffer.from(base), Buffer.from("/"), Buffer.from(name)]);
function copyTree(source, destination) {
    fs.mkdirSync(destination, { mode: 0o700 });
    for (const entry of fs.readdirSync(source, { encoding: "buffer", withFileTypes: true })) {
        const from = join(source, entry.name),
            to = join(destination, entry.name);
        if (entry.isDirectory()) copyTree(from, to);
        else fs.copyFileSync(from, to);
    }
}
function hashes(root, prefix = Buffer.alloc(0)) {
    const result = {};
    for (const entry of fs.readdirSync(root, { encoding: "buffer", withFileTypes: true })) {
        const relative = Buffer.concat([prefix, entry.name]);
        const member = join(root, entry.name);
        if (entry.isDirectory())
            Object.assign(result, hashes(member, Buffer.concat([relative, Buffer.from("/")])));
        else
            result[relative.toString("hex")] = crypto
                .createHash("sha256")
                .update(fs.readFileSync(member))
                .digest("hex");
    }
    return result;
}
function assertPath(value, expected) {
    const bytes =
        typeof value === "string" ? Buffer.from(value) : Buffer.from(value.base64, "base64");
    assert.deepEqual(bytes, expected);
}

for (const [sourceSuffix, destinationSuffix] of [
    [Buffer.from("source-é"), Buffer.from("dest-λ")],
    [Buffer.from([115, 255]), Buffer.from("dest")],
    [Buffer.from("source"), Buffer.from([100, 254])],
    [Buffer.from([115, 255]), Buffer.from([100, 254])],
]) {
    test(`native recovery binds exact paths ${sourceSuffix.toString("hex")}/${destinationSuffix.toString("hex")} and publishes atomically`, async () => {
        const root = fs.mkdtempSync(
            path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), "node-recovery-"),
        );
        const client = new Termtrace(binary, root);
        const recording = await client.launch(["/bin/sh", "-c", "printf RECOVERY; read line"]);
        try {
            await recording.output().waitFor(Buffer.from("RECOVERY"), 5000);
            await recording.stop();
        } finally {
            if (recording.isActive()) await recording.stop();
        }
        const source = join(root, sourceSuffix),
            destination = join(root, destinationSuffix);
        copyTree(Buffer.from(recording.path), source);
        fs.appendFileSync(join(source, "events.jsonl"), '{"torn":');
        const before = hashes(source);
        const report = await client.evidence(source).recover(destination);
        assert.deepEqual(Buffer.from(report.source), join(source, "events.jsonl"));
        assert.deepEqual(Buffer.from(report.recovered), join(destination, "events.jsonl"));
        assert.equal(typeof report.valid_events, "bigint");
        assert.ok(report.valid_events > 0n);
        assert.ok(report.discarded_tail);
        const after = hashes(source);
        assert.deepEqual(before, after);
        const diagnostic = JSON.parse(fs.readFileSync(join(destination, "recovery.json"), "utf8"));
        assert.equal(diagnostic.complete_evidence, false);
        assertPath(diagnostic.source_session, source);
        assertPath(diagnostic.recovery.source, join(source, "events.jsonl"));
        assertPath(diagnostic.recovery.recovered, join(destination, "events.jsonl"));
        assert.equal(fs.existsSync(join(destination, "completion.json")), false);
        const events = await new Events(destination).page(null, 1000);
        assert.equal(BigInt(events.events.length), report.valid_events);
        assert.equal(events.has_more, false);
        assert.equal(events.incomplete_tail, false);
        const published = hashes(destination);
        await assert.rejects(client.evidence(source).recover(destination));
        assert.deepEqual(published, hashes(destination));
        await assert.rejects(client.evidence(source).recover(source));
        assert.deepEqual(before, hashes(source));
        await assert.rejects(client.evidence(source).recover(join(source, "nested-recovery")));
        const alias = join(root, "source-link");
        fs.symlinkSync(source, alias);
        await assert.rejects(client.evidence(source).recover(join(alias, "nested-recovery")));
        assert.equal(fs.existsSync(join(source, "nested-recovery")), false);
        assert.deepEqual(before, hashes(source));
        const log = await client
            .evidence(join(source, "events.jsonl"))
            .recoverLog(join(root, Buffer.from([112, 253])));
        assert.deepEqual(Buffer.from(log.source), join(source, "events.jsonl"));
        assert.equal(log.valid_events, report.valid_events);
        fs.writeFileSync(join(source, "events.jsonl"), "invalid header\n");
        const broken = hashes(source),
            absent = join(root, Buffer.from([97, 252]));
        await assert.rejects(client.evidence(source).recover(absent));
        assert.equal(fs.existsSync(absent), false);
        assert.deepEqual(broken, hashes(source));
        assert.equal(
            fs.readdirSync(root).some((name) => name.startsWith(".termtrace-recovery-")),
            false,
        );
        fs.writeFileSync(
            path.join(root, "recovery-observed.json"),
            JSON.stringify(
                {
                    source_hex: source.toString("hex"),
                    destination_hex: destination.toString("hex"),
                    source_hashes_before: before,
                    source_hashes_after: after,
                    valid_events: report.valid_events.toString(),
                    existing_destination_unchanged: true,
                    failed_destination_absent: true,
                    broken_source_hashes: broken,
                },
                null,
                2,
            ),
        );
    });
}
