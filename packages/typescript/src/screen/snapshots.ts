import { ResourceLimitError } from "../errors.js";
import { materializeRow } from "./cells.js";
import type {
    ColorSource,
    Rgb,
    ScreenAppearance,
    ScreenCell,
    ScreenFrame,
    ScreenRow,
    ScreenSpan,
    SpanCell,
} from "./frame.js";

export type SnapshotProfile = "text" | "spans" | "cells";
export type SnapshotOptions = {
    readonly includeCursor?: boolean;
    readonly includeModes?: boolean;
    readonly maxCells?: number;
};
type NormalizedFrameRow = Pick<ScreenRow, "row" | "softWrapped" | "wrapContinuation" | "text"> & {
    readonly spans?: ReadonlyArray<ScreenSpan>;
    readonly cells?: ReadonlyArray<ScreenCell>;
};
type NormalizedFrameBase = {
    readonly version: number;
    readonly profile: SnapshotProfile;
    readonly presented: boolean;
    readonly captureQuality: ScreenFrame["captureQuality"];
    readonly geometry: ScreenFrame["geometry"];
    readonly requestedRegion: ScreenFrame["requestedRegion"];
    readonly returnedRegion: ScreenFrame["returnedRegion"];
    readonly colors: ScreenFrame["colors"];
    readonly cursor?: ScreenFrame["cursor"];
    readonly modes?: ScreenFrame["modes"];
};
type NormalizedFrameBaseBuilder = {
    -readonly [Key in keyof NormalizedFrameBase]: NormalizedFrameBase[Key];
};
export type NormalizedFrame = NormalizedFrameBase & {
    readonly rows: ReadonlyArray<NormalizedFrameRow>;
};

/** Pure deterministic review representation with volatile actor and evidence identity removed. */
export function normalizeFrame(
    frame: ScreenFrame,
    profile: SnapshotProfile,
    options: SnapshotOptions = {},
): NormalizedFrame {
    const common: NormalizedFrameBaseBuilder = {
        version: frame.version,
        profile,
        presented: frame.presented,
        captureQuality: frame.captureQuality,
        geometry: frame.geometry,
        requestedRegion: frame.requestedRegion,
        returnedRegion: frame.returnedRegion,
        colors: frame.colors,
    };
    if (options.includeCursor !== false) common.cursor = frame.cursor;
    if (options.includeModes !== false) common.modes = frame.modes;
    if (profile === "text") {
        const rows = Object.freeze(
            frame.rows.map((row) =>
                Object.freeze({
                    row: row.row,
                    softWrapped: row.softWrapped,
                    wrapContinuation: row.wrapContinuation,
                    text: row.text,
                }),
            ),
        );
        return Object.freeze({ ...common, rows });
    }
    if (profile === "spans") {
        const rows = Object.freeze(
            frame.rows.map((row) =>
                Object.freeze({
                    row: row.row,
                    softWrapped: row.softWrapped,
                    wrapContinuation: row.wrapContinuation,
                    text: row.text,
                    spans: row.spans.length > 0 ? row.spans : spansFromCells(row.cells ?? []),
                }),
            ),
        );
        return Object.freeze({ ...common, rows });
    }
    const maximum = options.maxCells ?? 4096;
    if (!Number.isSafeInteger(maximum) || maximum <= 0)
        throw new RangeError("snapshot maxCells must be a positive safe integer");
    const requested = frame.rows.reduce(
        (count, row) =>
            count +
            (row.cells?.length ??
                row.spans.reduce((spanCount, span) => spanCount + span.cells.length, 0)),
        0,
    );
    if (requested > maximum) throw new ResourceLimitError("cell_count", requested, maximum);
    const rows = Object.freeze(
        frame.rows.map((row) =>
            Object.freeze({
                row: row.row,
                softWrapped: row.softWrapped,
                wrapContinuation: row.wrapContinuation,
                text: row.text,
                cells: materializeRow(row),
            }),
        ),
    );
    return Object.freeze({ ...common, rows });
}

function spansFromCells(cells: ReadonlyArray<ScreenCell>): ReadonlyArray<ScreenSpan> {
    type SpanDraft = {
        readonly startColumn: number;
        endColumn: number;
        cellCount: number;
        text: string;
        readonly appearance: ScreenAppearance;
        readonly cells: SpanCell[];
    };
    const drafts: SpanDraft[] = [];
    for (const cell of cells) {
        const last = drafts.at(-1);
        const spanCell = Object.freeze({ grapheme: cell.grapheme, wideKind: cell.wideKind });
        if (
            last !== undefined &&
            last.endColumn === cell.column &&
            equalAppearance(last.appearance, cell.appearance)
        ) {
            last.endColumn = cell.column + 1;
            last.cellCount += 1;
            last.text += convenienceGrapheme(cell);
            last.cells.push(spanCell);
            continue;
        }
        drafts.push({
            startColumn: cell.column,
            endColumn: cell.column + 1,
            cellCount: 1,
            text: convenienceGrapheme(cell),
            appearance: cell.appearance,
            cells: [spanCell],
        });
    }
    return Object.freeze(
        drafts.map((span) =>
            Object.freeze({
                ...span,
                cells: Object.freeze(span.cells),
            }),
        ),
    );
}

function convenienceGrapheme(cell: ScreenCell): string {
    if (cell.grapheme.length > 0) return cell.grapheme;
    return cell.wideKind === "narrow" ? " " : "";
}

function equalAppearance(left: ScreenAppearance, right: ScreenAppearance): boolean {
    const leftStyle = left.style;
    const rightStyle = right.style;
    return (
        equalColorSource(leftStyle.foreground, rightStyle.foreground) &&
        equalColorSource(leftStyle.background, rightStyle.background) &&
        equalColorSource(leftStyle.underlineColor, rightStyle.underlineColor) &&
        leftStyle.bold === rightStyle.bold &&
        leftStyle.faint === rightStyle.faint &&
        leftStyle.italic === rightStyle.italic &&
        leftStyle.blink === rightStyle.blink &&
        leftStyle.inverse === rightStyle.inverse &&
        leftStyle.invisible === rightStyle.invisible &&
        leftStyle.strikethrough === rightStyle.strikethrough &&
        leftStyle.overline === rightStyle.overline &&
        leftStyle.underline === rightStyle.underline &&
        equalNullableRgb(left.resolvedForeground, right.resolvedForeground) &&
        equalNullableRgb(left.resolvedBackground, right.resolvedBackground) &&
        left.selected === right.selected
    );
}

function equalColorSource(left: ColorSource, right: ColorSource): boolean {
    if (left.kind !== right.kind) return false;
    if (left.kind === "none" || right.kind === "none") return true;
    if (left.kind === "palette" && right.kind === "palette") return left.value === right.value;
    if (left.kind === "rgb" && right.kind === "rgb") return equalRgb(left.value, right.value);
    return false;
}

function equalNullableRgb(left: Rgb | null, right: Rgb | null): boolean {
    if (left === null || right === null) return left === right;
    return equalRgb(left, right);
}

function equalRgb(left: Rgb, right: Rgb): boolean {
    return left.r === right.r && left.g === right.g && left.b === right.b;
}
