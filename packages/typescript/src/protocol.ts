import { ProtocolMismatchError } from "./errors.js";

import validateDriverRequest from "./generated/validate-driver-request.js";
import validateDriverResponse from "./generated/validate-driver-response.js";
import type { DriverRequest as WireDriverRequest } from "./generated/driver-request.js";
import type {
    ControlErrorDetails,
    DiagnoseReport,
    DriverResponse as WireDriverResponse,
    EventProjection,
    InputSequenceFailureDetails,
    Observation,
    ObservationFrame,
    ScreenPredicate,
    SemanticDiff,
    SessionCompletion,
    TimelineEntry,
    VerificationReport,
    WaitOutcome,
} from "./generated/driver-response.js";

/** JSON values accepted by the newline-framed driver request boundary. */
export type JsonValue =
    | null
    | boolean
    | number
    | string
    | ReadonlyArray<JsonValue>
    | { readonly [key: string]: JsonValue };

export type JsonObject = { readonly [key: string]: JsonValue };

type DriverResult = WireDriverResponse["result"];
type DriverSuccess = Extract<DriverResult, { readonly status: "ok" }>;
type DriverError = Extract<DriverResult, { readonly status: "error" }>;

export type DriverData = DriverSuccess["data"];
export type DriverDataKind = DriverData["kind"];
export type DriverDataOf<Kind extends DriverDataKind> = Extract<
    DriverData,
    { readonly kind: Kind }
>;
export type ControlResult = DriverDataOf<"control">["data"];
export type Artifact = DriverDataOf<"artifact">;
export type BundleResult = DriverDataOf<"bundle">;
export type EventPage = DriverDataOf<"events">;
export type TimelinePage = DriverDataOf<"timeline">;

export type {
    ControlErrorDetails,
    DiagnoseReport,
    EventProjection,
    InputSequenceFailureDetails,
    Observation,
    ObservationFrame,
    ScreenPredicate,
    SemanticDiff,
    SessionCompletion,
    TimelineEntry,
    VerificationReport,
    WaitOutcome,
};

export type DriverFailure = {
    readonly _tag: "DriverFailure";
    readonly code: string;
    readonly message: string;
    readonly retrySafety: string;
    readonly uncertain: boolean;
    readonly evidence: string | undefined;
    readonly details: ControlErrorDetails | undefined;
};

export class TermtraceProtocolError extends ProtocolMismatchError {
    readonly _tag = "TermtraceProtocolError" as const;
}

export class TermtraceOperationError extends Error {
    readonly _tag = "TermtraceOperationError" as const;

    constructor(readonly failure: DriverFailure) {
        super(failure.message);
    }
}

export type DriverResponse =
    | { readonly _tag: "Success"; readonly data: DriverData }
    | { readonly _tag: "Failure"; readonly failure: DriverFailure };

/** Parse and correlate one untrusted driver response without exposing payload data in failures. */
export function parseDriverResponse(input: unknown, expectedRequestId: string): DriverResponse {
    if (!validateDriverResponse(input)) {
        throw new TermtraceProtocolError("driver response does not match the protocol schema");
    }
    // SAFETY: the generated Ajv validator has proved the complete generated wire contract.
    const root = input as WireDriverResponse;
    if (root.version !== 5 || root.request_id !== expectedRequestId) {
        throw new TermtraceProtocolError("driver response version or request ID does not match");
    }
    if (root.result.status === "error") {
        return { _tag: "Failure", failure: failureFromWire(root.result) };
    }
    return { _tag: "Success", data: root.result.data };
}

/** Narrow method-specific data after checking its generated discriminant. */
export function expectDriverData<Kind extends DriverDataKind>(
    data: DriverData,
    expectedKind: Kind,
): DriverDataOf<Kind> {
    if (data.kind !== expectedKind) {
        throw new TermtraceProtocolError("driver returned an unexpected result kind");
    }
    // SAFETY: DriverData is a generated discriminated union and kind was checked above.
    return data as DriverDataOf<Kind>;
}

/** Validate and serialize one client-authored request without exposing its payload in failures. */
export function encodeDriverRequest(method: JsonObject, requestId: string): string {
    const request = { version: 5, request_id: requestId, method };
    if (!validateDriverRequest(request)) {
        throw new TermtraceProtocolError("client request does not match the protocol schema");
    }
    // SAFETY: the generated Ajv validator proved the complete generated request contract.
    const validated = request as WireDriverRequest;
    return JSON.stringify(validated);
}

/** Copy one trusted request builder result into the exact JSON value domain. */
export function parseJsonObject(input: unknown): JsonObject {
    if (typeof input !== "object" || input === null || Array.isArray(input)) {
        throw new TypeError("driver request method must be a JSON object");
    }
    const parsed: Record<string, JsonValue> = {};
    for (const [key, value] of Object.entries(input)) {
        parsed[key] = parseJsonValue(value);
    }
    return parsed;
}

function failureFromWire(failure: DriverError): DriverFailure {
    return {
        _tag: "DriverFailure",
        code: failure.code,
        message: failure.message,
        retrySafety: failure.retry_safety,
        uncertain: failure.uncertain,
        evidence: failure.evidence ?? undefined,
        details: failure.details ?? undefined,
    };
}

function parseJsonValue(input: unknown): JsonValue {
    if (input === null || typeof input === "boolean" || typeof input === "string") {
        return input;
    }
    if (typeof input === "number" && Number.isFinite(input)) {
        return input;
    }
    if (Array.isArray(input)) {
        return input.map(parseJsonValue);
    }
    return parseJsonObject(input);
}
