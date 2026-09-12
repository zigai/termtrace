import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Ajv } from "ajv";
import { parseJson } from "ajv/dist/runtime/parseJson.js";
import { expect, test } from "bun:test";
import { Duration } from "../src/session/actions.js";
import {
    assertMatchStyle,
    assertMatches,
    assertNeutralRegion,
    assertNoMatches,
    assertNoWrappedRows,
    assertScreenMatch,
} from "../src/testing/assertions.js";
import { InvalidQueryError, LocatorTimeoutError } from "../src/errors.js";
import { StyleExpectation } from "../src/testing/vitest/matcher-input.js";
import { Locator } from "../src/screen/locator.js";
import {
    allOf,
    evaluateQuery,
    regionQuery,
    rowQuery,
    styleQuery,
    textQuery,
    type LocatorSpec,
    type StyleFilter,
} from "../src/screen/queries.js";
import { parseScreenFrame, ScreenFrame, type ScreenAppearance } from "../src/screen/frame.js";
import type { JsonObject, JsonValue } from "../src/index.js";

const fixturePath = join(import.meta.dir, "../../../fixtures/conformance/locators/locator.json");

type MutableStyleFilter = { -readonly [Key in keyof StyleFilter]: StyleFilter[Key] };
type NormalizedMatch = {
    readonly text: string;
    readonly ranges: ReadonlyArray<readonly [number, number, number]>;
};
type FixtureQuery =
    | {
          readonly kind: "text";
          readonly value: string;
          readonly exact?: boolean;
          readonly cross_soft_wraps?: boolean;
      }
    | { readonly kind: "regex"; readonly pattern: string }
    | { readonly kind: "style"; readonly bold?: boolean; readonly resolved_background?: string }
    | { readonly kind: "row"; readonly row: number }
    | {
          readonly kind: "region";
          readonly top: number;
          readonly left: number;
          readonly width: number;
          readonly height: number;
      }
    | { readonly kind: "all"; readonly specs: FixtureQuery[] };
type FixtureFrame = JsonObject & {
    readonly data: JsonObject & { readonly rows: JsonObject[] };
};
type FixtureCase = {
    readonly name: string;
    readonly query: FixtureQuery;
    readonly matches?: NormalizedMatch[];
    readonly error?: "invalid_query";
};
type LocatorCorpus = {
    readonly version: number;
    readonly frame: FixtureFrame;
    readonly cases: FixtureCase[];
};

const querySchema = {
    oneOf: [
        {
            type: "object",
            required: ["kind", "value"],
            properties: {
                kind: { const: "text" },
                value: { type: "string" },
                exact: { type: "boolean" },
                cross_soft_wraps: { type: "boolean" },
            },
            additionalProperties: false,
        },
        {
            type: "object",
            required: ["kind", "pattern"],
            properties: { kind: { const: "regex" }, pattern: { type: "string" } },
            additionalProperties: false,
        },
        {
            type: "object",
            required: ["kind"],
            properties: {
                kind: { const: "style" },
                bold: { type: "boolean" },
                resolved_background: { type: "string" },
            },
            additionalProperties: false,
        },
        {
            type: "object",
            required: ["kind", "row"],
            properties: { kind: { const: "row" }, row: { type: "integer" } },
            additionalProperties: false,
        },
        {
            type: "object",
            required: ["kind", "top", "left", "width", "height"],
            properties: {
                kind: { const: "region" },
                top: { type: "integer" },
                left: { type: "integer" },
                width: { type: "integer" },
                height: { type: "integer" },
            },
            additionalProperties: false,
        },
        {
            type: "object",
            required: ["kind", "specs"],
            properties: {
                kind: { const: "all" },
                specs: { type: "array", items: { $ref: "#/$defs/query" } },
            },
            additionalProperties: false,
        },
    ],
};
const validateCorpus = new Ajv().compile<LocatorCorpus>({
    type: "object",
    required: ["version", "frame", "cases"],
    additionalProperties: false,
    properties: {
        version: { const: 1 },
        frame: {
            type: "object",
            required: ["data"],
            properties: {
                data: {
                    type: "object",
                    required: ["rows"],
                    properties: {
                        rows: { type: "array", items: { type: "object" } },
                    },
                    additionalProperties: true,
                },
            },
            additionalProperties: true,
        },
        cases: {
            type: "array",
            items: {
                type: "object",
                required: ["name", "query"],
                properties: {
                    name: { type: "string" },
                    query: { $ref: "#/$defs/query" },
                    matches: {
                        type: "array",
                        items: {
                            type: "object",
                            required: ["text", "ranges"],
                            additionalProperties: false,
                            properties: {
                                text: { type: "string" },
                                ranges: {
                                    type: "array",
                                    items: {
                                        type: "array",
                                        minItems: 3,
                                        maxItems: 3,
                                        items: { type: "integer" },
                                    },
                                },
                            },
                        },
                    },
                    error: { const: "invalid_query" },
                },
                additionalProperties: false,
            },
        },
    },
    $defs: { query: querySchema },
});
const fixtureJson = parseJson(readFileSync(fixturePath, "utf8"), 0);
if (!validateCorpus(fixtureJson)) throw new TypeError("locator corpus does not match its schema");
const corpus = fixtureJson;

function spec(query: FixtureQuery): LocatorSpec {
    if (query.kind === "text") {
        return textQuery(query.value, {
            exact: query.exact ?? false,
            crossSoftWrap: query.cross_soft_wraps ?? false,
        });
    }

    if (query.kind === "regex") return textQuery(new RegExp(query.pattern));

    if (query.kind === "style") {
        const filter: MutableStyleFilter = {};
        if (query.bold !== undefined) filter.bold = query.bold;

        if (query.resolved_background !== undefined)
            filter.resolvedBackground = parseHexColor(query.resolved_background);

        return styleQuery(filter);
    }

    if (query.kind === "row") return rowQuery(query.row);

    if (query.kind === "region") {
        return regionQuery({
            top: query.top,
            left: query.left,
            width: query.width,
            height: query.height,
        });
    }

    if (query.kind === "all") return allOf(...query.specs.map(spec));
    const exhaustive: never = query;
    throw new TypeError(`unknown fixture query ${String(exhaustive)}`);
}

function normalizedMatches(
    frame: ScreenFrame,
    query: FixtureQuery,
): ReadonlyArray<NormalizedMatch> {
    return evaluateQuery(frame, spec(query)).map((match) => {
        expect(match.frame).toBe(frame);
        expect(match.stateHash).toBe("fixture-state-v1");
        expect(match.frameHash).toBe("fixture-frame-v1");
        expect(match.frame.presented).toBe(false);
        expect(match.query.length).toBeLessThanOrEqual(240);

        return {
            text: match.text,
            ranges: match.ranges.map(
                (range) => [range.row, range.startColumn, range.endColumn] as const,
            ),
        };
    });
}

function parseHexColor(value: string): `#${string}` {
    if (!/^#[0-9a-fA-F]{6}$/u.test(value)) throw new TypeError("fixture color must be #RRGGBB");

    // SAFETY: the preceding anchored expression proves the complete #RRGGBB representation;
    // TypeScript cannot derive a template-literal type from a regular-expression match.
    return value as `#${string}`;
}

function fixtureFrame(): ScreenFrame {
    return parseScreenFrame(corpus.frame);
}

function textOnlyFixtureFrame(): ScreenFrame {
    const frame = corpus.frame;

    return parseScreenFrame({
        ...frame,
        data: {
            detail: "text",
            rows: frame.data.rows.map((row) => ({
                row: row.row,
                soft_wrapped: row.soft_wrapped,
                wrap_continuation: row.wrap_continuation,
                text: row.text,
            })),
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

function malformedStyleFilter(
    ...properties: ReadonlyArray<readonly [string, JsonValue | undefined]>
): StyleFilter {
    const filter: MutableStyleFilter = {};
    for (const [name, value] of properties) {
        Object.defineProperty(filter, name, { value, enumerable: true });
    }

    return filter;
}

test("shared locator conformance corpus matches public contract", () => {
    expect(corpus.version).toBe(1);
    const frame = parseScreenFrame(corpus.frame);
    expect(frame.presented).toBe(false);
    expect(frame.captureQuality).toBe("incomplete");

    for (const testCase of corpus.cases) {
        if (testCase.error === "invalid_query") {
            expect(() => normalizedMatches(frame, testCase.query)).toThrow(InvalidQueryError);

            continue;
        }

        expect(testCase.error).toBeUndefined();

        if (testCase.matches === undefined) throw new TypeError("fixture matches must be an array");
        expect(normalizedMatches(frame, testCase.query), testCase.name).toEqual(testCase.matches);
    }
});

test("runner-neutral assertions are strict and evidence-bound", () => {
    const frame = parseScreenFrame(corpus.frame);
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
    const invalid: StyleFilter[] = [
        malformedStyleFilter(["bld", true]),
        malformedStyleFilter(["bold", undefined]),
        malformedStyleFilter(["bold", true], ["bld", true]),
        malformedStyleFilter(["bold", "yes"]),
        malformedStyleFilter(["underline", 1.5]),
        malformedStyleFilter(["underline", -2_147_483_649]),
        malformedStyleFilter(["underline", 2_147_483_648]),
        malformedStyleFilter(["wideKind", "wide_tail"]),
        malformedStyleFilter(["foreground", { kind: "none", value: 1 }]),
        malformedStyleFilter(["foreground", { kind: "palette", value: 256 }]),
        malformedStyleFilter(["foreground", { kind: "rgb", value: { r: 0, g: 0, b: 300 } }]),
        malformedStyleFilter(["underlineColor", "#ffffff"]),
        malformedStyleFilter(["resolvedForeground", { r: 0, g: 0 }]),
        malformedStyleFilter(["resolvedBackground", { r: 0, g: 0, b: 0, alpha: 1 }]),
    ];
    for (const filter of invalid) {
        expect(() => styleQuery(filter)).toThrow(InvalidQueryError);
        expect(() => StyleExpectation.parse(filter)).toThrow(TypeError);
    }
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
