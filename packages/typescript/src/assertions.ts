import type { ObservationFrame } from "./protocol.js";
import type { Session } from "./index.js";

export type TerminalSnapshot = {
  readonly version: 1;
  readonly cols: number;
  readonly rows: number;
  readonly viewportText: string;
  readonly cursor: {
    readonly column: number | null;
    readonly row: number | null;
    readonly visible: boolean;
  };
  readonly modes: ObservationFrame["modes"];
};

/** Produce the stable frame subset suitable for reviewed snapshots. */
export async function stableTerminalSnapshot(session: Session, forMs = 250): Promise<TerminalSnapshot> {
  const observed = await session.semantic.inspect();
  if (!observed.source_completed) {
    await session.screen.waitForPredicate(
      { predicate: "state_hash", hash: observed.state_hash, comparison: "unchanged" },
      { stableForMs: forMs },
    );
  }
  return normalizeFrame(await session.screen.frame());
}

/** Fail with an evidence-located screen message when text is absent. */
export async function assertScreenText(session: Session, expected: string | RegExp): Promise<void> {
  const text = await session.screen.text();
  const pass = typeof expected === "string" ? text.includes(expected) : regexMatches(expected, text);
  if (!pass) {
    throw new Error(`presented screen did not match ${String(expected)}\n${text}\nevidence: ${session.canonicalSession}`);
  }
}

/** Assert normalized cursor properties from a compact frame. */
export async function assertCursor(
  session: Session,
  expected: { readonly row?: number; readonly column?: number; readonly visible?: boolean },
): Promise<void> {
  const frame = await session.screen.frame();
  assertCursorField(session, "row", expected.row, frame.cursor_row ?? null);
  assertCursorField(session, "column", expected.column, frame.cursor_column ?? null);
  assertCursorField(session, "visible", expected.visible, frame.cursor_visible);
}

/** Reject any evidence-backed presented transient finding. */
export async function assertNoPresentedTransient(session: Session): Promise<void> {
  const report = await session.semantic.diagnose(["transient"]);
  if (report.findings.some((finding) => finding.code === "presented_transient")) {
    throw new Error(`presented transient found\nevidence: ${session.canonicalSession}`);
  }
}

/** Compare one stable normalized frame without volatile evidence identity. */
export async function assertTerminalSnapshot(session: Session, expected: TerminalSnapshot): Promise<void> {
  const actual = await stableTerminalSnapshot(session);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`terminal snapshot differs\nexpected: ${JSON.stringify(expected, null, 2)}\nactual: ${JSON.stringify(actual, null, 2)}\nevidence: ${session.canonicalSession}`);
  }
}

/** Assertion-library-neutral methods compatible with Vitest's matcher extension. */
export const termtraceMatchers = {
  async toHaveScreenText(session: Session, expected: string | RegExp): Promise<{ pass: boolean; message: () => string }> {
    try {
      await assertScreenText(session, expected);
      return { pass: true, message: () => `expected presented screen not to match ${String(expected)}` };
    } catch (cause: unknown) {
      return { pass: false, message: () => cause instanceof Error ? cause.message : String(cause) };
    }
  },
  async toHaveCursor(session: Session, expected: { readonly row?: number; readonly column?: number; readonly visible?: boolean }): Promise<{ pass: boolean; message: () => string }> {
    try {
      await assertCursor(session, expected);
      return { pass: true, message: () => "expected cursor not to match" };
    } catch (cause: unknown) {
      return { pass: false, message: () => cause instanceof Error ? cause.message : String(cause) };
    }
  },
};

function normalizeFrame(frame: ObservationFrame): TerminalSnapshot {
  return {
    version: 1,
    cols: frame.cols,
    rows: frame.rows,
    viewportText: frame.viewport_text,
    cursor: {
      column: frame.cursor_column ?? null,
      row: frame.cursor_row ?? null,
      visible: frame.cursor_visible,
    },
    modes: frame.modes,
  };
}

function assertCursorField(
  session: Session,
  field: "row" | "column" | "visible",
  expected: number | boolean | undefined,
  actual: number | boolean | null,
): void {
  if (expected !== undefined && actual !== expected) {
    throw new Error(`cursor ${field} expected ${String(expected)}, received ${String(actual)}\nevidence: ${session.canonicalSession}`);
  }
}

function regexMatches(pattern: RegExp, text: string): boolean {
  pattern.lastIndex = 0;
  return pattern.test(text);
}
