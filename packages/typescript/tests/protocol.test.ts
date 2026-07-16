import { describe, expect, test } from "bun:test";

import { parseDriverResponse, TermtraceProtocolError } from "../src/protocol.js";

describe("parseDriverResponse", () => {
  test("parses correlated structured failures", () => {
    const response = parseDriverResponse(
      {
        version: 2,
        request_id: "request-1",
        result: {
          status: "error",
          code: "stale_state",
          message: "state changed",
          retry_safety: "safe_same_request_id",
          uncertain: false,
          evidence: "/evidence/session",
        },
      },
      "request-1",
    );

    expect(response).toEqual({
      _tag: "Failure",
      failure: {
        _tag: "DriverFailure",
        code: "stale_state",
        message: "state changed",
        retrySafety: "safe_same_request_id",
        uncertain: false,
        evidence: "/evidence/session",
      },
    });
  });

  test("rejects an uncorrelated response", () => {
    expect(() =>
      parseDriverResponse(
        { version: 2, request_id: "other", result: { status: "ok", data: {} } },
        "request-1",
      ),
    ).toThrow(TermtraceProtocolError);
  });
});
