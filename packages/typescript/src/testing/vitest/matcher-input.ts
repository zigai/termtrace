import type { ColorSource, Rgb, WideKind } from "../../screen/frame.js";
import type { StyleFilter } from "../../screen/queries.js";
import type { SnapshotOptions, SnapshotProfile } from "../../screen/snapshots.js";
import type { TerminalCursorExpectation } from "./types.js";

export function parseTextExpectation(input: unknown): string | RegExp {
    if (typeof input === "string" || input instanceof RegExp) {
        return input;
    }
    throw new TypeError("text expectation must be a string or RegExp");
}

export function parseCursorExpectation(input: unknown): TerminalCursorExpectation {
    const value = matcherObject(input, "cursor expectation");
    const row = optionalNonnegativeInteger(value.row, "cursor row");
    const column = optionalNonnegativeInteger(value.column, "cursor column");
    const visible = optionalBoolean(value.visible, "cursor visibility");
    return Object.freeze({
        ...(row === undefined ? {} : { row }),
        ...(column === undefined ? {} : { column }),
        ...(visible === undefined ? {} : { visible }),
    });
}

export function parseCountExpectation(input: unknown): number {
    if (typeof input !== "number" || !Number.isSafeInteger(input) || input < 0) {
        throw new TypeError("matcher count must be a nonnegative safe integer");
    }
    return input;
}

export function parseStyleExpectation(input: unknown): StyleFilter {
    const value = matcherObject(input, "style expectation");
    const wideKind = optionalWideKind(value.wideKind);
    const underline = optionalNonnegativeInteger(value.underline, "underline");
    const foreground = optionalColorMatcher(value.foreground, "foreground");
    const background = optionalColorMatcher(value.background, "background");
    const underlineColor = optionalColorSource(value.underlineColor, "underline");
    const resolvedForeground = optionalRgbMatcher(value.resolvedForeground, "resolved foreground");
    const resolvedBackground = optionalRgbMatcher(value.resolvedBackground, "resolved background");
    return Object.freeze({
        ...optionalBooleanField(value, "bold"),
        ...optionalBooleanField(value, "faint"),
        ...optionalBooleanField(value, "italic"),
        ...optionalBooleanField(value, "blink"),
        ...optionalBooleanField(value, "inverse"),
        ...optionalBooleanField(value, "invisible"),
        ...optionalBooleanField(value, "strikethrough"),
        ...optionalBooleanField(value, "overline"),
        ...optionalBooleanField(value, "selected"),
        ...(underline === undefined ? {} : { underline }),
        ...(wideKind === undefined ? {} : { wideKind }),
        ...(foreground === undefined ? {} : { foreground }),
        ...(background === undefined ? {} : { background }),
        ...(underlineColor === undefined ? {} : { underlineColor }),
        ...(resolvedForeground === undefined ? {} : { resolvedForeground }),
        ...(resolvedBackground === undefined ? {} : { resolvedBackground }),
    });
}

export function parseSnapshotProfile(input: unknown): SnapshotProfile {
    if (input === undefined || input === "spans") {
        return "spans";
    }
    if (input === "text" || input === "cells") {
        return input;
    }
    throw new TypeError("snapshot profile must be text, spans, or cells");
}

export function parseSnapshotOptions(input: unknown): SnapshotOptions {
    if (input === undefined) {
        return Object.freeze({});
    }
    const value = matcherObject(input, "snapshot options");
    const includeCursor = optionalBoolean(value.includeCursor, "includeCursor");
    const includeModes = optionalBoolean(value.includeModes, "includeModes");
    const maxCells = optionalNonnegativeInteger(value.maxCells, "maxCells");
    if (maxCells === 0) {
        throw new TypeError("maxCells must be positive");
    }
    return Object.freeze({
        ...(includeCursor === undefined ? {} : { includeCursor }),
        ...(includeModes === undefined ? {} : { includeModes }),
        ...(maxCells === undefined ? {} : { maxCells }),
    });
}

export function parseSnapshotHint(input: unknown): string | undefined {
    if (input !== undefined && typeof input !== "string") {
        throw new TypeError("snapshot hint must be a string");
    }
    return input;
}

function matcherObject(input: unknown, label: string): Readonly<Record<string, unknown>> {
    if (typeof input !== "object" || input === null || Array.isArray(input)) {
        throw new TypeError(`${label} must be an object`);
    }
    return Object.fromEntries(Object.entries(input));
}

function optionalBoolean(input: unknown, label: string): boolean | undefined {
    if (input === undefined || typeof input === "boolean") {
        return input;
    }
    throw new TypeError(`${label} must be a boolean`);
}

function optionalBooleanField(
    input: Readonly<Record<string, unknown>>,
    field:
        | "bold"
        | "faint"
        | "italic"
        | "blink"
        | "inverse"
        | "invisible"
        | "strikethrough"
        | "overline"
        | "selected",
): Readonly<Record<string, boolean>> {
    const value = optionalBoolean(input[field], field);
    if (value === undefined) {
        return Object.freeze({});
    }
    return Object.freeze({ [field]: value });
}

function optionalNonnegativeInteger(input: unknown, label: string): number | undefined {
    if (input === undefined) {
        return undefined;
    }
    if (typeof input === "number" && Number.isSafeInteger(input) && input >= 0) {
        return input;
    }
    throw new TypeError(`${label} must be a nonnegative safe integer`);
}

function optionalWideKind(input: unknown): WideKind | undefined {
    if (input === undefined) {
        return undefined;
    }
    if (
        input === "narrow" ||
        input === "wide" ||
        input === "spacer_tail" ||
        input === "spacer_head"
    ) {
        return input;
    }
    throw new TypeError("wideKind has an unknown value");
}

function optionalColorMatcher(
    input: unknown,
    label: string,
): ColorSource | `#${string}` | undefined {
    if (typeof input === "string") {
        return parseHexColor(input, label);
    }
    return optionalColorSource(input, label);
}

function optionalRgbMatcher(input: unknown, label: string): Rgb | `#${string}` | null | undefined {
    if (typeof input === "string") {
        return parseHexColor(input, label);
    }
    return optionalRgb(input, label);
}

function parseHexColor(input: string, label: string): `#${string}` {
    if (!/^#[0-9a-fA-F]{6}$/u.test(input)) {
        throw new TypeError(`${label} must use #RRGGBB syntax`);
    }
    return input as `#${string}`;
}

function optionalColorSource(input: unknown, label: string): ColorSource | undefined {
    if (input === undefined) {
        return undefined;
    }
    const value = matcherObject(input, `${label} color`);
    if (value.kind === "none") {
        return Object.freeze({ kind: "none" });
    }
    if (value.kind === "palette") {
        const index = optionalNonnegativeInteger(value.value, `${label} palette index`);
        if (index === undefined || index > 255) {
            throw new TypeError(`${label} palette index must be between 0 and 255`);
        }
        return Object.freeze({ kind: "palette", value: index });
    }
    if (value.kind === "rgb") {
        return Object.freeze({ kind: "rgb", value: parseRgb(value.value, label) });
    }
    throw new TypeError(`${label} color kind is invalid`);
}

function optionalRgb(input: unknown, label: string): Rgb | null | undefined {
    if (input === undefined || input === null) {
        return input;
    }
    return parseRgb(input, label);
}

function parseRgb(input: unknown, label: string): Rgb {
    const value = matcherObject(input, label);
    const red = optionalNonnegativeInteger(value.r, `${label} red`);
    const green = optionalNonnegativeInteger(value.g, `${label} green`);
    const blue = optionalNonnegativeInteger(value.b, `${label} blue`);
    if (
        red === undefined ||
        green === undefined ||
        blue === undefined ||
        red > 255 ||
        green > 255 ||
        blue > 255
    ) {
        throw new TypeError(`${label} RGB channels must be bytes`);
    }
    return Object.freeze({ r: red, g: green, b: blue });
}
