import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, test } from "bun:test";

import {
  encodeDriverRequest,
  parseDriverResponse,
  TermtraceProtocolError,
} from "../src/protocol.js";
import { Keyboard, type ControlResult, type JsonObject, type Session } from "../src/index.js";

type FixtureCase = { readonly name: string; readonly response: unknown };

const fixturePath = resolve(import.meta.dir, "../../../fixtures/protocol/driver-responses.json");
const fixtures = parseFixtures(JSON.parse(readFileSync(fixturePath, "utf8")));

describe("parseDriverResponse", () => {
  test("parses generated success variants", () => {
    const hello = fixtures.valid[0];
    const attached = fixtures.valid[1];
    if (hello === undefined || attached === undefined) throw new Error("valid fixtures are missing");

    const parsedHello = parseDriverResponse(hello.response, requestId(hello.response));
    const parsedAttached = parseDriverResponse(attached.response, requestId(attached.response));

    expect(parsedHello._tag).toBe("Success");
    if (parsedHello._tag === "Success") expect(parsedHello.data.kind).toBe("hello");
    expect(parsedAttached._tag).toBe("Success");
    if (parsedAttached._tag === "Success") expect(parsedAttached.data.kind).toBe("attached");
  });

  test("parses correlated structured failures", () => {
    const fixture = fixtures.valid[2];
    if (fixture === undefined) throw new Error("operation-error fixture is missing");

    const response = parseDriverResponse(fixture.response, requestId(fixture.response));

    expect(response).toEqual({
      _tag: "Failure",
      failure: {
        _tag: "DriverFailure",
        code: "stale_state",
        message: "state changed",
        retrySafety: "safe_new_request",
        uncertain: false,
        evidence: "/tmp/example.termtrace",
        details: {
          boundaries: {
            durable: { event_hash: "durable", next_event_seq: 8, output_bytes: 5 },
            observed: { event_hash: "observed", next_event_seq: 8, output_bytes: 5 },
          },
          current_presented_state_hash: "state",
          scope: "state_guard",
        },
      },
    });
  });

  test("preserves exact partial input sequence details", () => {
    const fixture = fixtures.valid[3];
    if (fixture === undefined) throw new Error("partial-input fixture is missing");

    const response = parseDriverResponse(fixture.response, requestId(fixture.response));

    expect(response._tag).toBe("Failure");
    if (response._tag === "Failure") {
      expect(response.failure.retrySafety).toBe("safe_same_request_id");
      expect(response.failure.uncertain).toBe(true);
      expect(response.failure.details?.input_sequence).toMatchObject({
        atoms_applied: 1,
        failed_atom_bytes_written: 2,
        resume_from_atom_index: 2,
      });
    }
  });

  test("rejects an uncorrelated response", () => {
    const fixture = fixtures.valid[0];
    if (fixture === undefined) throw new Error("hello fixture is missing");
    expect(() =>
      parseDriverResponse(fixture.response, "00000000-0000-0000-0000-000000000099"),
    ).toThrow(TermtraceProtocolError);
  });

  test("rejects shared invalid fixtures without payload leakage", () => {
    for (const fixture of fixtures.invalid) {
      try {
        parseDriverResponse(fixture.response, requestId(fixture.response));
        throw new Error(`invalid fixture was accepted: ${fixture.name}`);
      } catch (cause: unknown) {
        expect(cause).toBeInstanceOf(TermtraceProtocolError);
        expect(String(cause)).not.toContain("SECRET_SENTINEL");
      }
    }
  });

  test("validates outgoing requests without payload leakage", () => {
    const requestId = "00000000-0000-0000-0000-000000000020";

    expect(JSON.parse(encodeDriverRequest({ method: "hello" }, requestId))).toEqual({
      version: 4,
      request_id: requestId,
      method: { method: "hello" },
    });
    try {
      encodeDriverRequest({ method: "hello", secret_payload: "SECRET_SENTINEL" }, requestId);
      throw new Error("invalid request was accepted");
    } catch (cause: unknown) {
      expect(cause).toBeInstanceOf(TermtraceProtocolError);
      expect(String(cause)).not.toContain("SECRET_SENTINEL");
    }
  });
});

describe("Keyboard.sequence", () => {
  test("builds one guarded input-sequence request", async () => {
    let captured: JsonObject | undefined;
    const session = {
      control(command: JsonObject): Promise<ControlResult> {
        captured = command;
        return Promise.resolve({} as ControlResult);
      },
    } as unknown as Session;
    const keyboard = new Keyboard(session);

    await keyboard.sequence(
      [
        { kind: "text", text: "go", callerIntent: "submit text" },
        { kind: "key", key: "enter", modifiers: { control: true }, callerIntent: "submit key" },
      ],
      { paceMs: 5, verbose: true, requestId: "00000000-0000-0000-0000-000000000020" },
    );

    expect(captured).toMatchObject({
      command: "input_sequence",
      pace_ns: 5_000_000,
      include_atom_outcomes: true,
    });
    expect(captured?.atoms).toHaveLength(2);
    expect(captured?.atoms).toMatchObject([
      { caller_intent: "submit text" },
      { caller_intent: "submit key" },
    ]);
    expect(captured?.atoms).toMatchObject([
      { operation_id: "00000000-0000-0000-0000-000000000021" },
      { operation_id: "00000000-0000-0000-0000-000000000022" },
    ]);
    const first = captured;
    await keyboard.sequence(
      [
        { kind: "text", text: "go", callerIntent: "submit text" },
        { kind: "key", key: "enter", modifiers: { control: true }, callerIntent: "submit key" },
      ],
      { paceMs: 5, verbose: true, requestId: "00000000-0000-0000-0000-000000000020" },
    );
    expect(captured).toEqual(first);
    if (captured === undefined) throw new Error("sequence command was not captured");
    const sequenceCommand = captured;
    expect(() =>
      encodeDriverRequest(
        {
          method: "control",
          handle: "00000000-0000-0000-0000-000000000021",
          command: sequenceCommand,
          expected_presented_state_hash: null,
        },
        "00000000-0000-0000-0000-000000000020",
      ),
    ).not.toThrow();
    expect(() => keyboard.sequence([])).toThrow(RangeError);
    expect(() =>
      keyboard.sequence(
        [
          { kind: "text", text: "a" },
          { kind: "text", text: "b" },
          { kind: "text", text: "c" },
        ],
        { paceMs: 40_000 },
      ),
    ).toThrow("total input sequence pacing");
  });
});

function parseFixtures(input: unknown): { readonly valid: FixtureCase[]; readonly invalid: FixtureCase[] } {
  if (!isRecord(input) || !Array.isArray(input.valid) || !Array.isArray(input.invalid)) {
    throw new Error("protocol fixture root is invalid");
  }
  return { valid: input.valid.map(parseCase), invalid: input.invalid.map(parseCase) };
}

function parseCase(input: unknown): FixtureCase {
  if (!isRecord(input) || typeof input.name !== "string" || !("response" in input)) {
    throw new Error("protocol fixture case is invalid");
  }
  return { name: input.name, response: input.response };
}

function requestId(input: unknown): string {
  if (!isRecord(input) || typeof input.request_id !== "string") {
    throw new Error("protocol fixture response has no request ID");
  }
  return input.request_id;
}

function isRecord(input: unknown): input is Record<string, unknown> {
  return typeof input === "object" && input !== null && !Array.isArray(input);
}
