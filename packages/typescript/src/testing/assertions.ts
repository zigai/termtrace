import { Duration } from "../session/actions.js";
import type { Session } from "../session/session.js";
import { NoMatchError, MultipleMatchesError } from "../errors.js";
import {
    evaluateQuery,
    matchHasStyle,
    regionQuery,
    textQuery,
    type LocatorSpec,
    type ScreenMatch,
    type StyleFilter,
} from "../screen/queries.js";
import { ScreenFrame, type Region, type ScreenCell } from "../screen/frame.js";
import { normalizeFrame, type NormalizedFrame, type SnapshotProfile } from "../screen/snapshots.js";

export type TerminalSnapshot = NormalizedFrame;

export class AssertionMismatchError extends Error {
    readonly _tag = "AssertionMismatchError" as const;
    constructor(message: string) {
        super(message);
        this.name = "AssertionMismatchError";
    }
}

/** Capture stable exactly once and normalize the actor-returned frame. */
export async function stableTerminalSnapshot(
    session: Session,
    forMs = 250,
    profile: SnapshotProfile = "spans",
): Promise<TerminalSnapshot> {
    const capture = await session.screen.captureStable({
        stableFor: Duration.milliseconds(forMs),
        timeout: Duration.seconds(10),
    });
    return normalizeFrame(capture.frame, profile);
}

/** Require exactly one pure match without silently choosing among duplicates. */
export function assertScreenMatch(frame: ScreenFrame, spec: LocatorSpec): ScreenMatch {
    const matches = evaluateQuery(frame, spec);
    const description = describe(spec);
    if (matches.length === 0) throw new NoMatchError(description, frame.stateHash);
    if (matches.length !== 1) {
        throw new MultipleMatchesError(
            description,
            matches.length,
            frame.stateHash,
            matches.map((match) => match.ranges),
            matches.slice(0, 8).map((match) => match.text.slice(0, 80)),
        );
    }
    const match = matches[0];
    if (match === undefined) throw new NoMatchError(description, frame.stateHash);
    return match;
}

/** Require at least one match, or an exact count when supplied. */
export function assertMatches(
    frame: ScreenFrame,
    spec: LocatorSpec,
    count?: number,
): ReadonlyArray<ScreenMatch> {
    const matches = evaluateQuery(frame, spec);
    if (count === undefined && matches.length === 0)
        throw new NoMatchError(describe(spec), frame.stateHash);
    if (count !== undefined && matches.length !== count) {
        throw new AssertionMismatchError(
            `expected ${count} matches, received ${matches.length}; state=${frame.stateHash}`,
        );
    }
    return matches;
}

/** Require a pure query to have no matches. */
export function assertNoMatches(frame: ScreenFrame, spec: LocatorSpec): void {
    const matches = evaluateQuery(frame, spec);
    if (matches.length > 0) {
        throw new AssertionMismatchError(
            `expected no matches, received ${matches.length}; state=${frame.stateHash}`,
        );
    }
}

/** Require the selected match text to equal a string or fully satisfy a native regex. */
export function assertMatchText(match: ScreenMatch, expected: string | RegExp): void {
    if (typeof expected === "string") {
        if (match.text !== expected)
            throw new AssertionMismatchError(
                `expected matched text ${JSON.stringify(expected)}, received ${JSON.stringify(bounded(match.text))}`,
            );
        return;
    }
    expected.lastIndex = 0;
    const result = expected.exec(match.text);
    expected.lastIndex = 0;
    if (result?.index !== 0 || result[0] !== match.text) {
        throw new AssertionMismatchError(
            `matched text ${JSON.stringify(bounded(match.text))} does not fully satisfy ${String(expected)}`,
        );
    }
}

/** Require every authoritative head cell covered by a match to satisfy a style filter. */
export function assertMatchStyle(match: ScreenMatch, expected: StyleFilter): void {
    if (!matchHasStyle(match, expected)) {
        throw new AssertionMismatchError(
            `covered cells do not satisfy ${bounded(JSON.stringify(expected))}; state=${match.stateHash}`,
        );
    }
}

/** Require no native soft-wrap or continuation facts. */
export function assertNoWrappedRows(frame: ScreenFrame): void {
    const wrapped = frame.rows
        .filter((row) => row.softWrapped || row.wrapContinuation)
        .map((row) => row.row);
    if (wrapped.length > 0)
        throw new AssertionMismatchError(
            `expected no native wrapped rows, received ${JSON.stringify(wrapped)}; state=${frame.stateHash}`,
        );
}

/** Require a bounded region to contain only unselected default-style blank cells. */
export function assertNeutralRegion(frame: ScreenFrame, region: Region): void {
    evaluateQuery(frame, regionQuery(region));
    const cells = cellsInRegion(frame, region);
    const nonNeutral = cells.filter((cell) => !isNeutral(cell)).length;
    const expected = region.width * region.height;
    if (cells.length !== expected || nonNeutral > 0) {
        throw new AssertionMismatchError(
            `expected ${expected} neutral cells, checked ${cells.length} with ${nonNeutral} non-neutral; state=${frame.stateHash}`,
        );
    }
}

/** Assert text using one frame capture when passed a session, or no I/O when passed a frame. */
export async function assertScreenText(
    subject: Session | ScreenFrame,
    expected: string | RegExp,
): Promise<void> {
    const frame =
        subject instanceof ScreenFrame ? subject : await subject.screen.frame({ detail: "spans" });
    assertMatches(frame, textQuery(expected));
}

/** Assert cursor properties using one frame capture when needed. */
export async function assertCursor(
    subject: Session | ScreenFrame,
    expected: { readonly row?: number; readonly column?: number; readonly visible?: boolean },
): Promise<void> {
    const frame =
        subject instanceof ScreenFrame ? subject : await subject.screen.frame({ detail: "spans" });
    assertCursorField("row", expected.row, frame.cursor.row);
    assertCursorField("column", expected.column, frame.cursor.column);
    assertCursorField("visible", expected.visible, frame.cursor.visible);
}

/** Reject any evidence-backed presented transient finding. */
export async function assertNoPresentedTransient(session: Session): Promise<void> {
    const report = await session.semantic.diagnose(["transient"]);
    if (report.findings.some((finding) => finding.code === "presented_transient")) {
        throw new AssertionMismatchError(
            `presented transient found\nevidence: ${session.canonicalSession}`,
        );
    }
}

/** Compare one stable normalized frame without volatile evidence identity. */
export async function assertTerminalSnapshot(
    session: Session,
    expected: TerminalSnapshot,
): Promise<void> {
    const actual = await stableTerminalSnapshot(session);
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
        throw new AssertionMismatchError(
            `terminal snapshot differs\nexpected: ${bounded(JSON.stringify(expected))}\nactual: ${bounded(JSON.stringify(actual))}`,
        );
    }
}

type MatcherResult = { readonly pass: boolean; readonly message: () => string };

/** Assertion-library-neutral methods; only expectation mismatches become matcher failures. */
export const termtraceMatchers = Object.freeze({
    async toHaveScreenText(session: Session, expected: string | RegExp): Promise<MatcherResult> {
        try {
            await assertScreenText(session, expected);
            return {
                pass: true,
                message: () => `expected presented screen not to match ${String(expected)}`,
            };
        } catch (cause: unknown) {
            if (
                cause instanceof AssertionMismatchError ||
                cause instanceof NoMatchError ||
                cause instanceof MultipleMatchesError
            ) {
                return { pass: false, message: () => cause.message };
            }
            throw cause;
        }
    },
    async toHaveCursor(
        session: Session,
        expected: { readonly row?: number; readonly column?: number; readonly visible?: boolean },
    ): Promise<MatcherResult> {
        try {
            await assertCursor(session, expected);
            return { pass: true, message: () => "expected cursor not to match" };
        } catch (cause: unknown) {
            if (cause instanceof AssertionMismatchError)
                return { pass: false, message: () => cause.message };
            throw cause;
        }
    },
});

function cellsInRegion(frame: ScreenFrame, region: Region): ScreenCell[] {
    const cells: ScreenCell[] = [];
    const bottom = region.top + region.height;
    const right = region.left + region.width;
    for (const row of frame.rows) {
        if (row.row < region.top || row.row >= bottom) continue;
        if (row.cells !== null) {
            cells.push(
                ...row.cells.filter((cell) => cell.column >= region.left && cell.column < right),
            );
            continue;
        }
        for (const span of row.spans) {
            for (let offset = 0; offset < span.cells.length; offset += 1) {
                const cell = span.cells[offset];
                const column = span.startColumn + offset;
                if (cell !== undefined && column >= region.left && column < right) {
                    cells.push(
                        Object.freeze({
                            row: row.row,
                            column,
                            grapheme: cell.grapheme,
                            wideKind: cell.wideKind,
                            appearance: span.appearance,
                        }),
                    );
                }
            }
        }
    }
    return cells;
}

function isNeutral(cell: ScreenCell): boolean {
    const style = cell.appearance.style;
    return (
        (cell.grapheme === "" || cell.grapheme === " ") &&
        (cell.wideKind === "narrow" ||
            cell.wideKind === "spacer_head" ||
            cell.wideKind === "spacer_tail") &&
        style.foreground.kind === "none" &&
        style.background.kind === "none" &&
        style.underlineColor.kind === "none" &&
        !style.bold &&
        !style.faint &&
        !style.italic &&
        style.underline === 0 &&
        !style.blink &&
        !style.inverse &&
        !style.invisible &&
        !style.strikethrough &&
        !style.overline &&
        !cell.appearance.selected &&
        cell.appearance.resolvedForeground === null &&
        cell.appearance.resolvedBackground === null
    );
}

function assertCursorField(
    field: "row" | "column" | "visible",
    expected: number | boolean | undefined,
    actual: number | boolean | null,
): void {
    if (expected !== undefined && actual !== expected)
        throw new AssertionMismatchError(
            `cursor ${field} expected ${String(expected)}, received ${String(actual)}`,
        );
}
function describe(spec: LocatorSpec): string {
    switch (spec.kind) {
        case "text":
            return String(spec.query);
        case "style":
            return `style ${JSON.stringify(spec.style)}`;
        case "row":
            return `row ${spec.row}`;
        case "region":
            return `region ${JSON.stringify(spec.region)}`;
        case "all":
            return spec.specs.map(describe).join(" and ");
    }
}
function bounded(value: string): string {
    return value.length <= 1_000 ? value : `${value.slice(0, 999)}…`;
}
