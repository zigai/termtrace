import type { ColorSource, Rgb, WideKind } from "../../screen/frame.js";
import type { StyleFilter } from "../../screen/queries.js";
import type { SnapshotOptions, SnapshotProfile } from "../../screen/snapshots.js";
import type { TerminalCursorExpectation } from "./types.js";

type Mutable<Value> = { -readonly [Key in keyof Value]: Value[Key] };
type MatcherExpectationSource = {
    readonly background?: unknown;
    readonly blink?: unknown;
    readonly bold?: unknown;
    readonly column?: unknown;
    readonly faint?: unknown;
    readonly foreground?: unknown;
    readonly includeCursor?: unknown;
    readonly includeModes?: unknown;
    readonly inverse?: unknown;
    readonly invisible?: unknown;
    readonly italic?: unknown;
    readonly b?: unknown;
    readonly kind?: unknown;
    readonly maxCells?: unknown;
    readonly overline?: unknown;
    readonly g?: unknown;
    readonly resolvedBackground?: unknown;
    readonly resolvedForeground?: unknown;
    readonly row?: unknown;
    readonly selected?: unknown;
    readonly strikethrough?: unknown;
    readonly r?: unknown;
    readonly underline?: unknown;
    readonly underlineColor?: unknown;
    readonly value?: unknown;
    readonly visible?: unknown;
    readonly wideKind?: unknown;
};

const BOOLEAN_STYLE_FIELDS = [
    "bold",
    "faint",
    "italic",
    "blink",
    "inverse",
    "invisible",
    "strikethrough",
    "overline",
    "selected",
] as const;

export const TextExpectation = {
    parse(input: unknown): string | RegExp {
        if (input instanceof RegExp) {
            return input;
        }
        if (input instanceof Object) {
            throw new TypeError("text expectation must be a string or RegExp");
        }
        const value = String(input);
        if (input === value) {
            return value;
        }
        throw new TypeError("text expectation must be a string or RegExp");
    },
};

export const CursorExpectation = {
    parse(input: unknown): TerminalCursorExpectation {
        const value = MatcherExpectationSource.parse(input, "cursor expectation");
        const row = OptionalNonnegativeInteger.parse(value.row, "cursor row");
        const column = OptionalNonnegativeInteger.parse(value.column, "cursor column");
        const visible = OptionalBoolean.parse(value.visible, "cursor visibility");
        const expectation: Mutable<TerminalCursorExpectation> = {};
        if (row !== undefined) expectation.row = row;
        if (column !== undefined) expectation.column = column;
        if (visible !== undefined) expectation.visible = visible;
        return Object.freeze(expectation);
    },
};

export const CountExpectation = {
    parse(input: unknown): number {
        if (!Number.isSafeInteger(input)) {
            throw new TypeError("matcher count must be a nonnegative safe integer");
        }
        const value = Number(input);
        if (value < 0) {
            throw new TypeError("matcher count must be a nonnegative safe integer");
        }
        return value;
    },
};

export const StyleExpectation = {
    parse(input: unknown): StyleFilter {
        const value = MatcherExpectationSource.parse(input, "style expectation");
        const wideKind = OptionalWideKind.parse(value.wideKind);
        const underline = OptionalNonnegativeInteger.parse(value.underline, "underline");
        const foreground = OptionalColorMatcher.parse(value.foreground, "foreground");
        const background = OptionalColorMatcher.parse(value.background, "background");
        const underlineColor = OptionalColorSource.parse(value.underlineColor, "underline");
        const resolvedForeground = OptionalRgbMatcher.parse(
            value.resolvedForeground,
            "resolved foreground",
        );
        const resolvedBackground = OptionalRgbMatcher.parse(
            value.resolvedBackground,
            "resolved background",
        );
        const expectation: Mutable<StyleFilter> = {};
        for (const field of BOOLEAN_STYLE_FIELDS) {
            const fieldValue = OptionalBoolean.parse(value[field], field);
            if (fieldValue !== undefined) expectation[field] = fieldValue;
        }
        if (underline !== undefined) expectation.underline = underline;
        if (wideKind !== undefined) expectation.wideKind = wideKind;
        if (foreground !== undefined) expectation.foreground = foreground;
        if (background !== undefined) expectation.background = background;
        if (underlineColor !== undefined) expectation.underlineColor = underlineColor;
        if (resolvedForeground !== undefined) {
            expectation.resolvedForeground = resolvedForeground;
        }
        if (resolvedBackground !== undefined) {
            expectation.resolvedBackground = resolvedBackground;
        }
        return Object.freeze(expectation);
    },
};

export const SnapshotProfileExpectation = {
    parse(input: unknown): SnapshotProfile {
        if (input === undefined || input === "spans") {
            return "spans";
        }
        if (input === "text" || input === "cells") {
            return input;
        }
        throw new TypeError("snapshot profile must be text, spans, or cells");
    },
};

export const SnapshotOptionsExpectation = {
    parse(input: unknown): SnapshotOptions {
        if (input === undefined) {
            return Object.freeze({});
        }
        const value = MatcherExpectationSource.parse(input, "snapshot options");
        const includeCursor = OptionalBoolean.parse(value.includeCursor, "includeCursor");
        const includeModes = OptionalBoolean.parse(value.includeModes, "includeModes");
        const maxCells = OptionalNonnegativeInteger.parse(value.maxCells, "maxCells");
        if (maxCells === 0) {
            throw new TypeError("maxCells must be positive");
        }
        const options: Mutable<SnapshotOptions> = {};
        if (includeCursor !== undefined) options.includeCursor = includeCursor;
        if (includeModes !== undefined) options.includeModes = includeModes;
        if (maxCells !== undefined) options.maxCells = maxCells;
        return Object.freeze(options);
    },
};

export const SnapshotHintExpectation = {
    parse(input: unknown): string | undefined {
        if (input === undefined) {
            return undefined;
        }
        if (input instanceof Object) {
            throw new TypeError("snapshot hint must be a string");
        }
        const value = String(input);
        if (input === value) {
            return value;
        }
        throw new TypeError("snapshot hint must be a string");
    },
};

const MatcherExpectationSource = {
    parse(input: unknown, label: string): MatcherExpectationSource {
        if (!(input instanceof Object) || input instanceof Function || Array.isArray(input)) {
            throw new TypeError(`${label} must be an object`);
        }
        return Object.assign({}, input);
    },
};

const OptionalBoolean = {
    parse(input: unknown, label: string): boolean | undefined {
        if (input === undefined || input === true || input === false) {
            return input;
        }
        throw new TypeError(`${label} must be a boolean`);
    },
};

const OptionalNonnegativeInteger = {
    parse(input: unknown, label: string): number | undefined {
        if (input === undefined) {
            return undefined;
        }
        if (!Number.isSafeInteger(input)) {
            throw new TypeError(`${label} must be a nonnegative safe integer`);
        }
        const value = Number(input);
        if (value < 0) {
            throw new TypeError(`${label} must be a nonnegative safe integer`);
        }
        return value;
    },
};

const OptionalWideKind = {
    parse(input: unknown): WideKind | undefined {
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
    },
};

const OptionalColorMatcher = {
    parse(input: unknown, label: string): ColorSource | `#${string}` | undefined {
        if (input instanceof Object) {
            return OptionalColorSource.parse(input, label);
        }
        const stringValue = String(input);
        if (input === stringValue) {
            return parseHexColor(stringValue, label);
        }
        return OptionalColorSource.parse(input, label);
    },
};

const OptionalRgbMatcher = {
    parse(input: unknown, label: string): Rgb | `#${string}` | null | undefined {
        if (input instanceof Object) {
            return OptionalRgb.parse(input, label);
        }
        const stringValue = String(input);
        if (input === stringValue) {
            return parseHexColor(stringValue, label);
        }
        return OptionalRgb.parse(input, label);
    },
};

function parseHexColor(input: string, label: string): `#${string}` {
    if (!/^#[0-9a-fA-F]{6}$/u.test(input)) {
        throw new TypeError(`${label} must use #RRGGBB syntax`);
    }
    // SAFETY: the anchored expression proves the complete #RRGGBB representation;
    // TypeScript cannot derive a template-literal type from a regular-expression match.
    return input as `#${string}`;
}

const OptionalColorSource = {
    parse(input: unknown, label: string): ColorSource | undefined {
        if (input === undefined) {
            return undefined;
        }
        const value = MatcherExpectationSource.parse(input, `${label} color`);
        if (value.kind === "none") {
            return Object.freeze({ kind: "none" });
        }
        if (value.kind === "palette") {
            const index = OptionalNonnegativeInteger.parse(value.value, `${label} palette index`);
            if (index === undefined || index > 255) {
                throw new TypeError(`${label} palette index must be between 0 and 255`);
            }
            return Object.freeze({ kind: "palette", value: index });
        }
        if (value.kind === "rgb") {
            return Object.freeze({ kind: "rgb", value: RgbExpectation.parse(value.value, label) });
        }
        throw new TypeError(`${label} color kind is invalid`);
    },
};

const OptionalRgb = {
    parse(input: unknown, label: string): Rgb | null | undefined {
        if (input === undefined || input === null) {
            return input;
        }
        return RgbExpectation.parse(input, label);
    },
};

const RgbExpectation = {
    parse(input: unknown, label: string): Rgb {
        const value = MatcherExpectationSource.parse(input, label);
        const red = OptionalNonnegativeInteger.parse(value.r, `${label} red`);
        const green = OptionalNonnegativeInteger.parse(value.g, `${label} green`);
        const blue = OptionalNonnegativeInteger.parse(value.b, `${label} blue`);
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
    },
};
