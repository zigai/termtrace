import { ProtocolMismatchError } from "../errors.js";
import type { DriverFailure } from "./failure.js";

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

type RequestConstruction<Value> =
    Value extends ReadonlyArray<infer Item>
        ? ReadonlyArray<RequestConstruction<Item>>
        : Value extends object
          ? { readonly [Key in keyof Value]: RequestConstruction<Value[Key]> }
          : Value;

export type DriverRequest = RequestConstruction<WireDriverRequest>;
export type DriverMethod = DriverRequest["method"];
export type DriverMethodName = DriverMethod["method"];

export type DriverMethodOf<Method extends DriverMethodName> = Extract<
    DriverMethod,
    { readonly method: Method }
>;

export type ControlCommand = DriverMethodOf<"control">["command"];

type WithoutSessionHandle<Method> = Method extends { readonly handle: string }
    ? Omit<Method, "handle">
    : never;

export type SessionMethodFields = WithoutSessionHandle<DriverMethod>;
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
export type { DriverFailure } from "./failure.js";

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
export function parseDriverResponse(input: JsonValue, expectedRequestId: string): DriverResponse {
    if (!validateDriverResponse(input)) {
        throw new TermtraceProtocolError("driver response does not match the protocol schema");
    }

    // SAFETY: the generated Ajv validator has proved the complete generated wire contract.
    const root = input as JsonObject & WireDriverResponse;
    if (root.version !== 5) {
        throw new TermtraceProtocolError("driver response version does not match");
    }
    if (root.result.status === "error") {
        return { _tag: "Failure", failure: failureFromWire(root.result) };
    }
    if (root.request_id !== expectedRequestId) {
        throw new TermtraceProtocolError("driver response request ID does not match");
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

/** Canonicalize and validate one client-authored request without exposing its payload in failures. */
export function encodeDriverRequest(method: DriverMethod, requestId: string): string {
    let encoded: string;
    let request: unknown;

    try {
        const authoredRequest: DriverRequest = { version: 5, request_id: requestId, method };
        const serialized = JSON.stringify(authoredRequest);
        if (serialized === undefined) {
            throw new Error("request serialization returned undefined");
        }

        encoded = serialized;
        request = JSON.parse(encoded);
    } catch {
        throw new TermtraceProtocolError("client request does not match the protocol schema");
    }

    if (!validateDriverRequest(request)) {
        throw new TermtraceProtocolError("client request does not match the protocol schema");
    }

    return encoded;
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
