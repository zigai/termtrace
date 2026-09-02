import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, test } from "bun:test";
import { Ajv } from "ajv";
import { parseJson } from "ajv/dist/runtime/parseJson.js";

import {
    encodeDriverRequest,
    parseDriverResponse,
    TermtraceProtocolError,
    type ControlCommand,
    type DriverMethod,
} from "../src/transport/protocol.js";
import {
    InvalidAtomError,
    Keyboard,
    PartialInputError,
    Session,
    TermtraceOperationError,
    type ControlResult,
    type JsonObject,
    type JsonValue,
} from "../src/index.js";
import type { SessionDriver } from "../src/session/session.js";

type FixtureResponse = JsonObject & { readonly request_id: string };
type FixtureCase = { readonly name: string; readonly response: FixtureResponse };
type FixtureCollection = {
    readonly valid: FixtureCase[];
    readonly invalid: FixtureCase[];
};
type CyclicMethod = {
    method: "hello";
    self?: JsonObject;
};
function methodSerializedAs(value: JsonValue): DriverMethod {
    const method: DriverMethod = { method: "hello" };
    Object.defineProperty(method, "toJSON", { enumerable: true, value: () => value });
    return method;
}

const fixturePath = resolve(
    import.meta.dir,
    "../../../fixtures/conformance/protocol/driver-responses.json",
);
const fixtureJson = parseJson(readFileSync(fixturePath, "utf8"), 0);
const validateFixtures = new Ajv().compile<FixtureCollection>({
    type: "object",
    required: ["valid", "invalid"],
    additionalProperties: false,
    properties: {
        valid: { type: "array", items: { $ref: "#/$defs/case" } },
        invalid: { type: "array", items: { $ref: "#/$defs/case" } },
    },
    $defs: {
        case: {
            type: "object",
            required: ["name", "response"],
            additionalProperties: false,
            properties: {
                name: { type: "string" },
                response: {
                    type: "object",
                    required: ["request_id"],
                    properties: { request_id: { type: "string" } },
                    additionalProperties: true,
                },
            },
        },
    },
});
if (!validateFixtures(fixtureJson)) throw new Error("protocol fixtures do not match their schema");
const fixtures = fixtureJson;

describe("parseDriverResponse", () => {
    test("parses generated success variants", () => {
        const hello = fixtures.valid[0];
        const attached = fixtures.valid[1];
        if (hello === undefined || attached === undefined)
            throw new Error("valid fixtures are missing");

        const parsedHello = parseDriverResponse(hello.response, hello.response.request_id);
        const parsedAttached = parseDriverResponse(attached.response, attached.response.request_id);

        expect(parsedHello._tag).toBe("Success");
        if (parsedHello._tag === "Success") expect(parsedHello.data.kind).toBe("hello");
        expect(parsedAttached._tag).toBe("Success");
        if (parsedAttached._tag === "Success") expect(parsedAttached.data.kind).toBe("attached");
    });

    test("parses correlated structured failures", () => {
        const fixture = fixtures.valid[2];
        if (fixture === undefined) throw new Error("operation-error fixture is missing");

        const response = parseDriverResponse(fixture.response, fixture.response.request_id);

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

        const response = parseDriverResponse(fixture.response, fixture.response.request_id);

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

    test("retains complete partial input evidence and retry identity", async () => {
        const fixture = fixtures.valid[3];
        if (fixture === undefined) throw new Error("partial-input fixture is missing");
        const parsed = parseDriverResponse(fixture.response, fixture.response.request_id);
        if (parsed._tag !== "Failure") throw new Error("partial-input fixture must be a failure");
        const controlDetails = parsed.failure.details;
        if (controlDetails === undefined)
            throw new Error("partial-input control details are missing");
        let envelopeRequestId: string | undefined;
        const driver: SessionDriver = {
            request(methodFields, expectedKind, options): Promise<never> {
                expect([methodFields.method, expectedKind]).toEqual(["control", "control"]);
                const generatedRequestId = options?.requestId;
                envelopeRequestId = generatedRequestId;
                const inputSequence = controlDetails.input_sequence;
                if (
                    inputSequence === undefined ||
                    inputSequence === null ||
                    generatedRequestId === undefined
                )
                    throw new Error("partial-input test setup is incomplete");
                throw new TermtraceOperationError({
                    ...parsed.failure,
                    details: {
                        ...controlDetails,
                        input_sequence: { ...inputSequence, sequence_id: generatedRequestId },
                    },
                });
            },
        };
        const session = new Session(
            driver,
            {
                canonical_session: "/tmp/example.termtrace",
                completed: false,
                handle: "00000000-0000-0000-0000-000000000010",
                kind: "attached",
                session_id: "00000000-0000-0000-0000-000000000011",
            },
            false,
            "manual",
        );

        const cause = await session.keyboard
            .sequence([
                { kind: "text", text: "a" },
                { kind: "text", text: "b" },
                { kind: "text", text: "c" },
            ])
            .catch((cause: unknown) => cause);

        expect(cause).toBeInstanceOf(PartialInputError);
        if (!(cause instanceof PartialInputError)) throw new Error("expected a PartialInputError");
        if (envelopeRequestId === undefined)
            throw new Error("partial-input envelope request ID is missing");
        expect(cause.sequenceId).toBe(envelopeRequestId);
        expect(cause).toMatchObject({
            atomCount: 3,
            completedAtoms: 1,
            bytesWritten: 4,
            failedAtomIndex: 1,
            failedAtomBytesWritten: 2,
            failedOperationId: "00000000-0000-0000-0000-000000000007",
            firstIntentEventSeq: 8,
            lastResultEventSeq: 11,
            resumeFromAtomIndex: 2,
            retrySafety: "safe_same_request_id",
            uncertain: true,
            evidence: "/tmp/example.termtrace",
        });
        expect(cause.controlDetails?.boundaries).toEqual({
            durable: { event_hash: "durable-2", next_event_seq: 12, output_bytes: 5 },
            observed: { event_hash: "observed-2", next_event_seq: 12, output_bytes: 5 },
        });
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
                parseDriverResponse(fixture.response, fixture.response.request_id);
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
            version: 5,
            request_id: requestId,
            method: { method: "hello" },
        });
        const method = methodSerializedAs({
            method: "hello",
            secret_payload: "SECRET_SENTINEL",
        });
        try {
            encodeDriverRequest(method, requestId);
            throw new Error("invalid request was accepted");
        } catch (cause: unknown) {
            expect(cause).toBeInstanceOf(TermtraceProtocolError);
            expect(String(cause)).not.toContain("SECRET_SENTINEL");
        }
    });

    test("validates the exact encoding produced by an inherited toJSON", () => {
        const requestId = "00000000-0000-0000-0000-000000000020";
        const method: DriverMethod = { method: "hello" };
        Object.setPrototypeOf(method, {
            toJSON() {
                return { method: "hello", secret_payload: "SECRET_SENTINEL" };
            },
        });

        try {
            encodeDriverRequest(method, requestId);
            throw new Error("unvalidated toJSON replacement was accepted");
        } catch (cause: unknown) {
            expect(cause).toBeInstanceOf(TermtraceProtocolError);
            expect(String(cause)).not.toContain("SECRET_SENTINEL");
        }
    });

    test("redacts request serialization failures", () => {
        const requestId = "00000000-0000-0000-0000-000000000020";
        const method: CyclicMethod = { method: "hello" };
        method.self = method;

        try {
            encodeDriverRequest(method, requestId);
            throw new Error("cyclic request was accepted");
        } catch (cause: unknown) {
            expect(cause).toBeInstanceOf(TermtraceProtocolError);
            expect(String(cause)).not.toContain("cyclic");
        }
    });

    test("rejects malformed runtime request input", () => {
        const requestId = "00000000-0000-0000-0000-000000000020";
        const malformedMethods: DriverMethod[] = [
            methodSerializedAs(null),
            methodSerializedAs([]),
            methodSerializedAs({
                method: "complete",
                handle: "00000000-0000-0000-0000-000000000021",
                timeout_ms: "invalid",
            }),
        ];

        for (const malformedMethod of malformedMethods) {
            expect(() => encodeDriverRequest(malformedMethod, requestId)).toThrow(
                TermtraceProtocolError,
            );
        }
    });
});

describe("Keyboard.sequence", () => {
    test("builds one guarded input-sequence request", async () => {
        let captured: ControlCommand | undefined;
        const session: ConstructorParameters<typeof Keyboard>[0] = {
            control(command): Promise<ControlResult> {
                captured = command;
                return Promise.resolve({
                    kind: "acknowledged",
                    boundaries: {
                        durable: { event_hash: "durable", next_event_seq: 1, output_bytes: 0 },
                        observed: { event_hash: "observed", next_event_seq: 1, output_bytes: 0 },
                    },
                });
            },
        };
        const keyboard = new Keyboard(session);

        await keyboard.sequence(
            [
                { kind: "text", text: "go", callerIntent: "submit text" },
                {
                    kind: "key",
                    key: "enter",
                    modifiers: { control: true },
                    callerIntent: "submit key",
                },
            ],
            { paceMs: 5, verbose: true, requestId: "00000000-0000-0000-0000-000000000020" },
        );
        if (captured?.command !== "input_sequence")
            throw new Error("sequence command was not captured");

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
                {
                    kind: "key",
                    key: "enter",
                    modifiers: { control: true },
                    callerIntent: "submit key",
                },
            ],
            { paceMs: 5, verbose: true, requestId: "00000000-0000-0000-0000-000000000020" },
        );
        expect(captured).toEqual(first);
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
        expect(() => keyboard.sequence([])).toThrow(InvalidAtomError);
        expect(() =>
            keyboard.sequence(
                [
                    { kind: "text", text: "a" },
                    { kind: "text", text: "b" },
                ],
                { paceMs: 31_000 },
            ),
        ).toThrow("total input sequence pacing");
    });
});
