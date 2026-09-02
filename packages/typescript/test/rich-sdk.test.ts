import { describe, expect, test } from "bun:test";

import { Duration, encodeAction, parseActionResult } from "../src/session/actions.js";
import {
    InvalidDurationError,
    InvalidQueryError,
    InvalidRegionError,
    LocatorTimeoutError,
    MultipleMatchesError,
    NoMatchError,
    ProtocolMismatchError,
} from "../src/errors.js";
import { Locator } from "../src/screen/locator.js";
import { Screen, Session, type JsonObject } from "../src/index.js";
import { allOf, evaluateQuery, regionQuery, styleQuery, textQuery } from "../src/screen/queries.js";
import {
    expectDriverData,
    parseDriverResponse,
    type DriverDataKind,
    type DriverDataOf,
    type SessionMethodFields,
} from "../src/transport/protocol.js";
import type { DriverRequestOptions } from "../src/transport/driver-client.js";
import type { SessionDriver } from "../src/session/session.js";
import {
    ScreenFrame,
    parseScreenFrame,
    type CellStyle,
    type Region,
    type ScreenAppearance,
    type ScreenCell,
    type ScreenRow,
} from "../src/screen/frame.js";
import { normalizeFrame } from "../src/screen/snapshots.js";

const style: CellStyle = Object.freeze({
    foreground: Object.freeze({ kind: "none" }),
    background: Object.freeze({ kind: "none" }),
    underlineColor: Object.freeze({ kind: "none" }),
    bold: false,
    faint: false,
    italic: false,
    blink: false,
    inverse: false,
    invisible: false,
    strikethrough: false,
    overline: false,
    underline: 0,
});
const appearance: ScreenAppearance = Object.freeze({
    style,
    resolvedForeground: null,
    resolvedBackground: null,
    selected: false,
});

type TestCell = {
    readonly grapheme: string;
    readonly wideKind?: ScreenCell["wideKind"];
    readonly appearance?: ScreenAppearance;
};

function frameFor(rows: ReadonlyArray<ReadonlyArray<TestCell>>, stateHash = "state"): ScreenFrame {
    const screenRows: ScreenRow[] = rows.map((source, row) => {
        const cells = source.map(
            (cell, column): ScreenCell =>
                Object.freeze({
                    row,
                    column,
                    grapheme: cell.grapheme,
                    wideKind: cell.wideKind ?? "narrow",
                    appearance: cell.appearance ?? appearance,
                }),
        );
        return Object.freeze({
            row,
            softWrapped: false,
            wrapContinuation: false,
            text: source.map((cell) => cell.grapheme).join(""),
            spans: Object.freeze([]),
            cells: Object.freeze(cells),
        });
    });
    return new ScreenFrame(
        1,
        stateHash,
        `${stateHash}-frame`,
        true,
        "live",
        Object.freeze({
            source: "actor",
            boundaries: wireBoundaries,
            observed_monotonic_ns: 0,
            session_id: "00000000-0000-0000-0000-000000000001",
        }),
        Object.freeze({ cols: rows[0]?.length ?? 1, rows: rows.length }),
        Object.freeze({ top: 0, left: 0, width: rows[0]?.length ?? 1, height: rows.length }),
        Object.freeze({ top: 0, left: 0, width: rows[0]?.length ?? 1, height: rows.length }),
        "cells",
        Object.freeze({
            row: 0,
            column: 0,
            visible: true,
            inViewport: true,
            blinking: false,
            style: "block",
            wideTail: false,
        }),
        Object.freeze({
            alternateScreen: false,
            bracketedPaste: false,
            synchronizedOutput: false,
            wraparound: true,
            values: Object.freeze([]),
        }),
        Object.freeze({
            foreground: Object.freeze({ r: 255, g: 255, b: 255 }),
            background: Object.freeze({ r: 0, g: 0, b: 0 }),
            cursor: null,
            palette: Object.freeze([]),
        }),
        screenRows,
    );
}

const wireBoundaries = Object.freeze({
    durable: Object.freeze({ event_hash: "durable", next_event_seq: 4, output_bytes: 2 }),
    observed: Object.freeze({ event_hash: "observed", next_event_seq: 4, output_bytes: 2 }),
});

function wireRegion(region: Region): JsonObject {
    return {
        top: region.top,
        left: region.left,
        right: region.left + region.width,
        bottom: region.top + region.height,
    };
}

function wireFrame(
    frame: ScreenFrame,
    requestedRegion: Region = frame.requestedRegion,
    returnedRegion: Region = frame.returnedRegion,
): JsonObject {
    const rows = frame.rows.map((row) => ({
        row: row.row,
        soft_wrapped: row.softWrapped,
        wrap_continuation: row.wrapContinuation,
        text: row.text,
        cells: (row.cells ?? []).map((cell) => ({
            row: cell.row,
            column: cell.column,
            grapheme: cell.grapheme,
            wide_kind: cell.wideKind,
            appearance: {
                style: {
                    foreground: { kind: "none" },
                    background: { kind: "none" },
                    underline_color: { kind: "none" },
                    bold: cell.appearance.style.bold,
                    faint: cell.appearance.style.faint,
                    italic: cell.appearance.style.italic,
                    blink: cell.appearance.style.blink,
                    inverse: cell.appearance.style.inverse,
                    invisible: cell.appearance.style.invisible,
                    strikethrough: cell.appearance.style.strikethrough,
                    overline: cell.appearance.style.overline,
                    underline: cell.appearance.style.underline,
                },
                resolved_foreground: null,
                resolved_background: null,
                selected: cell.appearance.selected,
            },
        })),
    }));
    return {
        version: 1,
        state_hash: frame.stateHash,
        frame_hash: frame.frameHash,
        presented: frame.presented,
        capture_quality: frame.captureQuality,
        source: {
            source: "actor",
            session_id: "00000000-0000-0000-0000-000000000001",
            observed_monotonic_ns: 1,
            boundaries: wireBoundaries,
        },
        geometry: frame.geometry,
        requested_region: wireRegion(requestedRegion),
        returned_region: wireRegion(returnedRegion),
        data: { detail: "cells", rows },
        cursor: {
            row: frame.cursor.row,
            column: frame.cursor.column,
            visible: frame.cursor.visible,
            in_viewport: frame.cursor.inViewport,
            blinking: frame.cursor.blinking,
            style: frame.cursor.style,
            wide_tail: frame.cursor.wideTail,
        },
        modes: {
            alternate_screen: frame.modes.alternateScreen,
            bracketed_paste: frame.modes.bracketedPaste,
            synchronized_output: frame.modes.synchronizedOutput,
            wraparound: frame.modes.wraparound,
            values: frame.modes.values,
        },
        colors: frame.colors,
    };
}

describe("rich frame queries", () => {
    test("maps wide graphemes to the complete terminal-column range", () => {
        const frame = frameFor([
            [
                { grapheme: "界", wideKind: "wide" },
                { grapheme: "", wideKind: "spacer_tail" },
                { grapheme: "!" },
            ],
        ]);
        const match = evaluateQuery(frame, textQuery("界"))[0];
        expect(match?.ranges).toEqual([{ row: 0, startColumn: 0, endColumn: 2 }]);
    });

    test("resets global and sticky regexp state", () => {
        const frame = frameFor([[{ grapheme: "a" }, { grapheme: "a" }]]);
        const pattern = /a/gy;
        pattern.lastIndex = 1;
        expect(evaluateQuery(frame, textQuery(pattern))).toHaveLength(2);
        expect(pattern.lastIndex).toBe(1);
    });

    test("rejects empty and zero-width queries", () => {
        expect(() => textQuery("")).toThrow(InvalidQueryError);
        expect(() => textQuery(/(?:)/)).toThrow(InvalidQueryError);
    });

    test("rejects partial-grapheme matches instead of silently dropping them", () => {
        const frame = frameFor([[{ grapheme: "e\u0301" }]]);
        expect(() => evaluateQuery(frame, textQuery("\u0301"))).toThrow(InvalidQueryError);
    });

    test("evaluates styles on wide heads and supports resolved hex colors", () => {
        const boldStyle = Object.freeze({ ...style, bold: true });
        const headAppearance = Object.freeze({
            ...appearance,
            style: boldStyle,
            resolvedBackground: Object.freeze({ r: 59, g: 130, b: 246 }),
        });
        const frame = frameFor([
            [
                { grapheme: "界", wideKind: "wide", appearance: headAppearance },
                { grapheme: "", wideKind: "spacer_tail", appearance },
            ],
        ]);
        const styled = evaluateQuery(
            frame,
            allOf(textQuery("界"), styleQuery({ bold: true, background: "#3b82f6" })),
        );
        expect(styled).toHaveLength(1);
        expect(styled[0]?.ranges).toEqual([{ row: 0, startColumn: 0, endColumn: 2 }]);
    });

    test("intersects nested regions and rejects scopes outside returned evidence", () => {
        const frame = frameFor([
            [{ grapheme: "a" }, { grapheme: "b" }, { grapheme: "c" }, { grapheme: "d" }],
        ]);
        const spec = allOf(
            regionQuery({ top: 0, left: 0, width: 4, height: 1 }),
            allOf(regionQuery({ top: 0, left: 1, width: 2, height: 1 })),
        );
        expect(evaluateQuery(frame, spec)[0]?.ranges).toEqual([
            { row: 0, startColumn: 1, endColumn: 3 },
        ]);
        expect(() =>
            evaluateQuery(frame, regionQuery({ top: 0, left: 3, width: 2, height: 1 })),
        ).toThrow(InvalidRegionError);
    });
});

describe("lazy locator request accounting", () => {
    test("composition does no I/O and each terminal method captures once", async () => {
        const frame = frameFor([[{ grapheme: "o" }, { grapheme: "k" }]]);
        let requests = 0;
        const locator = new Locator(async () => {
            requests += 1;
            return frame;
        }, textQuery("ok")).row(0);
        expect(requests).toBe(0);
        expect(await locator.count()).toBe(1);
        expect(requests).toBe(1);
        expect((await locator.single()).frame).toBe(frame);
        expect(requests).toBe(2);
    });

    test("single is strict while first and nth are explicit", async () => {
        const frame = frameFor([[{ grapheme: "a" }, { grapheme: "a" }]]);
        const locator = new Locator(async () => frame, textQuery("a"));
        await expect(locator.single()).rejects.toBeInstanceOf(MultipleMatchesError);
        expect((await locator.first()).ranges[0]?.startColumn).toBe(0);
        expect((await locator.nth(1)).ranges[0]?.startColumn).toBe(1);
        await expect(locator.nth(2)).rejects.toBeInstanceOf(NoMatchError);
    });

    test("wait requires an unchanged satisfying state and honors cancellation", async () => {
        const frame = frameFor([[{ grapheme: "o" }, { grapheme: "k" }]], "stable-state");
        let requests = 0;
        const locator = new Locator(async () => {
            requests += 1;
            return frame;
        }, textQuery("ok"));
        const result = await locator.wait({
            timeout: Duration.milliseconds(100),
            interval: Duration.milliseconds(1),
            stableFor: Duration.milliseconds(3),
        });
        expect(result.frame).toBe(frame);
        expect(requests).toBeGreaterThan(1);

        const controller = new AbortController();
        controller.abort(new Error("cancelled locator wait"));
        await expect(locator.wait({ signal: controller.signal })).rejects.toThrow(
            "cancelled locator wait",
        );
    });

    test("ambiguity errors carry exact state and bounded coordinates", async () => {
        const frame = frameFor([[{ grapheme: "a" }, { grapheme: "a" }]], "ambiguous-state");
        const error = await new Locator(async () => frame, textQuery("a"))
            .single()
            .catch((cause: unknown) => cause);
        expect(error).toBeInstanceOf(MultipleMatchesError);
        if (!(error instanceof MultipleMatchesError))
            throw new Error("expected MultipleMatchesError");
        expect(error.stateHash).toBe("ambiguous-state");
        expect(error.ranges[0]?.[0]).toEqual({ row: 0, startColumn: 0, endColumn: 1 });
    });
});

describe("Screen rich-frame requests", () => {
    test("forwards historical points and requires requested stability", async () => {
        const frame = frameFor([[{ grapheme: "o" }, { grapheme: "k" }]], "stable-state");
        const wire = wireFrame(frame);
        const requests: Array<Extract<SessionMethodFields, { readonly method: "screen_frame" }>> =
            [];
        const requestOptions: Array<{ readonly signal?: AbortSignal }> = [];
        const session: ConstructorParameters<typeof Screen>[0] = {
            completed: false,
            control(): Promise<never> {
                return Promise.reject(new Error("control is outside this screen test seam"));
            },
            request<Kind extends DriverDataKind>(
                methodFields: SessionMethodFields,
                expectedKind: Kind,
                options: DriverRequestOptions = {},
            ): Promise<DriverDataOf<Kind>> {
                if (methodFields.method !== "screen_frame")
                    throw new Error("screen test received a non-frame request");
                requests.push(methodFields);
                requestOptions.push(options);
                const requestId = "00000000-0000-0000-0000-000000000012";
                const response = parseDriverResponse(
                    {
                        version: 5,
                        request_id: requestId,
                        result: {
                            status: "ok",
                            data: { kind: "screen_frame", frame: wire },
                        },
                    },
                    requestId,
                );
                if (response._tag === "Failure")
                    throw new Error("screen test response must be successful");
                return Promise.resolve(expectDriverData(response.data, expectedKind));
            },
        };
        const screen = new Screen(session);

        const controller = new AbortController();
        const selected = await screen.frame(
            {
                point: { selector: "event_seq", event_seq: 7 },
                detail: "cells",
                region: frame.requestedRegion,
            },
            { signal: controller.signal },
        );
        expect(selected.stateHash).toBe("stable-state");
        expect(requests[0]?.point).toEqual({ selector: "event_seq", event_seq: 7 });
        expect(requestOptions[0]?.signal).toBe(controller.signal);

        requests.length = 0;
        const stable = await screen.waitForText("ok", {
            timeoutMs: 100,
            stableForMs: 3,
        });
        expect(stable.stateHash).toBe("stable-state");
        expect(requests.length).toBeGreaterThan(1);
    });

    test("preserves already-aborted wait cancellation without capturing", async () => {
        let requests = 0;
        const session = {
            completed: false,
            control(): Promise<never> {
                return Promise.reject(new Error("control is outside this screen test seam"));
            },
            request(): Promise<never> {
                requests += 1;
                return Promise.reject(new Error("capture must not start"));
            },
        };
        const screen = new Screen(session);
        const controller = new AbortController();
        controller.abort(new Error("cancelled screen wait"));

        await expect(screen.waitForText("never", { signal: controller.signal })).rejects.toThrow(
            "cancelled screen wait",
        );
        expect(requests).toBe(0);
    });

    test("aborts in-flight captures on caller cancellation and locator timeout", async () => {
        let captureStarted: (() => void) | undefined;
        const captureSignals: AbortSignal[] = [];
        const started = new Promise<void>((resolve) => {
            captureStarted = resolve;
        });
        const session: ConstructorParameters<typeof Screen>[0] = {
            completed: false,
            control(): Promise<never> {
                return Promise.reject(new Error("control is outside this screen test seam"));
            },
            request<Kind extends DriverDataKind>(
                methodFields: SessionMethodFields,
                expectedKind: Kind,
                options: DriverRequestOptions = {},
            ): Promise<never> {
                expect([methodFields.method, expectedKind]).toEqual([
                    "screen_frame",
                    "screen_frame",
                ]);
                if (options.signal !== undefined) captureSignals.push(options.signal);
                captureStarted?.();
                return new Promise<never>(() => {});
            },
        };
        const screen = new Screen(session);
        const controller = new AbortController();
        const reason = new Error("cancel in-flight screen capture");
        const cancelled = screen.waitForText("never", {
            timeoutMs: 1_000,
            signal: controller.signal,
        });

        await started;
        controller.abort(reason);
        expect(await cancelled.catch((cause: unknown) => cause)).toBe(reason);
        expect(captureSignals[0]?.aborted).toBe(true);
        expect(captureSignals[0]?.reason).toBe(reason);

        expect(captureSignals).toHaveLength(1);
        await expect(screen.waitForText("never", { timeoutMs: 10 })).rejects.toBeInstanceOf(
            LocatorTimeoutError,
        );
        expect(captureSignals[1]?.aborted).toBe(true);
    });
});

describe("actions and snapshots", () => {
    test("action and keyboard wire encoding share deterministic atom IDs", () => {
        const requestId = "00000000-0000-4000-8000-000000000001";
        const first = encodeAction([{ kind: "text", text: "x" }], {
            requestId,
            pace: Duration.nanoseconds(1n),
        });
        const second = encodeAction([{ kind: "text", text: "x" }], {
            requestId,
            pace: Duration.nanoseconds(1n),
        });
        expect(first).toEqual(second);
    });

    test("uses actor pacing admission for every atom", () => {
        expect(() =>
            encodeAction(
                [
                    { kind: "text", text: "a" },
                    { kind: "text", text: "b" },
                ],
                { pace: Duration.seconds(31) },
            ),
        ).toThrow(InvalidDurationError);
    });

    test("reuses a generated action ID for the control envelope", async () => {
        const frame = frameFor([[{ grapheme: "x" }]]);
        let envelopeRequestId: string | undefined;
        const driver: SessionDriver = {
            request(methodFields, expectedKind, options) {
                expect([methodFields.method, expectedKind]).toEqual(["control", "control"]);
                if (options?.requestId === undefined)
                    throw new Error("action control request ID was not generated");
                envelopeRequestId = options.requestId;
                const response = parseDriverResponse(
                    {
                        version: 5,
                        request_id: options.requestId,
                        result: {
                            status: "ok",
                            data: {
                                kind: "control",
                                data: {
                                    kind: "action_applied",
                                    boundaries: wireBoundaries,
                                    input: {
                                        sequence_id: options.requestId,
                                        atom_count: 1,
                                        bytes_written: 1,
                                        first_intent_event_seq: 1,
                                        last_result_event_seq: 2,
                                        pace_ns: 0,
                                    },
                                    postcondition: {
                                        kind: "none",
                                        matched: true,
                                        frame: wireFrame(frame),
                                    },
                                },
                            },
                        },
                    },
                    options.requestId,
                );
                if (response._tag === "Failure")
                    throw new Error("action test response must be successful");
                return Promise.resolve(expectDriverData(response.data, expectedKind));
            },
        };
        const session = new Session(
            driver,
            {
                canonical_session: "/tmp/action.termtrace",
                completed: false,
                handle: "00000000-0000-0000-0000-000000000020",
                kind: "attached",
                session_id: "00000000-0000-0000-0000-000000000021",
            },
            false,
            "manual",
        );

        const result = await session.act([{ kind: "text", text: "x" }]);
        if (envelopeRequestId === undefined)
            throw new Error("action envelope request ID was not captured");

        expect(result.requestId).toBe(envelopeRequestId);
    });

    test("retains and validates action boundaries against the terminating frame", () => {
        const frame = frameFor([[{ grapheme: "x" }]]);
        const action = {
            kind: "action_applied",
            boundaries: wireBoundaries,
            input: {
                sequence_id: "00000000-0000-0000-0000-000000000030",
                atom_count: 1,
                bytes_written: 1,
                first_intent_event_seq: 1,
                last_result_event_seq: 2,
                pace_ns: 0,
            },
            postcondition: { kind: "none", matched: true, frame: wireFrame(frame) },
        };
        const parsed = parseActionResult(action, "00000000-0000-0000-0000-000000000030", false);
        expect(parsed.boundaries).toEqual({
            durable: { eventHash: "durable", nextEventSeq: 4, outputBytes: 2 },
            observed: { eventHash: "observed", nextEventSeq: 4, outputBytes: 2 },
        });

        expect(() =>
            parseActionResult(
                {
                    ...action,
                    boundaries: {
                        ...wireBoundaries,
                        observed: { ...wireBoundaries.observed, next_event_seq: 5 },
                    },
                },
                "00000000-0000-0000-0000-000000000030",
                false,
            ),
        ).toThrow(ProtocolMismatchError);
    });

    test("reconstructs appearance spans from cells", () => {
        const boldAppearance = Object.freeze({
            ...appearance,
            style: Object.freeze({ ...style, bold: true }),
        });
        const frame = frameFor([
            [
                { grapheme: "x", appearance: boldAppearance },
                { grapheme: "", appearance: boldAppearance },
            ],
        ]);

        const normalized = normalizeFrame(frame, "spans");

        expect(normalized.rows).toEqual([
            {
                row: 0,
                softWrapped: false,
                wrapContinuation: false,
                text: "x",
                spans: [
                    {
                        startColumn: 0,
                        endColumn: 2,
                        cellCount: 2,
                        text: "x ",
                        appearance: boldAppearance,
                        cells: [
                            { grapheme: "x", wideKind: "narrow" },
                            { grapheme: "", wideKind: "narrow" },
                        ],
                    },
                ],
            },
        ]);
    });

    test("rejects clipped rich-frame projections", () => {
        const frame = frameFor([[{ grapheme: "a" }, { grapheme: "b" }]]);
        expect(() =>
            parseScreenFrame(
                wireFrame(
                    frame,
                    { top: 0, left: 0, width: 2, height: 1 },
                    { top: 0, left: 0, width: 1, height: 1 },
                ),
            ),
        ).toThrow(ProtocolMismatchError);
    });

    test("snapshot profiles omit frame and actor identity", () => {
        const frame = frameFor([[{ grapheme: "x" }]]);
        const normalized = normalizeFrame(frame, "cells");
        expect(normalized).not.toHaveProperty("stateHash");
        expect(normalized).not.toHaveProperty("frameHash");
        expect(normalized).not.toHaveProperty("source");
    });
});
