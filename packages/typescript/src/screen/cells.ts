import type { ScreenCell, ScreenRow, WideKind } from "./frame.js";

/** Preserve authoritative cells, or expand immutable cells from authoritative span coverage. */
export function materializeRow(row: ScreenRow): ReadonlyArray<ScreenCell> {
    if (row.cells !== null) return row.cells;
    const cells: ScreenCell[] = [];
    for (const span of row.spans) {
        for (const [index, cell] of span.cells.entries()) {
            cells.push(
                Object.freeze({
                    row: row.row,
                    column: span.startColumn + index,
                    grapheme: cell.grapheme,
                    wideKind: cell.wideKind,
                    appearance: span.appearance,
                }),
            );
        }
    }

    return Object.freeze(cells);
}

/** Neutral content includes blank spacer cells, but never a wide head or resolved color. */
export function cellContentIsNeutral(
    grapheme: string,
    wideKind: WideKind,
    appearance: ScreenCell["appearance"],
): boolean {
    const style = appearance.style;

    return (
        (grapheme === "" || grapheme === " ") &&
        (wideKind === "narrow" || wideKind === "spacer_head" || wideKind === "spacer_tail") &&
        style.foreground.kind === "none" &&
        style.background.kind === "none" &&
        style.underlineColor.kind === "none" &&
        !style.bold &&
        !style.faint &&
        !style.italic &&
        !style.blink &&
        !style.inverse &&
        !style.invisible &&
        !style.strikethrough &&
        !style.overline &&
        style.underline === 0 &&
        !appearance.selected &&
        appearance.resolvedForeground === null &&
        appearance.resolvedBackground === null
    );
}
