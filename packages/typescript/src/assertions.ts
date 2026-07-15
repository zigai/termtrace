import { TermtraceProtocolError, type JsonObject, type JsonValue } from "./protocol.js";
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
  readonly modes: JsonValue;
};

/** Produce the stable frame subset suitable for reviewed snapshots. */
export async function stableTerminalSnapshot(session: Session, forMs = 250): Promise<TerminalSnapshot> {
  const observed = await session.semantic.inspect();
  const observation = objectField(observed, "observation");
  const stateHash = stringField(observation, "state_hash");
  if (observation.source_completed !== true) {
    await session.screen.waitForPredicate(
      { predicate: "state_hash", hash: stateHash, comparison: "unchanged" },
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
  for (const [field, value] of Object.entries(expected)) {
    const wire = field === "column" ? "cursor_column" : field === "row" ? "cursor_row" : "cursor_visible";
    if (frame[wire] !== value) {
      throw new Error(`cursor ${field} expected ${String(value)}, received ${String(frame[wire])}\nevidence: ${session.canonicalSession}`);
    }
  }
}

/** Reject any evidence-backed presented transient finding. */
export async function assertNoPresentedTransient(session: Session): Promise<void> {
  const result = await session.semantic.diagnose(["transient"]);
  const report = objectField(result, "report");
  const findings = arrayField(report, "findings");
  if (findings.some((finding) => isJsonObject(finding) && finding.code === "presented_transient")) {
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

function normalizeFrame(frame: JsonObject): TerminalSnapshot {
  return {
    version: 1,
    cols: numberField(frame, "cols"),
    rows: numberField(frame, "rows"),
    viewportText: stringField(frame, "viewport_text"),
    cursor: {
      column: optionalNumberField(frame, "cursor_column"),
      row: optionalNumberField(frame, "cursor_row"),
      visible: booleanField(frame, "cursor_visible"),
    },
    modes: frame.modes ?? null,
  };
}

function objectField(value: JsonObject, field: string): JsonObject {
  const item = value[field];
  if (!isJsonObject(item)) {
    throw new TermtraceProtocolError(`${field} must be an object`);
  }
  return item;
}

function isJsonObject(value: JsonValue | undefined): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function arrayField(value: JsonObject, field: string): ReadonlyArray<JsonValue> {
  const item = value[field];
  if (!Array.isArray(item)) throw new TermtraceProtocolError(`${field} must be an array`);
  return item;
}

function stringField(value: JsonObject, field: string): string {
  const item = value[field];
  if (typeof item !== "string") throw new TermtraceProtocolError(`${field} must be a string`);
  return item;
}

function numberField(value: JsonObject, field: string): number {
  const item = value[field];
  if (typeof item !== "number") throw new TermtraceProtocolError(`${field} must be a number`);
  return item;
}

function optionalNumberField(value: JsonObject, field: string): number | null {
  const item = value[field];
  if (item === null) return null;
  return numberField(value, field);
}

function booleanField(value: JsonObject, field: string): boolean {
  const item = value[field];
  if (typeof item !== "boolean") throw new TermtraceProtocolError(`${field} must be a boolean`);
  return item;
}

function regexMatches(pattern: RegExp, text: string): boolean {
  pattern.lastIndex = 0;
  return pattern.test(text);
}
