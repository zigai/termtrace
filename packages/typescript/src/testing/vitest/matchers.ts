import { Snapshots, type MatcherState } from "vitest";

import { MultipleMatchesError, NoMatchError } from "../../errors.js";
import { ScreenFrame } from "../../screen/frame.js";
import { cellContentIsNeutral } from "../../screen/cells.js";
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
    CountExpectation,
    CursorExpectation,
    SnapshotHintExpectation,
    SnapshotOptionsExpectation,
    SnapshotProfileExpectation,
    StyleExpectation,
    TextExpectation,
} from "./matcher-input.js";
import type { TerminalMatcherResult, TerminalMatchers } from "./types.js";

export const terminalMatchers: TerminalMatchers = {
    async toHaveScreenText(received, expected) {
        const subject =
            received instanceof Screen ? await received.frame({ detail: "spans" }) : received;

        return assertionResult(
            () => assertScreenText(subject, expected),
            `expected screen not to contain ${String(expected)}`,
        );
    },
    async toHaveCursor(received, expected) {
        const subject =
            received instanceof Screen ? await received.frame({ detail: "spans" }) : received;

        return assertionResult(
            () => assertCursor(subject, expected),
            "expected cursor not to match",
        );
    },
    async toExist(received) {
        const matches = await received.all();
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

        const matches = await received.all();
        return matcherResult(
            matches.length === expected,
            `expected ${expected} match(es), received ${matches.length}`,
            `expected count not to be ${expected}`,
        );
    },
    async toHaveText(received, expected) {
        const matches = await received.all();
        const matched = matches.length === 1 && textMatches(matches[0]?.text ?? "", expected);
        return matcherResult(
            matched,
            locatorMismatch("text", matches),
            `expected strict locator not to have text ${String(expected)}`,
        );
    },
    async toHaveStyle(received, expected) {
        const matches = await received.all();
        const match = matches[0];
        const styled =
            match !== undefined &&
            matches.length === 1 &&
            evaluateQuery(match.frame, allOf(received.spec, styleQuery(expected))).length === 1;
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
        const matches = await received.all();
        const neutral = matches.length > 0 && matches.every(matchIsNeutral);
        return matcherResult(
            neutral,
            `expected a nonempty neutral/default region, received ${matches.length} non-neutral or absent match(es)`,
            "expected region not to be neutral/default styled",
        );
    },
    async toHaveNoPresentedTransient(received) {
        return assertionResult(
            () => assertNoPresentedTransient(received),
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
    toHaveScreenText(
        this: MatcherState,
        received: unknown,
        expected: unknown,
    ): Promise<TerminalMatcherResult> {
        return terminalMatchers.toHaveScreenText.call(
            this,
            ScreenTextSubject.parse(received),
            TextExpectation.parse(expected),
        );
    },
    toHaveCursor(
        this: MatcherState,
        received: unknown,
        expected: unknown,
    ): Promise<TerminalMatcherResult> {
        return terminalMatchers.toHaveCursor.call(
            this,
            ScreenTextSubject.parse(received),
            CursorExpectation.parse(expected),
        );
    },
    toExist(this: MatcherState, received: unknown): Promise<TerminalMatcherResult> {
        return terminalMatchers.toExist.call(this, LocatorSubject.parse(received));
    },
    toHaveCount(
        this: MatcherState,
        received: unknown,
        expected: unknown,
    ): Promise<TerminalMatcherResult> {
        return terminalMatchers.toHaveCount.call(
            this,
            LocatorSubject.parse(received),
            CountExpectation.parse(expected),
        );
    },
    toHaveText(
        this: MatcherState,
        received: unknown,
        expected: unknown,
    ): Promise<TerminalMatcherResult> {
        return terminalMatchers.toHaveText.call(
            this,
            LocatorSubject.parse(received),
            TextExpectation.parse(expected),
        );
    },
    toHaveStyle(
        this: MatcherState,
        received: unknown,
        expected: unknown,
    ): Promise<TerminalMatcherResult> {
        return terminalMatchers.toHaveStyle.call(
            this,
            LocatorSubject.parse(received),
            StyleExpectation.parse(expected),
        );
    },
    toHaveNoWrappedRows(this: MatcherState, received: unknown): Promise<TerminalMatcherResult> {
        return terminalMatchers.toHaveNoWrappedRows.call(this, ScreenTextSubject.parse(received));
    },
    toHaveNeutralRegion(this: MatcherState, received: unknown): Promise<TerminalMatcherResult> {
        return terminalMatchers.toHaveNeutralRegion.call(this, LocatorSubject.parse(received));
    },
    toHaveNoPresentedTransient(
        this: MatcherState,
        received: unknown,
    ): Promise<TerminalMatcherResult> {
        return terminalMatchers.toHaveNoPresentedTransient.call(
            this,
            SessionSubject.parse(received),
        );
    },
    toMatchTerminalSnapshot(
        this: MatcherState,
        received: unknown,
        profile?: unknown,
        options?: unknown,
        hint?: unknown,
    ): Promise<TerminalMatcherResult> {
        return terminalMatchers.toMatchTerminalSnapshot.call(
            this,
            SnapshotSubject.parse(received),
            SnapshotProfileExpectation.parse(profile),
            SnapshotOptionsExpectation.parse(options),
            SnapshotHintExpectation.parse(hint),
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

const LocatorSubject = {
    parse(received: unknown): Locator {
        if (received instanceof Locator) {
            return received;
        }

        throw new TypeError("matcher requires a Termtrace Locator");
    },
};

const SessionSubject = {
    parse(received: unknown): Session {
        if (received instanceof Session) {
            return received;
        }

        throw new TypeError("matcher requires a Termtrace Session");
    },
};

const ScreenTextSubject = {
    parse(received: unknown): Session | Screen | ScreenFrame {
        if (received instanceof Screen) {
            return received;
        }

        return ScreenAssertionSubject.parse(received);
    },
};

const ScreenAssertionSubject = {
    parse(received: unknown): Session | ScreenFrame {
        if (received instanceof Session || received instanceof ScreenFrame) {
            return received;
        }

        throw new TypeError("matcher requires a Termtrace Session, Screen, or ScreenFrame");
    },
};

const SnapshotSubject = {
    parse(received: unknown): Session | Screen | ScreenFrame | Locator {
        if (received instanceof Locator) {
            return received;
        }

        return ScreenTextSubject.parse(received);
    },
};

async function resolveFrame(
    received: Session | Screen | ScreenFrame | Locator,
): Promise<ScreenFrame> {
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
    if (!(expected instanceof RegExp)) {
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
