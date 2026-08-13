import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "bun:test";
import { Duration } from "../src/actions.js";
import {
    assertMatchStyle,
    assertMatches,
    assertNeutralRegion,
    assertNoMatches,
    assertNoWrappedRows,
    assertScreenMatch,
} from "../src/assertions.js";
import { InvalidQueryError, LocatorTimeoutError } from "../src/errors.js";
import { Locator } from "../src/locators.js";
import {
    allOf,
    evaluateQuery,
    regionQuery,
    rowQuery,
    styleQuery,
    textQuery,
    type LocatorSpec,
} from "../src/queries.js";
import { parseScreenFrame, ScreenFrame, type ScreenAppearance } from "../src/screen-frame.js";

const fixturePath = join(import.meta.dir, "../../../fixtures/locator-conformance.json");

function record(value: unknown): Readonly<Record<string, unknown>> {
    if (typeof value !== "object" || value === null || Array.isArray(value))
        throw new TypeError("fixture value must be an object");
    return Object.fromEntries(Object.entries(value));
}

function string(value: unknown): string {
    if (typeof value !== "string") throw new TypeError("fixture value must be a string");
    return value;
}

function integer(value: unknown): number {
    if (typeof value !== "number" || !Number.isSafeInteger(value))
        throw new TypeError("fixture value must be an integer");
    return value;
}

function boolean(value: unknown, fallback: boolean): boolean {
    if (value === undefined) return fallback;
    if (typeof value !== "boolean") throw new TypeError("fixture value must be a boolean");
    return value;
}

function spec(value: unknown): LocatorSpec {
    const query = record(value);
    const kind = string(query.kind);
    if (kind === "text") {
        return textQuery(string(query.value), {
            exact: boolean(query.exact, false),
            crossSoftWrap: boolean(query.cross_soft_wraps, false),
        });
    }
    if (kind === "regex") return textQuery(new RegExp(string(query.pattern)));
    if (kind === "style") {
        const bold = query.bold;
        const resolvedBackground = query.resolved_background;
        if (bold !== undefined && typeof bold !== "boolean")
            throw new TypeError("fixture bold value must be a boolean");
        if (resolvedBackground !== undefined && typeof resolvedBackground !== "string")
            throw new TypeError("fixture background must be a string");
        return styleQuery({
            ...(bold === undefined ? {} : { bold }),
            ...(resolvedBackground === undefined
                ? {}
                : { resolvedBackground: resolvedBackground as `#${string}` }),
        });
    }
    if (kind === "row") return rowQuery(integer(query.row));
    if (kind === "region") {
        return regionQuery({
            top: integer(query.top),
            left: integer(query.left),
            width: integer(query.width),
            height: integer(query.height),
        });
    }
    if (kind === "all") {
        if (!Array.isArray(query.specs)) throw new TypeError("fixture specs must be an array");
        return allOf(...query.specs.map(spec));
    }
    throw new TypeError(`unknown fixture query kind ${kind}`);
}

function normalizedMatches(
    frame: ScreenFrame,
    query: unknown,
): ReadonlyArray<Readonly<Record<string, unknown>>> {
    return evaluateQuery(frame, spec(query)).map((match) => {
        expect(match.frame).toBe(frame);
        expect(match.stateHash).toBe("fixture-state-v1");
        expect(match.frameHash).toBe("fixture-frame-v1");
        expect(match.frame.presented).toBe(false);
        expect(match.query.length).toBeLessThanOrEqual(240);
        return {
            text: match.text,
            ranges: match.ranges.map((range) => [range.row, range.startColumn, range.endColumn]),
        };
    });
}
function fixtureFrame(): ScreenFrame {
    const payload: unknown = JSON.parse(readFileSync(fixturePath, "utf8"));
    return parseScreenFrame(record(payload).frame);
}

function textOnlyFixtureFrame(): ScreenFrame {
    const payload: unknown = JSON.parse(readFileSync(fixturePath, "utf8"));
    const frame = record(record(payload).frame);
    const data = record(frame.data);
    if (!Array.isArray(data.rows)) throw new TypeError("fixture rows must be an array");
    return parseScreenFrame({
        ...frame,
        data: {
            detail: "text",
            rows: data.rows.map((value) => {
                const row = record(value);
                return {
                    row: row.row,
                    soft_wrapped: row.soft_wrapped,
                    wrap_continuation: row.wrap_continuation,
                    text: row.text,
                };
            }),
        },
    });
}

function literalFrame(text: string): ScreenFrame {
    const source = fixtureFrame();
    const appearance: ScreenAppearance | undefined = source.rows[0]?.spans[0]?.appearance;
    if (appearance === undefined) throw new TypeError("fixture must contain a styled span");
    const cells = [...text].map((grapheme) => ({ grapheme, wideKind: "narrow" as const }));
    const region = { top: 0, left: 0, width: cells.length, height: 1 };
    return new ScreenFrame(
        source.version,
        source.stateHash,
        source.frameHash,
        source.presented,
        source.captureQuality,
        source.source,
        { cols: cells.length, rows: 1 },
        region,
        region,
        "spans",
        source.cursor,
        source.modes,
        source.colors,
        [
            {
                row: 0,
                softWrapped: false,
                wrapContinuation: false,
                text,
                spans: [
                    {
                        startColumn: 0,
                        endColumn: cells.length,
                        cellCount: cells.length,
                        text,
                        appearance,
                        cells,
                    },
                ],
                cells: null,
            },
        ],
    );
}

test("shared locator conformance corpus matches public contract", () => {
    const payload: unknown = JSON.parse(readFileSync(fixturePath, "utf8"));
    const corpus = record(payload);
    expect(corpus.version).toBe(1);
    const frame = parseScreenFrame(corpus.frame);
    expect(frame.presented).toBe(false);
    expect(frame.captureQuality).toBe("incomplete");
    if (!Array.isArray(corpus.cases)) throw new TypeError("fixture cases must be an array");

    for (const value of corpus.cases) {
        const testCase = record(value);
        if (testCase.error === "invalid_query") {
            expect(() => normalizedMatches(frame, testCase.query)).toThrow(InvalidQueryError);
            continue;
        }
        expect(testCase.error).toBeUndefined();
        if (!Array.isArray(testCase.matches))
            throw new TypeError("fixture matches must be an array");
        expect(normalizedMatches(frame, testCase.query), string(testCase.name)).toEqual(
            testCase.matches,
        );
    }
});

test("runner-neutral assertions are strict and evidence-bound", () => {
    const payload: unknown = JSON.parse(readFileSync(fixturePath, "utf8"));
    const frame = parseScreenFrame(record(payload).frame);
    const error = assertScreenMatch(frame, textQuery("ERROR"));
    assertMatchStyle(error, { bold: true });
    expect(() => assertMatchStyle(error, { bold: false })).toThrow();
    expect(() => assertScreenMatch(frame, textQuery("Ready"))).toThrow();
    expect(assertMatches(frame, textQuery("Ready"), 2)).toHaveLength(2);
    assertNoMatches(frame, textQuery("missing"));
    expect(() => assertNoWrappedRows(frame)).toThrow();
    assertNeutralRegion(frame, { top: 0, left: 11, width: 1, height: 1 });
    expect(() => assertNeutralRegion(frame, { top: 2, left: 0, width: 12, height: 1 })).toThrow();
});

test("style filters reject unknown and malformed runtime fields", () => {
    const invalid: unknown[] = [
        { bld: true },
        { bold: undefined },
        { bold: true, bld: true },
        { bold: "yes" },
        { underline: 1.5 },
        { wideKind: "wide_tail" },
        { foreground: { kind: "none", value: 1 } },
        { foreground: { kind: "palette", value: 256 } },
        { foreground: { kind: "rgb", value: { r: 0, g: 0, b: 300 } } },
        { underlineColor: "#ffffff" },
        { resolvedForeground: { r: 0, g: 0 } },
        { resolvedBackground: { r: 0, g: 0, b: 0, alpha: 1 } },
    ];
    for (const filter of invalid)
        expect(() => styleQuery(filter as never)).toThrow(InvalidQueryError);

    expect(() =>
        styleQuery({
            foreground: { kind: "palette", value: 2 },
            background: "#010203",
            underlineColor: { kind: "rgb", value: { r: 4, g: 5, b: 6 } },
            resolvedForeground: null,
            resolvedBackground: { r: 7, g: 8, b: 9 },
        }),
    ).not.toThrow();
});

test("exact text is evaluated within the composed region", () => {
    const frame = fixtureFrame();
    const matches = evaluateQuery(
        frame,
        allOf(
            textQuery("Ready ", { exact: true }),
            regionQuery({ top: 0, left: 6, width: 6, height: 1 }),
        ),
    );
    expect(matches.map((match) => match.ranges)).toEqual([
        [{ row: 0, startColumn: 6, endColumn: 12 }],
    ]);
});

test("queries reject text-only frames instead of reporting false absence", () => {
    const frame = textOnlyFixtureFrame();
    const query = textQuery("Ready");
    expect(() => evaluateQuery(frame, query)).toThrow(InvalidQueryError);
    expect(() => frame.query(query)).toThrow(InvalidQueryError);
    expect(() => assertNoMatches(frame, query)).toThrow(InvalidQueryError);
});

test("literal matching preserves overlapping ranges", () => {
    const matches = evaluateQuery(literalFrame("aaa"), textQuery("aa"));
    expect(matches.map((match) => match.ranges)).toEqual([
        [{ row: 0, startColumn: 0, endColumn: 2 }],
        [{ row: 0, startColumn: 1, endColumn: 3 }],
    ]);
});

test("composed text and style queries are independent of clause order", () => {
    const frame = fixtureFrame();
    const textFirst = evaluateQuery(frame, allOf(textQuery("ERROR"), styleQuery({ bold: true })));
    const styleFirst = evaluateQuery(frame, allOf(styleQuery({ bold: true }), textQuery("ERROR")));
    expect(styleFirst.map((match) => ({ text: match.text, ranges: match.ranges }))).toEqual(
        textFirst.map((match) => ({ text: match.text, ranges: match.ranges })),
    );
    expect(styleFirst.map((match) => match.ranges)).toEqual([
        [{ row: 1, startColumn: 6, endColumn: 11 }],
    ]);
});

test("composed queries reject conflicting scopes with a primary matcher", () => {
    const frame = fixtureFrame();
    expect(() => evaluateQuery(frame, allOf(textQuery("Ready"), rowQuery(0), rowQuery(1)))).toThrow(
        InvalidQueryError,
    );
    expect(() =>
        evaluateQuery(
            frame,
            allOf(
                styleQuery({ bold: true }),
                regionQuery({ top: 0, left: 0, width: 1, height: 1 }),
                regionQuery({ top: 1, left: 0, width: 1, height: 1 }),
            ),
        ),
    ).toThrow(InvalidQueryError);
});

test("locator wait observes timeout while capture is in flight", async () => {
    let captureSignal: AbortSignal | undefined;
    const locator = new Locator(async (request, options) => {
        expect(request.detail).toBe("spans");
        captureSignal = options?.signal;
        return await new Promise<ScreenFrame>(() => {});
    }, textQuery("Ready"));
    await expect(
        locator.wait({
            timeout: Duration.milliseconds(10),
            interval: Duration.milliseconds(1),
        }),
    ).rejects.toBeInstanceOf(LocatorTimeoutError);
    expect(captureSignal?.aborted).toBe(true);
});

test("locator wait observes abort while capture is in flight", async () => {
    let captureStarted: (() => void) | undefined;
    let captureSignal: AbortSignal | undefined;
    const started = new Promise<void>((resolve) => {
        captureStarted = resolve;
    });
    const locator = new Locator(async (request, options) => {
        expect(request.detail).toBe("spans");
        captureSignal = options?.signal;
        captureStarted?.();
        return await new Promise<ScreenFrame>(() => {});
    }, textQuery("Ready"));
    const controller = new AbortController();
    const reason = new Error("cancel locator");
    const waiting = locator.wait({
        timeout: Duration.seconds(1),
        interval: Duration.milliseconds(1),
        signal: controller.signal,
    });
    await started;
    controller.abort(reason);
    await expect(waiting).rejects.toBe(reason);
    expect(captureSignal?.aborted).toBe(true);
});
