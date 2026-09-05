import { isDeepStrictEqual } from "node:util";
import { InvalidQueryError, InvalidRegionError, ResourceLimitError } from "../errors.js";
import { StyleFilterInput } from "./style-filter.js";
import { materializeRow } from "./cells.js";
import type {
    ColorSource,
    Region,
    Rgb,
    ScreenCell,
    ScreenFrame,
    ScreenRow,
    WideKind,
} from "./frame.js";

export type TextQuery = string | RegExp;
export type StyleFilter = {
    readonly bold?: boolean;
    readonly faint?: boolean;
    readonly italic?: boolean;
    readonly blink?: boolean;
    readonly inverse?: boolean;
    readonly invisible?: boolean;
    readonly strikethrough?: boolean;
    readonly overline?: boolean;
    readonly underline?: number;
    readonly selected?: boolean;
    readonly wideKind?: WideKind;
    readonly foreground?: ColorSource | `#${string}`;
    readonly background?: ColorSource | `#${string}`;
    readonly underlineColor?: ColorSource;
    readonly resolvedForeground?: Rgb | `#${string}` | null;
    readonly resolvedBackground?: Rgb | `#${string}` | null;
};
export type LocatorSpec =
    | {
          readonly kind: "text";
          readonly query: TextQuery;
          readonly exact?: boolean;
          readonly crossSoftWrap?: boolean;
      }
    | { readonly kind: "style"; readonly style: StyleFilter }
    | { readonly kind: "row"; readonly row: number }
    | { readonly kind: "region"; readonly region: Region }
    | { readonly kind: "all"; readonly specs: ReadonlyArray<LocatorSpec> };
export type CellRange = {
    readonly row: number;
    readonly startColumn: number;
    readonly endColumn: number;
};
export type ScreenMatch = {
    readonly frame: ScreenFrame;
    readonly stateHash: string;
    readonly frameHash: string;
    readonly ranges: ReadonlyArray<CellRange>;
    readonly text: string;
    readonly query: string;
    readonly excerpt: string;
    readonly excerptTruncated: boolean;
};

const MAX_DESCRIPTION = 256;
const MAX_EXCERPT = 320;
const MAX_MATCHES = 10_000;

export function textQuery(
    query: TextQuery,
    options: { readonly exact?: boolean; readonly crossSoftWrap?: boolean } = {},
): LocatorSpec {
    validateTextQuery(query);
    return Object.freeze({
        kind: "text",
        query,
        exact: options.exact ?? false,
        crossSoftWrap: options.crossSoftWrap ?? false,
    });
}
export function styleQuery(style: StyleFilter): LocatorSpec {
    const parsed = parseStyleFilter(style);
    return Object.freeze({ kind: "style", style: Object.freeze({ ...parsed.filter }) });
}
export function rowQuery(row: number): LocatorSpec {
    if (!Number.isSafeInteger(row) || row < 0)
        throw new InvalidQueryError("row must be a nonnegative safe integer");
    return Object.freeze({ kind: "row", row });
}
export function regionQuery(region: Region): LocatorSpec {
    return Object.freeze({ kind: "region", region: Object.freeze({ ...region }) });
}
export function allOf(...specs: ReadonlyArray<LocatorSpec>): LocatorSpec {
    if (specs.length === 0)
        throw new InvalidQueryError("composed query must contain at least one query");
    const flattened = flattenSpecs(specs);
    validateComposition(flattened);
    return Object.freeze({ kind: "all", specs: Object.freeze(flattened) });
}

export function evaluateQuery(frame: ScreenFrame, spec: LocatorSpec): ReadonlyArray<ScreenMatch> {
    validateSpec(spec);
    if (frame.detail === "text")
        throw new InvalidQueryError("query requires spans or cells frame detail");
    const description = describeQuery(spec);
    const matches = evaluate(frame, spec, description);
    matches.sort(compareMatches);
    if (matches.length > MAX_MATCHES)
        throw new ResourceLimitError("match_count", matches.length, MAX_MATCHES);
    return Object.freeze(matches);
}

function evaluate(frame: ScreenFrame, spec: LocatorSpec, description: string): ScreenMatch[] {
    switch (spec.kind) {
        case "text":
            return evaluateText(frame, spec, description);
        case "style":
            return evaluateStyle(frame, spec.style, description);
        case "row": {
            validateRowInFrame(frame, spec.row);
            return evaluateArea(
                frame,
                {
                    top: spec.row,
                    left: frame.returnedRegion.left,
                    width: frame.returnedRegion.width,
                    height: 1,
                },
                description,
            );
        }
        case "region":
            validateRegionInFrame(frame, spec.region);
            return evaluateArea(frame, spec.region, description);
        case "all":
            return evaluateComposition(frame, spec.specs, description);
    }
}

function evaluateText(
    frame: ScreenFrame,
    spec: Extract<LocatorSpec, { readonly kind: "text" }>,
    description: string,
    region?: Region,
): ScreenMatch[] {
    const groups =
        spec.crossSoftWrap === true ? logicalRows(frame.rows) : frame.rows.map((row) => [row]);
    const matches: ScreenMatch[] = [];
    for (const rows of groups) {
        const mapped = mapRows(rows, region);
        for (const match of findTextMatches(mapped.text, spec.query, spec.exact === true)) {
            if (!mapped.boundaries.has(match.start) || !mapped.boundaries.has(match.end))
                throw new InvalidQueryError(
                    "text match is not aligned to complete terminal graphemes",
                );
            const ranges = rangesForOffsets(mapped.units, match.start, match.end);
            if (ranges.length === 0) continue;
            pushBounded(
                matches,
                makeMatch(frame, ranges, mapped.text.slice(match.start, match.end), description),
            );
        }
    }
    return matches;
}

function evaluateStyle(
    frame: ScreenFrame,
    filter: StyleFilter,
    description: string,
    region?: Region,
): ScreenMatch[] {
    const parsedFilter = parseStyleFilter(filter);
    const matches: ScreenMatch[] = [];
    for (const row of frame.rows) {
        const cells = materializeRow(row).filter(
            (cell) =>
                region === undefined ||
                (cell.row >= region.top &&
                    cell.row < region.top + region.height &&
                    cell.column >= region.left &&
                    cell.column < region.left + region.width),
        );
        let start = -1;
        for (let index = 0; index <= cells.length; index += 1) {
            const cell = cells[index];
            const displayed =
                cell !== undefined &&
                (cell.wideKind === "narrow" ||
                    cell.wideKind === "wide" ||
                    filter.wideKind === cell.wideKind);
            const selected = displayed && cell !== undefined && matchesStyle(cell, parsedFilter);
            if (selected) {
                if (start < 0) start = index;
                continue;
            }
            if (start < 0) continue;
            const first = cells[start];
            const last = cells[index - 1];
            if (first !== undefined && last !== undefined)
                pushBounded(
                    matches,
                    makeMatch(
                        frame,
                        [
                            {
                                row: row.row,
                                startColumn: first.column,
                                endColumn: cellEndColumn(last),
                            },
                        ],
                        textForCells(cells.slice(start, index)),
                        description,
                    ),
                );
            start = -1;
        }
    }
    return matches;
}

function evaluateArea(frame: ScreenFrame, region: Region, description: string): ScreenMatch[] {
    evaluateRegionOnly(region);
    const bottom = region.top + region.height;
    const right = region.left + region.width;
    const matches: ScreenMatch[] = [];
    for (const row of frame.rows) {
        if (row.row < region.top || row.row >= bottom) continue;
        pushBounded(
            matches,
            makeMatch(
                frame,
                [{ row: row.row, startColumn: region.left, endColumn: right }],
                textForRange(row, region.left, right),
                description,
            ),
        );
    }
    return matches;
}

function evaluateComposition(
    frame: ScreenFrame,
    specs: ReadonlyArray<LocatorSpec>,
    description: string,
): ScreenMatch[] {
    const flattened = flattenSpecs(specs);
    const region = intersectScopes(frame, flattened);
    const text = flattened.find(
        (candidate): candidate is Extract<LocatorSpec, { readonly kind: "text" }> =>
            candidate.kind === "text",
    );
    const style = flattened.find(
        (candidate): candidate is Extract<LocatorSpec, { readonly kind: "style" }> =>
            candidate.kind === "style",
    );
    if (text !== undefined) {
        const matches = evaluateText(frame, text, description, region);
        if (style === undefined) return matches;
        return matches.filter((match) => matchHasStyle(match, style.style));
    }
    if (style !== undefined) return evaluateStyle(frame, style.style, description, region);
    return region === undefined ? [] : evaluateArea(frame, region, description);
}

type TextUnit = {
    readonly start: number;
    readonly end: number;
    readonly row: number;
    readonly startColumn: number;
    readonly endColumn: number;
};
type MappedRows = {
    readonly text: string;
    readonly units: ReadonlyArray<TextUnit>;
    readonly boundaries: ReadonlySet<number>;
};
function mapRows(rows: ReadonlyArray<ScreenRow>, region?: Region): MappedRows {
    let content = "";
    const units: TextUnit[] = [];
    for (const row of rows) {
        const cells = materializeRow(row).filter(
            (cell) =>
                region === undefined ||
                (cell.row >= region.top &&
                    cell.row < region.top + region.height &&
                    cell.column >= region.left &&
                    cell.column < region.left + region.width),
        );
        for (let index = 0; index < cells.length; index += 1) {
            const cell = cells[index];
            if (cell === undefined) continue;
            const grapheme =
                cell.grapheme.length > 0 ? cell.grapheme : cell.wideKind === "narrow" ? " " : "";
            if (grapheme.length === 0) continue;
            let endColumn = cell.column + 1;
            for (let next = index + 1; next < cells.length; next += 1) {
                const continuation = cells[next];
                if (
                    continuation === undefined ||
                    continuation.grapheme.length > 0 ||
                    continuation.wideKind === "narrow"
                )
                    break;
                endColumn = continuation.column + 1;
                index = next;
            }
            const start = content.length;
            content += grapheme;
            units.push({
                start,
                end: content.length,
                row: row.row,
                startColumn: cell.column,
                endColumn,
            });
        }
    }
    const boundaries = new Set<number>([0, content.length]);
    for (const unit of units) {
        boundaries.add(unit.start);
        boundaries.add(unit.end);
    }
    return { text: content, units, boundaries };
}
function logicalRows(rows: ReadonlyArray<ScreenRow>): ReadonlyArray<ReadonlyArray<ScreenRow>> {
    const groups: ScreenRow[][] = [];
    for (const row of rows) {
        const previous = groups[groups.length - 1];
        const previousRow = previous?.[previous.length - 1];
        if (previous !== undefined && previousRow?.softWrapped === true && row.wrapContinuation)
            previous.push(row);
        else groups.push([row]);
    }
    return groups;
}
function rangesForOffsets(units: ReadonlyArray<TextUnit>, start: number, end: number): CellRange[] {
    const ranges: CellRange[] = [];
    for (const unit of units) {
        if (unit.end <= start || unit.start >= end) continue;
        const last = ranges[ranges.length - 1];
        if (last !== undefined && last.row === unit.row && last.endColumn === unit.startColumn)
            ranges[ranges.length - 1] = {
                row: last.row,
                startColumn: last.startColumn,
                endColumn: unit.endColumn,
            };
        else
            ranges.push({
                row: unit.row,
                startColumn: unit.startColumn,
                endColumn: unit.endColumn,
            });
    }
    return ranges;
}
function findTextMatches(
    text: string,
    query: TextQuery,
    exact: boolean,
): ReadonlyArray<{ readonly start: number; readonly end: number }> {
    if (query instanceof RegExp) {
        const flags = query.flags.includes("g") ? query.flags : `${query.flags}g`;
        const pattern = new RegExp(query.source, flags);
        pattern.lastIndex = 0;
        const matches: Array<{ start: number; end: number }> = [];
        for (;;) {
            const match = pattern.exec(text);
            if (match === null) break;
            if (match[0].length === 0)
                throw new InvalidQueryError(
                    "regular expressions must not produce zero-width matches",
                );
            if (!exact || (match.index === 0 && match[0].length === text.length))
                pushTextMatch(matches, { start: match.index, end: match.index + match[0].length });
        }
        return matches;
    }
    if (exact) return text === query ? [{ start: 0, end: text.length }] : [];
    const matches: Array<{ start: number; end: number }> = [];
    for (let start = text.indexOf(query); start >= 0; start = text.indexOf(query, start + 1))
        pushTextMatch(matches, { start, end: start + query.length });
    return matches;
}
function pushTextMatch(
    matches: Array<{ start: number; end: number }>,
    match: { readonly start: number; readonly end: number },
): void {
    if (matches.length >= MAX_MATCHES)
        throw new ResourceLimitError("match_count", matches.length + 1, MAX_MATCHES);
    matches.push(match);
}
function pushBounded(matches: ScreenMatch[], match: ScreenMatch): void {
    if (matches.length >= MAX_MATCHES)
        throw new ResourceLimitError("match_count", matches.length + 1, MAX_MATCHES);
    matches.push(match);
}
function validateTextQuery(query: TextQuery): void {
    if (!(query instanceof RegExp)) {
        if (query.length === 0) throw new InvalidQueryError("text query must not be empty");
        return;
    }
    const probe = new RegExp(query.source, query.flags.replaceAll("g", "").replaceAll("y", ""));
    probe.lastIndex = 0;
    const empty = probe.exec("");
    if (empty !== null && empty[0].length === 0)
        throw new InvalidQueryError("regular expression must not match an empty string");
}
function validateSpec(spec: LocatorSpec): void {
    switch (spec.kind) {
        case "text":
            validateTextQuery(spec.query);
            return;
        case "style":
            parseStyleFilter(spec.style);
            return;
        case "row":
            if (!Number.isSafeInteger(spec.row) || spec.row < 0)
                throw new InvalidQueryError("row must be a nonnegative safe integer");
            return;
        case "region":
            evaluateRegionOnly(spec.region);
            return;
        case "all": {
            if (spec.specs.length === 0)
                throw new InvalidQueryError("composed query must not be empty");
            const flattened = flattenSpecs(spec.specs);
            for (const nested of flattened) validateSpec(nested);
            validateComposition(flattened);
            return;
        }
    }
}
function evaluateRegionOnly(region: Region): void {
    if (
        !Number.isSafeInteger(region.top) ||
        !Number.isSafeInteger(region.left) ||
        !Number.isSafeInteger(region.width) ||
        !Number.isSafeInteger(region.height) ||
        region.top < 0 ||
        region.left < 0 ||
        region.width <= 0 ||
        region.height <= 0
    )
        throw new InvalidRegionError("query region must be nonempty nonnegative safe integers");
}
type ParsedColorMatcher = {
    readonly source: ColorSource | undefined;
    readonly resolved: Rgb | undefined;
};
type ParsedStyleFilter = {
    readonly filter: StyleFilter;
    readonly sourceForeground: ColorSource | undefined;
    readonly sourceBackground: ColorSource | undefined;
    readonly resolvedForeground: Rgb | null | undefined;
    readonly resolvedBackground: Rgb | null | undefined;
};

function parseStyleFilter(input: StyleFilter): ParsedStyleFilter {
    const style = StyleFilterInput.parse(input);
    const foreground = parseColorMatcher(style.foreground);
    const background = parseColorMatcher(style.background);
    return Object.freeze({
        filter: style,
        sourceForeground: foreground.source,
        sourceBackground: background.source,
        resolvedForeground: foreground.resolved ?? parseResolvedColor(style.resolvedForeground),
        resolvedBackground: background.resolved ?? parseResolvedColor(style.resolvedBackground),
    });
}

function parseColorMatcher(value: ColorSource | `#${string}` | undefined): ParsedColorMatcher {
    if (value === undefined) return Object.freeze({ source: undefined, resolved: undefined });
    if (!(value instanceof Object))
        return Object.freeze({ source: undefined, resolved: parseHexRgb(value) });
    return Object.freeze({
        source: value,
        resolved: undefined,
    });
}

function parseResolvedColor(value: Rgb | `#${string}` | null | undefined): Rgb | null | undefined {
    if (value === undefined || value === null) return value;
    return value instanceof Object ? value : parseHexRgb(value);
}

function parseHexRgb(value: string): Rgb {
    return Object.freeze({
        r: Number.parseInt(value.slice(1, 3), 16),
        g: Number.parseInt(value.slice(3, 5), 16),
        b: Number.parseInt(value.slice(5, 7), 16),
    });
}
function flattenSpecs(specs: ReadonlyArray<LocatorSpec>): LocatorSpec[] {
    const flattened: LocatorSpec[] = [];
    for (const spec of specs) {
        if (spec.kind === "all") flattened.push(...flattenSpecs(spec.specs));
        else flattened.push(spec);
    }
    return flattened;
}
function validateComposition(specs: ReadonlyArray<LocatorSpec>): void {
    if (specs.filter((spec) => spec.kind === "text").length > 1)
        throw new InvalidQueryError("only one text query may be composed");
    if (specs.filter((spec) => spec.kind === "style").length > 1)
        throw new InvalidQueryError("only one style query may be composed");
}
function validateRowInFrame(frame: ScreenFrame, row: number): void {
    if (
        row >= frame.geometry.rows ||
        row < frame.returnedRegion.top ||
        row >= frame.returnedRegion.top + frame.returnedRegion.height
    )
        throw new InvalidRegionError("query row is outside the returned frame region");
}
function validateRegionInFrame(frame: ScreenFrame, region: Region): void {
    validateRegionInGeometry(frame, region);
    const bottom = region.top + region.height;
    const right = region.left + region.width;
    const returnedBottom = frame.returnedRegion.top + frame.returnedRegion.height;
    const returnedRight = frame.returnedRegion.left + frame.returnedRegion.width;
    if (
        region.top < frame.returnedRegion.top ||
        region.left < frame.returnedRegion.left ||
        bottom > returnedBottom ||
        right > returnedRight
    )
        throw new InvalidRegionError("query region is outside the returned frame region");
}
function validateRegionInGeometry(frame: ScreenFrame, region: Region): void {
    evaluateRegionOnly(region);
    if (
        region.top + region.height > frame.geometry.rows ||
        region.left + region.width > frame.geometry.cols
    )
        throw new InvalidRegionError("query region is outside the frame geometry");
}
function intersectScopes(
    frame: ScreenFrame,
    specs: ReadonlyArray<LocatorSpec>,
): Region | undefined {
    let top = frame.returnedRegion.top;
    let left = frame.returnedRegion.left;
    let bottom = top + frame.returnedRegion.height;
    let right = left + frame.returnedRegion.width;
    for (const spec of specs) {
        if (spec.kind === "row") {
            validateRowInFrame(frame, spec.row);
            top = Math.max(top, spec.row);
            bottom = Math.min(bottom, spec.row + 1);
        }
        if (spec.kind === "region") {
            validateRegionInGeometry(frame, spec.region);
            top = Math.max(top, spec.region.top);
            left = Math.max(left, spec.region.left);
            bottom = Math.min(bottom, spec.region.top + spec.region.height);
            right = Math.min(right, spec.region.left + spec.region.width);
        }
    }
    if (top >= bottom || left >= right)
        throw new InvalidQueryError("composed region scopes do not intersect");
    const intersection = { top, left, width: right - left, height: bottom - top };
    validateRegionInFrame(frame, intersection);
    return intersection;
}
function cellEndColumn(cell: ScreenCell): number {
    return cell.column + (cell.wideKind === "wide" ? 2 : 1);
}

function matchesStyle(cell: ScreenCell, parsed: ParsedStyleFilter): boolean {
    const filter = parsed.filter;
    const style = cell.appearance.style;
    return (
        optionalEqual(filter.bold, style.bold) &&
        optionalEqual(filter.faint, style.faint) &&
        optionalEqual(filter.italic, style.italic) &&
        optionalEqual(filter.blink, style.blink) &&
        optionalEqual(filter.inverse, style.inverse) &&
        optionalEqual(filter.invisible, style.invisible) &&
        optionalEqual(filter.strikethrough, style.strikethrough) &&
        optionalEqual(filter.overline, style.overline) &&
        optionalEqual(filter.underline, style.underline) &&
        optionalEqual(filter.selected, cell.appearance.selected) &&
        optionalEqual(filter.wideKind, cell.wideKind) &&
        optionalDeepEqual(parsed.sourceForeground, style.foreground) &&
        optionalDeepEqual(parsed.sourceBackground, style.background) &&
        optionalDeepEqual(filter.underlineColor, style.underlineColor) &&
        optionalDeepEqual(parsed.resolvedForeground, cell.appearance.resolvedForeground) &&
        optionalDeepEqual(parsed.resolvedBackground, cell.appearance.resolvedBackground)
    );
}

/** Return whether every authoritative head cell covered by a match has the style. */
export function matchHasStyle(match: ScreenMatch, filter: StyleFilter): boolean {
    const parsedFilter = parseStyleFilter(filter);
    const cells = cellsForMatch(match.frame, match).filter(
        (cell) =>
            cell.wideKind === "narrow" ||
            cell.wideKind === "wide" ||
            filter.wideKind === cell.wideKind,
    );
    return cells.length > 0 && cells.every((cell) => matchesStyle(cell, parsedFilter));
}
function optionalEqual<T>(expected: T | undefined, actual: T): boolean {
    return expected === undefined || expected === actual;
}
function optionalDeepEqual<T>(expected: T | undefined, actual: T): boolean {
    return expected === undefined || isDeepStrictEqual(expected, actual);
}
function cellsForMatch(frame: ScreenFrame, match: ScreenMatch): ScreenCell[] {
    const cells: ScreenCell[] = [];
    for (const range of match.ranges) {
        const row = frame.rows.find((candidate) => candidate.row === range.row);
        if (row !== undefined)
            cells.push(
                ...materializeRow(row).filter(
                    (cell) => cell.column >= range.startColumn && cell.column < range.endColumn,
                ),
            );
    }
    return cells;
}
function textForRange(row: ScreenRow, start: number, end: number): string {
    return textForCells(
        materializeRow(row).filter((cell) => cell.column >= start && cell.column < end),
    );
}
function textForCells(cells: ReadonlyArray<ScreenCell>): string {
    let value = "";
    for (const cell of cells) {
        if (cell.grapheme.length > 0) value += cell.grapheme;
        else if (cell.wideKind === "narrow") value += " ";
    }
    return value;
}
function makeMatch(
    frame: ScreenFrame,
    ranges: ReadonlyArray<CellRange>,
    text: string,
    query: string,
): ScreenMatch {
    const excerptTruncated = text.length > MAX_EXCERPT;
    return Object.freeze({
        frame,
        stateHash: frame.stateHash,
        frameHash: frame.frameHash,
        ranges: Object.freeze(ranges.map((range) => Object.freeze({ ...range }))),
        text,
        query,
        excerpt: excerptTruncated ? `${text.slice(0, MAX_EXCERPT - 1)}…` : text,
        excerptTruncated,
    });
}
function describeQuery(spec: LocatorSpec): string {
    let value: string;
    switch (spec.kind) {
        case "text":
            value = `${spec.exact === true ? "exact " : ""}${String(spec.query)}`;
            break;
        case "style":
            value = `style ${JSON.stringify(spec.style)}`;
            break;
        case "row":
            value = `row ${spec.row}`;
            break;
        case "region":
            value = `region ${JSON.stringify(spec.region)}`;
            break;
        case "all":
            value = spec.specs.map(describeQuery).join(" and ");
            break;
    }
    return value.length <= MAX_DESCRIPTION ? value : `${value.slice(0, MAX_DESCRIPTION - 1)}…`;
}
function compareMatches(left: ScreenMatch, right: ScreenMatch): number {
    const leftRange = left.ranges[0];
    const rightRange = right.ranges[0];
    if (leftRange === undefined || rightRange === undefined)
        return left.ranges.length - right.ranges.length;
    return (
        leftRange.row - rightRange.row ||
        leftRange.startColumn - rightRange.startColumn ||
        leftRange.endColumn - rightRange.endColumn
    );
}
