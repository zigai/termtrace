import { InvalidRegionError, ProtocolMismatchError } from "../errors.js";
import type {
    CellStyle as WireCellStyle,
    ScreenAppearance as WireScreenAppearance,
    ScreenAppearance1 as WireScreenCellAppearance,
    ScreenCell as WireScreenCell,
    ScreenCellRow as WireScreenCellRow,
    ScreenFrame as WireScreenFrame,
    ScreenSpan as WireScreenSpan,
    ScreenSpanRow as WireScreenSpanRow,
    ScreenTextRow as WireScreenTextRow,
} from "../transport/generated/driver-response.js";
import type { ScreenProjectionRequest } from "../transport/generated/driver-request.js";
import validateScreenFrame from "../transport/generated/validate-screen-frame.js";
import { evaluateQuery, type LocatorSpec, type ScreenMatch } from "./queries.js";
import type { JsonValue } from "../transport/protocol.js";

export type FrameDetail = "none" | "text" | "spans" | "cells";
export type WideKind = "narrow" | "wide" | "spacer_tail" | "spacer_head";
export type Rgb = { readonly r: number; readonly g: number; readonly b: number };

export type ColorSource =
    | { readonly kind: "none" }
    | { readonly kind: "palette"; readonly value: number }
    | { readonly kind: "rgb"; readonly value: Rgb };

export type CellStyle = {
    readonly foreground: ColorSource;
    readonly background: ColorSource;
    readonly underlineColor: ColorSource;
    readonly bold: boolean;
    readonly faint: boolean;
    readonly italic: boolean;
    readonly blink: boolean;
    readonly inverse: boolean;
    readonly invisible: boolean;
    readonly strikethrough: boolean;
    readonly overline: boolean;
    readonly underline: number;
};

export type ScreenAppearance = {
    readonly style: CellStyle;
    readonly resolvedForeground: Rgb | null;
    readonly resolvedBackground: Rgb | null;
    readonly selected: boolean;
};

export type ScreenCell = {
    readonly row: number;
    readonly column: number;
    readonly grapheme: string;
    readonly wideKind: WideKind;
    readonly appearance: ScreenAppearance;
};

export type SpanCell = { readonly grapheme: string; readonly wideKind: WideKind };

export type ScreenSpan = {
    readonly startColumn: number;
    readonly endColumn: number;
    readonly cellCount: number;
    readonly text: string;
    readonly appearance: ScreenAppearance;
    readonly cells: ReadonlyArray<SpanCell>;
};

export type ScreenRow = {
    readonly row: number;
    readonly softWrapped: boolean;
    readonly wrapContinuation: boolean;
    readonly text: string;
    readonly spans: ReadonlyArray<ScreenSpan>;
    readonly cells: ReadonlyArray<ScreenCell> | null;
};

export type Region = {
    readonly top: number;
    readonly left: number;
    readonly width: number;
    readonly height: number;
};

export type WireRegion = {
    readonly top: number;
    readonly left: number;
    readonly right: number;
    readonly bottom: number;
};

export type StatePoint =
    | { readonly selector: "latest_presented" }
    | { readonly selector: "state_id"; readonly state_id: number }
    | { readonly selector: "event_seq"; readonly event_seq: number }
    | { readonly selector: "monotonic_ns"; readonly monotonic_ns: number }
    | { readonly selector: "marker"; readonly label: string };

export type FrameRequest = {
    readonly point?: StatePoint;
    readonly detail?: FrameDetail;
    readonly region?: Region;
};

export type ScreenGeometry = { readonly cols: number; readonly rows: number };

export type CursorState = {
    readonly row: number | null;
    readonly column: number | null;
    readonly visible: boolean;
    readonly inViewport: boolean;
    readonly blinking: boolean;
    readonly style: "bar" | "block" | "underline" | "hollow_block";
    readonly wideTail: boolean | null;
};

export type ModeState = {
    readonly alternateScreen: boolean;
    readonly bracketedPaste: boolean;
    readonly synchronizedOutput: boolean;
    readonly wraparound: boolean;

    readonly values: ReadonlyArray<{
        readonly ansi: boolean;
        readonly enabled: boolean;
        readonly number: number;
    }>;
};

export type ColorState = {
    readonly foreground: Rgb;
    readonly background: Rgb;
    readonly cursor: Rgb | null;
    readonly palette: ReadonlyArray<Rgb>;
};

export type ScreenFrameSource = WireScreenFrame["source"];

export class ScreenFrame {
    readonly rows: ReadonlyArray<ScreenRow>;

    constructor(
        readonly version: number,
        readonly stateHash: string,
        readonly frameHash: string,
        readonly presented: boolean,
        readonly captureQuality: "live" | "stable" | "completed" | "incomplete",
        readonly source: ScreenFrameSource,
        readonly geometry: ScreenGeometry,
        readonly requestedRegion: Region,
        readonly returnedRegion: Region,
        readonly detail: FrameDetail,
        readonly cursor: CursorState,
        readonly modes: ModeState,
        readonly colors: ColorState,
        rows: ReadonlyArray<ScreenRow>,
    ) {
        this.rows = Object.freeze([...rows]);
        Object.freeze(this);
    }

    get viewportText(): string {
        return this.rows.map((row) => row.text).join("\n");
    }

    get viewport_text(): string {
        return this.viewportText;
    }

    get state_hash(): string {
        return this.stateHash;
    }

    get frame_hash(): string {
        return this.frameHash;
    }

    query(spec: LocatorSpec): ReadonlyArray<ScreenMatch> {
        return evaluateQuery(this, spec);
    }
}

export function normalizeRegion(region: Region): WireRegion {
    for (const [field, value] of Object.entries(region)) {
        if (!Number.isSafeInteger(value) || value < 0)
            throw new InvalidRegionError(`${field} must be a nonnegative safe integer`);
    }

    if (region.width === 0 || region.height === 0)
        throw new InvalidRegionError("region width and height must be nonzero");

    const right = region.left + region.width;
    const bottom = region.top + region.height;
    if (!Number.isSafeInteger(right) || !Number.isSafeInteger(bottom))
        throw new InvalidRegionError("region edges overflow safe integers");

    return { top: region.top, left: region.left, right, bottom };
}

export function encodeFrameRequest(
    request: FrameRequest = {},
): Readonly<ScreenProjectionRequest & Record<string, JsonValue>> {
    const detail = request.detail ?? "spans";
    if (detail === "cells" && request.region === undefined)
        throw new InvalidRegionError("cell detail requires an explicit region");

    return Object.freeze({
        detail,
        region: request.region === undefined ? null : normalizeRegion(request.region),
    } satisfies ScreenProjectionRequest);
}

const wireScreenFrameSchema = Object.freeze({
    parse(input: unknown): WireScreenFrame {
        if (!validateScreenFrame(input))
            throw new ProtocolMismatchError("screen frame does not match the protocol schema");

        // SAFETY: the generated Ajv validator proved the complete ScreenFrame definition.
        return input as WireScreenFrame;
    },
});

/** Parse a generated rich-frame DTO and project it into the immutable domain model. */
export function parseScreenFrame(input: unknown): ScreenFrame {
    const frame = wireScreenFrameSchema.parse(input);
    if (frame.version !== 1)
        throw new ProtocolMismatchError(`unsupported screen frame version ${frame.version}`);
    if (frame.geometry.cols === 0 || frame.geometry.rows === 0)
        throw new ProtocolMismatchError("screen frame geometry must be positive");

    const geometry = Object.freeze({
        cols: frame.geometry.cols,
        rows: frame.geometry.rows,
    });
    const requestedRegion = parseWireRegion(frame.requested_region, geometry, "requested region");
    const returnedRegion = parseWireRegion(frame.returned_region, geometry, "returned region");
    if (!equalRegion(requestedRegion, returnedRegion))
        throw new ProtocolMismatchError("screen frame returned region is clipped");

    const rows = parseRows(frame.data, returnedRegion);
    if (rows.length !== returnedRegion.height)
        throw new ProtocolMismatchError("screen frame row count does not match returned region");

    return new ScreenFrame(
        frame.version,
        frame.state_hash,
        frame.frame_hash,
        frame.presented,
        frame.capture_quality,
        Object.freeze({ ...frame.source }),
        geometry,
        requestedRegion,
        returnedRegion,
        frame.data.detail,
        parseCursor(frame.cursor),
        parseModes(frame.modes),
        parseColors(frame.colors),
        rows,
    );
}

function parseRows(data: WireScreenFrame["data"], region: Region): ReadonlyArray<ScreenRow> {
    switch (data.detail) {
        case "none":
            return [];
        case "text":
            return data.rows.map((row, index) => parseTextRow(row, region.top + index));
        case "spans":
            return data.rows.map((row, index) => parseSpanRow(row, region, region.top + index));
        case "cells":
            return data.rows.map((row, index) => parseCellRow(row, region, region.top + index));
    }
}

function parseTextRow(value: WireScreenTextRow, expectedRow: number): ScreenRow {
    validateRow(value.row, expectedRow);
    return makeRow(value, [], null);
}

function parseSpanRow(value: WireScreenSpanRow, region: Region, expectedRow: number): ScreenRow {
    validateRow(value.row, expectedRow);

    const spans = value.spans.map(parseSpan);
    validateSpanCoverage(spans, region);

    return makeRow(value, spans, null);
}

function parseCellRow(value: WireScreenCellRow, region: Region, expectedRow: number): ScreenRow {
    validateRow(value.row, expectedRow);

    const cells = value.cells.map(parseCell);
    if (
        cells.length !== region.width ||
        cells.some((cell, index) => cell.row !== value.row || cell.column !== region.left + index)
    )
        throw new ProtocolMismatchError("screen cell coverage is inconsistent");

    return makeRow(value, [], cells);
}

function validateRow(row: number, expectedRow: number): void {
    if (row !== expectedRow) throw new ProtocolMismatchError("screen rows are not contiguous");
}

function makeRow(
    value: WireScreenTextRow | WireScreenSpanRow | WireScreenCellRow,
    spans: ReadonlyArray<ScreenSpan>,
    cells: ReadonlyArray<ScreenCell> | null,
): ScreenRow {
    return Object.freeze({
        row: value.row,
        softWrapped: value.soft_wrapped,
        wrapContinuation: value.wrap_continuation,
        text: value.text,
        spans: Object.freeze(spans),
        cells: cells === null ? null : Object.freeze(cells),
    });
}

function parseSpan(value: WireScreenSpan): ScreenSpan {
    const cells = value.cells.map(
        (cell): SpanCell =>
            Object.freeze({
                grapheme: cell.grapheme,
                wideKind: cell.wide_kind,
            }),
    );
    if (
        value.end_column <= value.start_column ||
        value.cell_count !== value.end_column - value.start_column ||
        cells.length !== value.cell_count
    )
        throw new ProtocolMismatchError("screen span coverage is inconsistent");

    return Object.freeze({
        startColumn: value.start_column,
        endColumn: value.end_column,
        cellCount: value.cell_count,
        text: value.text,
        appearance: parseAppearance(value.appearance),
        cells: Object.freeze(cells),
    });
}

function validateSpanCoverage(spans: ReadonlyArray<ScreenSpan>, region: Region): void {
    let column = region.left;
    for (const span of spans) {
        if (span.startColumn !== column)
            throw new ProtocolMismatchError("screen spans are not contiguous");

        column = span.endColumn;
    }

    if (column !== region.left + region.width)
        throw new ProtocolMismatchError("screen spans do not cover the returned region");
}

function parseCell(value: WireScreenCell): ScreenCell {
    return Object.freeze({
        row: value.row,
        column: value.column,
        grapheme: value.grapheme,
        wideKind: value.wide_kind,
        appearance: parseAppearance(value.appearance),
    });
}

function parseAppearance(value: WireScreenAppearance | WireScreenCellAppearance): ScreenAppearance {
    return Object.freeze({
        style: parseStyle(value.style),
        resolvedForeground: value.resolved_foreground ?? null,
        resolvedBackground: value.resolved_background ?? null,
        selected: value.selected,
    });
}

function parseStyle(value: WireCellStyle): CellStyle {
    return Object.freeze({
        foreground: parseColorSource(value.foreground),
        background: parseColorSource(value.background),
        underlineColor: parseColorSource(value.underline_color),
        bold: value.bold,
        faint: value.faint,
        italic: value.italic,
        blink: value.blink,
        inverse: value.inverse,
        invisible: value.invisible,
        strikethrough: value.strikethrough,
        overline: value.overline,
        underline: value.underline,
    });
}

function parseColorSource(value: WireCellStyle["foreground"]): ColorSource {
    if (value.kind === "none") return Object.freeze({ kind: value.kind });
    if (value.kind === "palette") return Object.freeze({ kind: value.kind, value: value.value });
    return Object.freeze({ kind: value.kind, value: rgb(value.value) });
}

function parseCursor(value: WireScreenFrame["cursor"]): CursorState {
    return Object.freeze({
        row: value.row ?? null,
        column: value.column ?? null,
        visible: value.visible,
        inViewport: value.in_viewport,
        blinking: value.blinking,
        style: value.style,
        wideTail: value.wide_tail ?? null,
    });
}

function parseModes(value: WireScreenFrame["modes"]): ModeState {
    return Object.freeze({
        alternateScreen: value.alternate_screen,
        bracketedPaste: value.bracketed_paste,
        synchronizedOutput: value.synchronized_output,
        wraparound: value.wraparound,
        values: Object.freeze(
            value.values.map((mode) =>
                Object.freeze({
                    ansi: mode.ansi,
                    enabled: mode.enabled,
                    number: mode.number,
                }),
            ),
        ),
    });
}

function parseColors(value: WireScreenFrame["colors"]): ColorState {
    return Object.freeze({
        foreground: rgb(value.foreground),
        background: rgb(value.background),
        cursor: value.cursor === undefined || value.cursor === null ? null : rgb(value.cursor),
        palette: Object.freeze(value.palette.map(rgb)),
    });
}

function parseWireRegion(
    value: WireScreenFrame["requested_region"] | WireScreenFrame["returned_region"],
    geometry: ScreenGeometry,
    label: string,
): Region {
    if (
        value.right <= value.left ||
        value.bottom <= value.top ||
        value.right > geometry.cols ||
        value.bottom > geometry.rows
    )
        throw new ProtocolMismatchError(`${label} is empty or outside geometry`);

    return Object.freeze({
        top: value.top,
        left: value.left,
        width: value.right - value.left,
        height: value.bottom - value.top,
    });
}

function equalRegion(left: Region, right: Region): boolean {
    return (
        left.top === right.top &&
        left.left === right.left &&
        left.width === right.width &&
        left.height === right.height
    );
}

function rgb(value: { readonly r: number; readonly g: number; readonly b: number }): Rgb {
    return Object.freeze({ r: value.r, g: value.g, b: value.b });
}
