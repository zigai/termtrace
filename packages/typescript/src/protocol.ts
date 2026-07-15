/** JSON values accepted by the newline-framed driver. */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | ReadonlyArray<JsonValue>
  | { readonly [key: string]: JsonValue };

export type JsonObject = { readonly [key: string]: JsonValue };

export type DriverFailure = {
  readonly _tag: "DriverFailure";
  readonly code: string;
  readonly message: string;
  readonly retrySafety: string;
  readonly uncertain: boolean;
  readonly evidence: string | undefined;
};

export class TermtraceProtocolError extends Error {
  readonly _tag = "TermtraceProtocolError" as const;
}

export class TermtraceOperationError extends Error {
  readonly _tag = "TermtraceOperationError" as const;

  constructor(readonly failure: DriverFailure) {
    super(failure.message);
  }
}

export type DriverResponse =
  | { readonly _tag: "Success"; readonly data: JsonObject }
  | { readonly _tag: "Failure"; readonly failure: DriverFailure };

/** Parse and correlate one untrusted driver response. */
export function parseDriverResponse(input: unknown, expectedRequestId: string): DriverResponse {
  const root = parseObject(input, "response");
  if (root.version !== 1 || root.request_id !== expectedRequestId) {
    throw new TermtraceProtocolError("driver response version or request ID does not match");
  }
  const result = parseObject(root.result, "response.result");
  if (result.status === "ok") {
    return { _tag: "Success", data: parseObject(result.data, "response.result.data") };
  }
  if (result.status !== "error") {
    throw new TermtraceProtocolError("driver response status is invalid");
  }
  return {
    _tag: "Failure",
    failure: {
      _tag: "DriverFailure",
      code: parseString(result.code, "response.result.code"),
      message: parseString(result.message, "response.result.message"),
      retrySafety: parseString(result.retry_safety, "response.result.retry_safety"),
      uncertain: parseBoolean(result.uncertain, "response.result.uncertain"),
      evidence: parseOptionalString(result.evidence, "response.result.evidence"),
    },
  };
}

export function parseString(input: JsonValue | undefined, path: string): string {
  if (typeof input !== "string") {
    throw new TermtraceProtocolError(`${path} must be a string`);
  }
  return input;
}

function parseBoolean(input: JsonValue | undefined, path: string): boolean {
  if (typeof input !== "boolean") {
    throw new TermtraceProtocolError(`${path} must be a boolean`);
  }
  return input;
}

function parseOptionalString(input: JsonValue | undefined, path: string): string | undefined {
  if (input === null || input === undefined) {
    return undefined;
  }
  return parseString(input, path);
}

function parseObject(input: unknown, path: string): JsonObject {
  if (typeof input !== "object" || input === null || Array.isArray(input)) {
    throw new TermtraceProtocolError(`${path} must be an object`);
  }
  const parsed: Record<string, JsonValue> = {};
  for (const [key, value] of Object.entries(input)) {
    parsed[key] = parseJsonValue(value, `${path}.${key}`);
  }
  return parsed;
}

function parseJsonValue(input: unknown, path: string): JsonValue {
  if (input === null || typeof input === "boolean" || typeof input === "number" || typeof input === "string") {
    return input;
  }
  if (Array.isArray(input)) {
    return input.map((value) => parseJsonValue(value, path));
  }
  return parseObject(input, path);
}
