import { InvalidQueryError } from "../../errors.js";
import { StyleFilterInput } from "../../screen/style-filter.js";
import type { StyleFilter } from "../../screen/queries.js";
import type { SnapshotOptions, SnapshotProfile } from "../../screen/snapshots.js";
import type { TerminalCursorExpectation } from "./types.js";

type Mutable<Value> = { -readonly [Key in keyof Value]: Value[Key] };

type MatcherExpectationSource = {
    readonly column?: unknown;
    readonly includeCursor?: unknown;
    readonly includeModes?: unknown;
    readonly maxCells?: unknown;
    readonly row?: unknown;
    readonly visible?: unknown;
};

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
        try {
            return StyleFilterInput.parse(input);
        } catch (cause) {
            if (cause instanceof InvalidQueryError) {
                throw new TypeError(cause.message, { cause });
            }

            throw cause;
        }
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
