import { randomUUID } from "node:crypto";
import {
    InvalidAtomError,
    InvalidDurationError,
    PostconditionIncompleteError,
    ProtocolMismatchError,
} from "./errors.js";
import type {
    DriverRequest as WireDriverRequest,
    InputSequenceAtom as WireInputSequenceAtom,
} from "./generated/driver-request.js";
import validateControlData from "./generated/validate-control-data.js";
import {
    encodeFrameRequest,
    parseScreenFrame,
    type FrameRequest,
    type ScreenFrame,
} from "./screen-frame.js";
import type { ControlResult, ScreenPredicate } from "./protocol.js";

export class Duration {
    private constructor(readonly nanoseconds: bigint) {
        Object.freeze(this);
    }

    static nanoseconds(value: bigint): Duration {
        if (value < 0n || value > 18_446_744_073_709_551_615n)
            throw new InvalidDurationError("nanoseconds must fit unsigned 64-bit range");
        return new Duration(value);
    }
    static milliseconds(value: number): Duration {
        return Duration.fromNumber(value, 1_000_000, "milliseconds");
    }
    static seconds(value: number): Duration {
        return Duration.fromNumber(value, 1_000_000_000, "seconds");
    }

    toWireNumber(field: string): number {
        if (this.nanoseconds > BigInt(Number.MAX_SAFE_INTEGER))
            throw new InvalidDurationError(`${field} exceeds the lossless JSON integer range`);
        return Number(this.nanoseconds);
    }

    private static fromNumber(value: number, multiplier: number, field: string): Duration {
        if (!Number.isFinite(value) || value < 0)
            throw new InvalidDurationError(`${field} must be finite and nonnegative`);
        const nanoseconds = value * multiplier;
        if (!Number.isSafeInteger(nanoseconds))
            throw new InvalidDurationError(
                `${field} must represent an exact, lossless whole nanosecond duration`,
            );
        return Duration.nanoseconds(BigInt(nanoseconds));
    }
}

export type KeyInput =
    | "enter"
    | "escape"
    | "tab"
    | "backspace"
    | "delete"
    | "insert"
    | "home"
    | "end"
    | "page_up"
    | "page_down"
    | "arrow_up"
    | "arrow_down"
    | "arrow_left"
    | "arrow_right"
    | { readonly function: number }
    | { readonly letter: string };
export type InputAtom =
    | { readonly kind: "text"; readonly text: string; readonly callerIntent?: string }
    | {
          readonly kind: "bytes";
          readonly bytes: Uint8Array;
          readonly inputKind?: "raw" | "text";
          readonly callerIntent?: string;
      }
    | {
          readonly kind: "key";
          readonly key: KeyInput;
          readonly modifiers?: {
              readonly shift?: boolean;
              readonly control?: boolean;
              readonly alt?: boolean;
              readonly superKey?: boolean;
          };
          readonly callerIntent?: string;
      }
    | {
          readonly kind: "paste";
          readonly data: string | Uint8Array;
          readonly bracketed?: boolean;
          readonly callerIntent?: string;
      };
export type ActionPostcondition =
    | { readonly kind: "none" }
    | { readonly kind: "idle"; readonly stableFor: Duration; readonly timeout: Duration }
    | {
          readonly kind: "screen";
          readonly predicate: ScreenPredicate;
          readonly timeout: Duration;
      };
export type ActionOptions = {
    readonly requestId?: string;
    readonly ifState?: string;
    readonly callerIntent?: string;
    readonly pace?: Duration;
    readonly verboseAtomOutcomes?: boolean;
    readonly postcondition?: ActionPostcondition;
    readonly frame?: FrameRequest;
    readonly allowIncomplete?: boolean;
};
export type InputAtomOutcome = {
    readonly operationId: string;
    readonly inputKind: string;
    readonly bytesWritten: number;
    readonly intentEventSeq: number;
    readonly resultEventSeq: number;
};
export type ActionInputResult = {
    readonly completedAtoms: number;
    readonly bytesWritten: number;
    readonly firstIntentEventSeq: number;
    readonly lastResultEventSeq: number;
    readonly pace: Duration;
    readonly atomOutcomes: ReadonlyArray<InputAtomOutcome> | null;
};
export type ActionCompletion =
    | { readonly kind: "none" }
    | {
          readonly kind: "idle";
          readonly quality: "stable" | "child_completed" | "output_closed" | "deadline";
      }
    | {
          readonly kind: "screen";
          readonly quality:
              | "matched"
              | "matched_unsettled"
              | "child_completed"
              | "output_closed"
              | "deadline";
      };
export type CanonicalBoundary = {
    readonly eventHash: string;
    readonly nextEventSeq: number;
    readonly outputBytes: number;
};
export type ActionBoundaries = {
    readonly durable: CanonicalBoundary;
    readonly observed: CanonicalBoundary;
};
export type ActionResult = {
    readonly requestId: string;
    readonly input: ActionInputResult;
    readonly completion: ActionCompletion;
    readonly boundaries: ActionBoundaries;
    readonly frame: ScreenFrame;
};

export function normalizeInputAtoms(
    atoms: ReadonlyArray<InputAtom>,
    requestId: string,
    callerIntent?: string,
): ReadonlyArray<WireInputSequenceAtom> {
    if (atoms.length < 1 || atoms.length > 4096)
        throw new InvalidAtomError("input must contain 1 through 4096 atoms");
    return Object.freeze(
        atoms.map((atom, index) => encodeAtom(atom, operationId(requestId, index), callerIntent)),
    );
}

type WireControlMethod = Extract<WireDriverRequest["method"], { readonly method: "control" }>;
type WireActionCommand = Extract<WireControlMethod["command"], { readonly command: "act" }>;
type ActionCommand = Readonly<
    Omit<WireActionCommand, "atoms"> & {
        readonly atoms: ReadonlyArray<WireInputSequenceAtom>;
    }
>;

export function encodeAction(
    atoms: ReadonlyArray<InputAtom>,
    options: ActionOptions = {},
): { readonly requestId: string; readonly command: ActionCommand } {
    const requestId = options.requestId ?? randomUUID();
    const encodedAtoms = normalizeInputAtoms(atoms, requestId, options.callerIntent);
    const pace = options.pace ?? Duration.nanoseconds(0n);
    if (pace.nanoseconds > 60_000_000_000n)
        throw new InvalidDurationError("pace must not exceed 60 seconds");
    if (pace.nanoseconds * BigInt(atoms.length) > 60_000_000_000n)
        throw new InvalidDurationError("total action pacing must not exceed 60 seconds");
    const postcondition = encodePostcondition(options.postcondition ?? { kind: "none" });
    return Object.freeze({
        requestId,
        command: Object.freeze({
            command: "act",
            atoms: encodedAtoms,
            pace_ns: pace.toWireNumber("pace"),
            include_atom_outcomes: options.verboseAtomOutcomes ?? false,
            postcondition,
            projection: encodeFrameRequest(options.frame),
        }),
    });
}

export function parseActionResult(
    input: unknown,
    expectedRequestId: string,
    allowIncomplete: boolean,
): ActionResult {
    const value = parseWireActionResult(input);
    if (value.kind !== "action_applied")
        throw new ProtocolMismatchError("action response kind is invalid");
    const wireInput = value.input;
    const wirePostcondition = value.postcondition;
    const boundaries = parseActionBoundaries(value.boundaries);
    if (wirePostcondition.frame.source.source !== "actor")
        throw new ProtocolMismatchError("action terminating frame is not actor-owned");
    const frameBoundaries = parseActionBoundaries(wirePostcondition.frame.source.boundaries);
    if (!equalBoundaries(boundaries, frameBoundaries))
        throw new ProtocolMismatchError(
            "action boundaries do not match the terminating frame boundaries",
        );

    const frame = parseScreenFrame(wirePostcondition.frame);
    let completion: ActionCompletion;
    if (wirePostcondition.kind === "none") {
        completion = Object.freeze({ kind: wirePostcondition.kind });
    } else if (wirePostcondition.kind === "idle") {
        completion = Object.freeze({
            kind: wirePostcondition.kind,
            quality: wirePostcondition.quality,
        });
    } else {
        completion = Object.freeze({
            kind: wirePostcondition.kind,
            quality: wirePostcondition.quality,
        });
    }
    const atomOutcomes =
        wireInput.atoms === undefined ? null : wireInput.atoms.map(parseAtomOutcome);
    const result: ActionResult = Object.freeze({
        requestId: wireInput.sequence_id,
        input: Object.freeze({
            completedAtoms: wireInput.atom_count,
            bytesWritten: wireInput.bytes_written,
            firstIntentEventSeq: wireInput.first_intent_event_seq,
            lastResultEventSeq: wireInput.last_result_event_seq,
            pace: Duration.nanoseconds(BigInt(wireInput.pace_ns)),
            atomOutcomes: atomOutcomes === null ? null : Object.freeze(atomOutcomes),
        }),
        completion,
        boundaries,
        frame,
    });
    if (result.requestId !== expectedRequestId)
        throw new ProtocolMismatchError("action result request ID does not match request");
    const complete =
        completion.kind === "none" ||
        (completion.kind === "idle" && completion.quality === "stable") ||
        (completion.kind === "screen" &&
            (completion.quality === "matched" || completion.quality === "matched_unsettled"));
    if (!complete && !allowIncomplete) {
        if (completion.kind === "none")
            throw new ProtocolMismatchError("none completion was unexpectedly incomplete");
        throw new PostconditionIncompleteError(result, completion.quality);
    }
    return result;
}

function encodeAtom(
    atom: InputAtom,
    id: string,
    defaultIntent: string | undefined,
): WireInputSequenceAtom {
    const callerIntent = atom.callerIntent ?? defaultIntent ?? null;
    switch (atom.kind) {
        case "text":
            if (atom.text.length === 0) throw new InvalidAtomError("text atom must not be empty");
            return Object.freeze({
                input: "bytes",
                operation_id: id,
                data: Buffer.from(atom.text).toString("base64"),
                input_kind: "text",
                caller_intent: callerIntent,
            });
        case "bytes":
            if (atom.bytes.byteLength === 0)
                throw new InvalidAtomError("bytes atom must not be empty");
            return Object.freeze({
                input: "bytes",
                operation_id: id,
                data: Buffer.from(atom.bytes).toString("base64"),
                input_kind: atom.inputKind ?? "raw",
                caller_intent: callerIntent,
            });
        case "key":
            return Object.freeze({
                input: "key",
                operation_id: id,
                key: normalizeKey(atom.key),
                modifiers: Object.freeze({
                    shift: atom.modifiers?.shift ?? false,
                    control: atom.modifiers?.control ?? false,
                    alt: atom.modifiers?.alt ?? false,
                    super_key: atom.modifiers?.superKey ?? false,
                }),
                caller_intent: callerIntent,
            });
        case "paste": {
            const bytes = Buffer.from(atom.data);
            if (bytes.byteLength === 0) throw new InvalidAtomError("paste atom must not be empty");
            if (atom.bracketed === false)
                return Object.freeze({
                    input: "bytes",
                    operation_id: id,
                    data: bytes.toString("base64"),
                    input_kind: "text",
                    caller_intent: callerIntent,
                });
            return Object.freeze({
                input: "paste",
                operation_id: id,
                data: bytes.toString("base64"),
                caller_intent: callerIntent,
            });
        }
    }
}
function encodePostcondition(
    postcondition: ActionPostcondition,
): WireActionCommand["postcondition"] {
    switch (postcondition.kind) {
        case "none":
            return Object.freeze({ condition: "none" });
        case "idle":
            if (
                postcondition.stableFor.nanoseconds === 0n ||
                postcondition.timeout.nanoseconds === 0n ||
                postcondition.stableFor.nanoseconds > postcondition.timeout.nanoseconds
            )
                throw new InvalidDurationError(
                    "idle stableFor must be nonzero and no greater than timeout",
                );
            return Object.freeze({
                condition: "idle",
                stable_for_ns: postcondition.stableFor.toWireNumber("stableFor"),
                timeout_ns: postcondition.timeout.toWireNumber("timeout"),
            });
        case "screen":
            if (postcondition.timeout.nanoseconds === 0n)
                throw new InvalidDurationError("screen timeout must be nonzero");
            return Object.freeze({
                condition: "screen",
                predicate: postcondition.predicate,
                timeout_ns: postcondition.timeout.toWireNumber("timeout"),
            });
    }
}
function normalizeKey(key: KeyInput): KeyInput {
    if (typeof key === "string") {
        const named = [
            "enter",
            "escape",
            "tab",
            "backspace",
            "delete",
            "insert",
            "home",
            "end",
            "page_up",
            "page_down",
            "arrow_up",
            "arrow_down",
            "arrow_left",
            "arrow_right",
        ] as const;
        if (!named.some((value) => value === key))
            throw new InvalidAtomError("key name is not supported");
        return key;
    }
    if ("function" in key) {
        if (!Number.isSafeInteger(key.function) || key.function < 1 || key.function > 24)
            throw new InvalidAtomError("function key must be between 1 and 24");
        return Object.freeze({ function: key.function });
    }
    if (key.letter.length === 0 || Array.from(key.letter).length !== 1)
        throw new InvalidAtomError("letter key must contain exactly one Unicode scalar");
    return Object.freeze({ letter: key.letter });
}
function operationId(requestId: string, index: number): string {
    const match = /^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-)([0-9a-f]{12})$/iu.exec(
        requestId,
    );
    const prefix = match?.[1];
    const tail = match?.[2];
    if (prefix === undefined || tail === undefined)
        throw new InvalidAtomError("requestId must be a UUID");
    return `${prefix}${(BigInt(`0x${tail}`) ^ BigInt(index + 1)).toString(16).padStart(12, "0")}`;
}

function parseWireActionResult(input: unknown): ControlResult {
    if (!validateControlData(input))
        throw new ProtocolMismatchError("action result does not match the protocol schema");
    // SAFETY: the generated Ajv validator proved the complete ControlData definition.
    return input as ControlResult;
}
type WireActionResult = Extract<ControlResult, { readonly kind: "action_applied" }>;
type WireAtomOutcome = NonNullable<WireActionResult["input"]["atoms"]>[number];
type WireBoundaries = WireActionResult["boundaries"];
type WireCanonicalBoundary = WireBoundaries["durable"];

function parseAtomOutcome(value: WireAtomOutcome): InputAtomOutcome {
    return Object.freeze({
        operationId: value.operation_id,
        inputKind: value.input_kind,
        bytesWritten: value.bytes_written,
        intentEventSeq: value.intent_event_seq,
        resultEventSeq: value.result_event_seq,
    });
}

function parseActionBoundaries(value: WireBoundaries): ActionBoundaries {
    return Object.freeze({
        durable: parseCanonicalBoundary(value.durable),
        observed: parseCanonicalBoundary(value.observed),
    });
}

function parseCanonicalBoundary(value: WireCanonicalBoundary): CanonicalBoundary {
    return Object.freeze({
        eventHash: value.event_hash,
        nextEventSeq: value.next_event_seq,
        outputBytes: value.output_bytes,
    });
}

function equalBoundaries(left: ActionBoundaries, right: ActionBoundaries): boolean {
    return (
        equalCanonicalBoundary(left.durable, right.durable) &&
        equalCanonicalBoundary(left.observed, right.observed)
    );
}

function equalCanonicalBoundary(left: CanonicalBoundary, right: CanonicalBoundary): boolean {
    return (
        left.eventHash === right.eventHash &&
        left.nextEventSeq === right.nextEventSeq &&
        left.outputBytes === right.outputBytes
    );
}
