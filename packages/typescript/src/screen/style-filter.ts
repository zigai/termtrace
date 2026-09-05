import { InvalidQueryError } from "../errors.js";
import validateStyleFilter from "../transport/generated/validate-style-filter.js";
import type { StyleFilter } from "./queries.js";

/** Parse caller-owned style input into an immutable, schema-validated value. */
export const StyleFilterInput = Object.freeze({
    parse(input: unknown): StyleFilter {
        let candidate: unknown;
        try {
            candidate = structuredClone(input);
        } catch {
            throw new InvalidQueryError("style query must match the style filter schema");
        }
        if (
            !validateStyleFilter(candidate) ||
            Object.values(Object(candidate)).includes(undefined)
        ) {
            throw new InvalidQueryError("style query must match the style filter schema");
        }
        // SAFETY: the generated Ajv validator proves the full StyleFilter contract;
        // the explicit undefined check also enforces present optional-property values.
        const style = candidate as StyleFilter;
        for (const color of [
            style.foreground,
            style.background,
            style.underlineColor,
            style.resolvedForeground,
            style.resolvedBackground,
        ]) {
            if (color instanceof Object) {
                if ("kind" in color && color.kind === "rgb") Object.freeze(color.value);
                Object.freeze(color);
            }
        }
        return Object.freeze(style);
    },
});
