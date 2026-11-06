const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const { LocatorSpec, ScreenFrame } = require("..");

const fixtures = path.join(__dirname, "../../../fixtures/conformance/locators");
const parity = JSON.parse(fs.readFileSync(path.join(fixtures, "parity.json"), "utf8"));
// The frame is decoded natively from raw text so its u64 fields never pass through Number.
const frameJson = fs.readFileSync(path.join(fixtures, parity.frame), "utf8");

function spec(description) {
    let locator;
    if ("text" in description) locator = LocatorSpec.text(description.text);
    else if ("exact" in description) locator = LocatorSpec.exact(description.exact);
    else if ("regex" in description) locator = LocatorSpec.regex(description.regex);
    else locator = new LocatorSpec();
    if ("row" in description) locator = locator.inRow(description.row);
    if ("region" in description) locator = locator.inRegion(description.region);
    if ("style" in description) locator = locator.withStyle(description.style);
    if (description.cross_soft_wraps) locator = locator.acrossSoftWraps();
    return locator;
}

const triples = (ranges) => ranges.map((r) => [r.row, r.start_column, r.end_column]);

function checkCase(frame, testCase) {
    const { name, expect } = testCase;
    let found;
    try {
        const result = frame.query(spec(testCase.spec));
        const operation = testCase.operation;
        if (operation === undefined) found = result.all();
        else if (operation === "nth") found = [result.nth(testCase.index)];
        else found = [result[operation]()];
    } catch (error) {
        assert.ok(expect.error, `${name}: ${error.message}`);
        assert.equal(error.kind, expect.error.kind, name);
        assert.deepEqual(error.effect, { kind: "not_applied" }, name);
        for (const field of ["count", "index", "state_hash"]) {
            if (field in expect.error && !(field === "count" && expect.error.kind === "no_match"))
                assert.equal(
                    error[field],
                    field === "state_hash" ? expect.error[field] : BigInt(expect.error[field]),
                    `${name}: ${field}`,
                );
        }
        if (expect.error.ranges) assert.deepEqual(triples(error.ranges), expect.error.ranges, name);
        return;
    }
    assert.ok(expect.matches, `${name}: unexpected success`);
    assert.deepEqual(
        found.map((m) => ({
            text: m.text,
            ranges: triples(m.ranges),
            widths: m.cells.map((c) => c.width),
        })),
        expect.matches,
        name,
    );
    for (const m of found) assert.equal(m.frame.frameHash, frame.frameHash, name);
}

test("offline parity corpus matches ranges, widths, and failure kinds", () => {
    const frame = ScreenFrame.fromJson(frameJson);
    assert.ok(parity.cases.length >= 12);
    for (const testCase of parity.cases) checkCase(frame, testCase);
});

test("u64 frame facts stay exact bigints and absent cursor facts stay null", () => {
    const frame = ScreenFrame.fromJson(frameJson);
    const facts = parity.frame_facts;
    assert.equal(frame.source.observed_monotonic_ns, BigInt(facts.observed_monotonic_ns));
    assert.equal(frame.source.observed_monotonic_ns, 18446744073709551615n);
    assert.equal(frame.source.boundaries.observed.next_event_seq, 9007199254740993n);
    assert.equal(frame.source.boundaries.durable.next_event_seq, 9n);
    assert.equal(frame.cursor.row, null);
    assert.equal(frame.cursor.column, null);
    assert.equal(frame.rows, 5);
    assert.equal(frame.content.rows.length, 5);
    assert.equal(frame.stateHash, facts.state_hash);
});

test("host regexes and malformed inputs are rejected with argument errors", () => {
    assert.throws(() => LocatorSpec.regex(/Save/), TypeError);
    for (const pattern of ["x*", "(", ""]) {
        assert.throws(
            () => LocatorSpec.regex(pattern),
            (error) => error.kind === "invalid_query",
        );
    }
    assert.throws(() => new LocatorSpec().withStyle({ bold: null }), TypeError);
    assert.throws(() => new LocatorSpec().inRegion({ top: 0, left: 0, right: 1 }), TypeError);
    for (const row of [-1, 1.5, Number.NaN, 65536]) {
        assert.throws(() => new LocatorSpec().inRow(row), RangeError);
    }
    assert.throws(() => ScreenFrame.fromJson("{}"), TypeError);
    const frame = ScreenFrame.fromJson(frameJson);
    assert.throws(() => frame.query(LocatorSpec.text("Save")).nth(-1), RangeError);
});
