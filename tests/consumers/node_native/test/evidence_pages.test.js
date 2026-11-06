const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const { Termtrace, Events, Output } = require("@termtrace/core");
const root = path.resolve(__dirname, "../../../..");
const fixture = JSON.parse(
    fs.readFileSync(path.join(root, "fixtures/conformance/sdk/evidence-pages.json"), "utf8"),
);
const binary = process.env.TERMTRACE_BIN || path.join(root, "target/debug/termtrace");

test("cursor JSON retains exact numeric tokens through Rust", () => {
    const events = new Events("/unused");
    const text = JSON.stringify(fixture.event_cursor).replace(/"(\d+)"/g, "$1");
    const cursor = events.cursorFromJson(text);
    assert.equal(cursor.next_event_seq, 18446744073709551615n);
    assert.equal(cursor.source.inode, 9007199254740993n);
    assert.deepEqual(
        Buffer.from(cursor.source.session_path),
        Buffer.from("/tmp/nonutf8-\xff", "latin1"),
    );
    assert.deepEqual(events.cursorFromJson(events.cursorToJson(cursor)), cursor);
    for (const token of ["-1", "1.5", "18446744073709551616", '"1"']) {
        assert.throws(
            () => events.cursorFromJson(text.replaceAll("18446744073709551615", token)),
            TypeError,
        );
    }
    const output = new Output("/unused");
    const decoded = output.cursorFromJson(
        '{"events":' + text + ',"byte_offset":18446744073709551615}',
    );
    assert.equal(decoded.byte_offset, 18446744073709551615n);
    assert.deepEqual(output.cursorFromJson(output.cursorToJson(decoded)), decoded);
});

test("public native pages retain exact bytes, follow positions and completion resume", async () => {
    const evidence = fs.mkdtempSync(
        path.join(process.env.TASK05_EVIDENCE || os.tmpdir(), "node-pages-"),
    );
    const client = new Termtrace(binary, evidence);
    const recording = await client.launch(["/bin/sh", "-c", "printf 'A\\000\\377BC'; read line"]);
    try {
        const output = recording.output();
        assert.equal(
            (await output.waitFor(Buffer.from([65, 0, 255, 66, 67]), 5000)).output_end_offset,
            5n,
        );
        const first = await output.page(null, 3);
        assert.deepEqual(Buffer.from(first.bytes), Buffer.from([65, 0, 255]));
        assert.equal(first.has_more, true);
        assert.equal(typeof first.cursor.byte_offset, "bigint");
        const events = recording.events();
        const live = await events.follow();
        assert.equal(live.kind, "live");
        assert.equal(live.device, first.cursor.events.source.device);
        await recording.mark("evidence-page");
        const metadata = await events.page(null, 1, true, ["output"]);
        assert.equal(metadata.events[0].event, null);
        assert.equal(metadata.events[0].byte_payloads[0].byte_count, 5n);
        await recording.stop();
        const second = await output.page(first.cursor, 2);
        assert.deepEqual(Buffer.from(second.bytes), Buffer.from("BC"));
        assert.equal(second.has_more, false);
        const eof = await output.page(second.cursor, 2);
        assert.deepEqual(eof.cursor, second.cursor);
        assert.equal(eof.bytes.length, 0);
        assert.equal((await events.follow()).kind, "completed");
        let cursor,
            more = true;
        const all = [];
        while (more) {
            const page = await events.page(cursor, 1);
            all.push(...page.events);
            cursor = page.cursor;
            more = page.has_more;
        }
        assert.deepEqual(
            all.map((event) => event.seq),
            all.map((_, i) => BigInt(i)),
        );
        assert.ok(all.some((event) => event.event?.type === "marker"));
        const session = await client.attach(recording.path);
        for (const invalid of [-1n, 0.5, Infinity]) {
            assert.throws(() => session.readOutput(invalid), TypeError);
            assert.throws(() => session.readEvents(invalid), TypeError);
        }
        for (const limit of [0, 4194305]) await assert.rejects(output.page(null, limit));
        assert.throws(() => output.page({ ...first.cursor, byte_offset: -1n }, 2), TypeError);
        await assert.rejects(events.page(null, 1, false, ["invented"]));
        assert.throws(() => client.verifySession(recording.path, "signatur"), TypeError);
        fs.writeFileSync(
            path.join(evidence, "observed.json"),
            JSON.stringify({
                source: recording.path,
                output_hex: Buffer.concat([
                    Buffer.from(first.bytes),
                    Buffer.from(second.bytes),
                ]).toString("hex"),
                event_count: all.length,
                follow: "completed",
            }),
        );
    } finally {
        if (recording.isActive()) await recording.stop();
    }
});
