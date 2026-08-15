import { Snapshots, type MatcherState } from "vitest";

import { MultipleMatchesError, NoMatchError } from "../../errors.js";
import { ScreenFrame, type ScreenCell, type WideKind } from "../../screen/frame.js";
import { Locator } from "../../screen/locator.js";
import { allOf, evaluateQuery, styleQuery, type ScreenMatch } from "../../screen/queries.js";
import { Screen } from "../../screen/screen.js";
import { normalizeFrame } from "../../screen/snapshots.js";
import { Session } from "../../session/session.js";
import {
    AssertionMismatchError,
    assertCursor,
    assertNoPresentedTransient,
    assertScreenText,
} from "../assertions.js";
import {
    parseCountExpectation,
    parseCursorExpectation,
    parseSnapshotHint,
    parseSnapshotOptions,
    parseSnapshotProfile,
    parseStyleExpectation,
    parseTextExpectation,
} from "./matcher-input.js";
import type { TerminalMatcherResult, TerminalMatchers } from "./types.js";

export const terminalMatchers: TerminalMatchers = {
    async toHaveScreenText(received, expected) {
        const subject =
            received instanceof Screen
                ? await received.frame({ detail: "spans" })
                : requireScreenAssertionSubject(received);
        return assertionResult(
            () => assertScreenText(subject, expected),
            `expected screen not to contain ${String(expected)}`,
        );
    },
    async toHaveCursor(received, expected) {
        const subject =
            received instanceof Screen
                ? await received.frame({ detail: "spans" })
                : requireScreenAssertionSubject(received);
        return assertionResult(
            () => assertCursor(subject, expected),
            "expected cursor not to match",
        );
    },
    async toExist(received) {
        const matches = await requireLocator(received).all();
        return matcherResult(
            matches.length > 0,
            `expected locator to resolve, but it matched ${matches.length} range(s)`,
            "expected locator not to resolve",
        );
    },
    async toHaveCount(received, expected) {
        if (!Number.isSafeInteger(expected) || expected < 0) {
            throw new RangeError("expected locator count must be a nonnegative safe integer");
        }
        const matches = await requireLocator(received).all();
        return matcherResult(
            matches.length === expected,
            `expected ${expected} match(es), received ${matches.length}`,
            `expected count not to be ${expected}`,
        );
    },
    async toHaveText(received, expected) {
        const matches = await requireLocator(received).all();
        const matched = matches.length === 1 && textMatches(matches[0]?.text ?? "", expected);
        return matcherResult(
            matched,
            locatorMismatch("text", matches),
            `expected strict locator not to have text ${String(expected)}`,
        );
    },
    async toHaveStyle(received, expected) {
        const locator = requireLocator(received);
        const matches = await locator.all();
        const match = matches[0];
        const styled =
            match !== undefined &&
            matches.length === 1 &&
            evaluateQuery(match.frame, allOf(locator.spec, styleQuery(expected))).length === 1;
        return matcherResult(
            styled,
            locatorMismatch("style", matches),
            "expected strict locator not to have the requested style",
        );
    },
    async toHaveNoWrappedRows(received) {
        const frame = await resolveFrame(received);
        const wrapped = frame.rows.filter((row) => row.softWrapped || row.wrapContinuation);
        return matcherResult(
            wrapped.length === 0,
            `expected no wrapped rows, received ${wrapped.length}`,
            "expected at least one wrapped row",
        );
    },
    async toHaveNeutralRegion(received) {
        const matches = await requireLocator(received).all();
        const neutral = matches.length > 0 && matches.every(matchIsNeutral);
        return matcherResult(
            neutral,
            `expected a nonempty neutral/default region, received ${matches.length} non-neutral or absent match(es)`,
            "expected region not to be neutral/default styled",
        );
    },
    async toHaveNoPresentedTransient(received) {
        const session = requireSession(received);
        return assertionResult(
            () => assertNoPresentedTransient(session),
            "expected a presented transient diagnostic",
        );
    },
    async toMatchTerminalSnapshot(received, profile = "spans", options = {}, hint) {
        const frame = await resolveFrame(received);
        const normalized = normalizeFrame(frame, profile, options);
        return Snapshots.toMatchSnapshot.call(this, normalized, hint);
    },
};

export const vitestMatcherRegistry = {
    toHaveScreenText(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveScreenText.call(
            this,
            received,
            parseTextExpectation(expected),
        );
    },
    toHaveCursor(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveCursor.call(this, received, parseCursorExpectation(expected));
    },
    toExist(this: MatcherState, received: unknown) {
        return terminalMatchers.toExist.call(this, received);
    },
    toHaveCount(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveCount.call(this, received, parseCountExpectation(expected));
    },
    toHaveText(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveText.call(this, received, parseTextExpectation(expected));
    },
    toHaveStyle(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveStyle.call(this, received, parseStyleExpectation(expected));
    },
    toHaveNoWrappedRows(this: MatcherState, received: unknown) {
        return terminalMatchers.toHaveNoWrappedRows.call(this, received);
    },
    toHaveNeutralRegion(this: MatcherState, received: unknown) {
        return terminalMatchers.toHaveNeutralRegion.call(this, received);
    },
    toHaveNoPresentedTransient(this: MatcherState, received: unknown) {
        return terminalMatchers.toHaveNoPresentedTransient.call(this, received);
    },
    toMatchTerminalSnapshot(
        this: MatcherState,
        received: unknown,
        profile?: unknown,
        options?: unknown,
        hint?: unknown,
    ) {
        return terminalMatchers.toMatchTerminalSnapshot.call(
            this,
            received,
            parseSnapshotProfile(profile),
            parseSnapshotOptions(options),
            parseSnapshotHint(hint),
        );
    },
};

async function assertionResult(
    operation: () => Promise<void>,
    negatedMessage: string,
): Promise<TerminalMatcherResult> {
    try {
        await operation();
        return matcherResult(true, "expected Termtrace assertion to pass", negatedMessage);
    } catch (cause: unknown) {
        if (
            !(cause instanceof AssertionMismatchError) &&
            !(cause instanceof NoMatchError) &&
            !(cause instanceof MultipleMatchesError)
        ) {
            throw cause;
        }
        return matcherResult(false, cause.message, negatedMessage);
    }
}

function matcherResult(
    pass: boolean,
    failedMessage: string,
    negatedMessage: string,
): TerminalMatcherResult {
    return { pass, message: () => (pass ? negatedMessage : failedMessage) };
}

function requireLocator(received: unknown): Locator {
    if (received instanceof Locator) {
        return received;
    }
    throw new TypeError("matcher requires a Termtrace Locator");
}

function requireSession(received: unknown): Session {
    if (received instanceof Session) {
        return received;
    }
    throw new TypeError("matcher requires a Termtrace Session");
}

function requireScreenAssertionSubject(received: unknown): Session | ScreenFrame {
    if (received instanceof Session || received instanceof ScreenFrame) {
        return received;
    }
    throw new TypeError("matcher requires a Termtrace Session, Screen, or ScreenFrame");
}

async function resolveFrame(received: unknown): Promise<ScreenFrame> {
    if (received instanceof ScreenFrame) {
        return received;
    }
    if (received instanceof Session) {
        return received.screen.frame({ detail: "spans" });
    }
    if (received instanceof Screen) {
        return received.frame({ detail: "spans" });
    }
    if (received instanceof Locator) {
        const matches = await received.all();
        const first = matches[0];
        if (first !== undefined) {
            return first.frame;
        }
        throw new AssertionMismatchError("locator did not resolve a frame for snapshotting");
    }
    throw new TypeError("matcher requires a Termtrace Session, Screen, ScreenFrame, or Locator");
}

function textMatches(actual: string, expected: string | RegExp): boolean {
    if (typeof expected === "string") {
        return actual === expected;
    }
    expected.lastIndex = 0;
    const match = expected.exec(actual);
    expected.lastIndex = 0;
    return match?.index === 0 && match[0] === actual;
}

function locatorMismatch(assertion: string, matches: ReadonlyArray<ScreenMatch>): string {
    if (matches.length !== 1) {
        return `expected strict locator ${assertion}, but it resolved ${matches.length} match(es)`;
    }
    return `strict locator did not have the expected ${assertion}; received ${matches[0]?.excerpt ?? ""}`;
}

function matchIsNeutral(match: ScreenMatch): boolean {
    for (const range of match.ranges) {
        const row = match.frame.rows.find((candidate) => candidate.row === range.row);
        if (row === undefined) {
            return false;
        }
        for (let column = range.startColumn; column < range.endColumn; column += 1) {
            if (row.cells !== null) {
                const cell = row.cells.find((candidate) => candidate.column === column);
                if (
                    cell === undefined ||
                    !cellContentIsNeutral(cell.grapheme, cell.wideKind, cell.appearance)
                ) {
                    return false;
                }
                continue;
            }
            const span = row.spans.find(
                (candidate) => candidate.startColumn <= column && candidate.endColumn > column,
            );
            const cell = span?.cells[column - (span?.startColumn ?? column)];
            if (
                span === undefined ||
                cell === undefined ||
                !cellContentIsNeutral(cell.grapheme, cell.wideKind, span.appearance)
            ) {
                return false;
            }
        }
    }
    return true;
}

function cellContentIsNeutral(
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
