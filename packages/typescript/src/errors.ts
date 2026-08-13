import type {
    ControlErrorDetails,
    DriverFailure,
    InputSequenceFailureDetails,
} from "./protocol.js";

export type TermtraceErrorCode =
    | "invalid_atom"
    | "invalid_duration"
    | "invalid_region"
    | "invalid_query"
    | "stale_state"
    | "action_busy"
    | "input_sequence_busy"
    | "postcondition_incomplete"
    | "stable_capture_incomplete"
    | "partial_input"
    | "resource_limit"
    | "protocol_mismatch"
    | "transport"
    | "request_timeout"
    | "no_match"
    | "multiple_matches"
    | "locator_timeout";

export class TermtraceError extends Error {
    constructor(
        readonly code: TermtraceErrorCode,
        message: string,
        options?: ErrorOptions,
    ) {
        super(message, options);
        this.name = new.target.name;
    }
}

export class ValidationError extends TermtraceError {}
export class InvalidAtomError extends ValidationError {
    constructor(message: string) {
        super("invalid_atom", message);
    }
}
export class InvalidDurationError extends ValidationError {
    constructor(message: string) {
        super("invalid_duration", message);
    }
}
export class InvalidRegionError extends ValidationError {
    constructor(message: string) {
        super("invalid_region", message);
    }
}
export class InvalidQueryError extends ValidationError {
    constructor(message: string) {
        super("invalid_query", message);
    }
}

export class StaleStateError extends TermtraceError {
    constructor(
        readonly expectedStateHash: string,
        readonly currentStateHash: string,
        readonly retrySafe: boolean,
    ) {
        super(
            "stale_state",
            `presented state changed (expected ${bounded(expectedStateHash)}, received ${bounded(currentStateHash)})`,
        );
    }
}

export class ActionBusyError extends TermtraceError {
    constructor(message = "another actor action is active") {
        super("action_busy", message);
    }
}
export class InputSequenceBusyError extends TermtraceError {
    constructor(message = "another input sequence is active") {
        super("input_sequence_busy", message);
    }
}

export class PostconditionIncompleteError<TResult> extends TermtraceError {
    constructor(
        readonly result: TResult,
        quality: string,
    ) {
        super("postcondition_incomplete", `action postcondition ended with ${bounded(quality)}`);
    }
}
export class StableCaptureIncompleteError<TCapture> extends TermtraceError {
    constructor(
        readonly capture: TCapture,
        quality: string,
    ) {
        super("stable_capture_incomplete", `stable capture ended with ${bounded(quality)}`);
    }
}

export class PartialInputError extends TermtraceError {
    readonly sequenceId: string;
    readonly atomCount: number;
    readonly completedAtoms: number;
    readonly bytesWritten: number;
    readonly failedAtomIndex: number;
    readonly failedAtomBytesWritten: number;
    readonly failedOperationId: string;
    readonly firstIntentEventSeq: number;
    readonly lastResultEventSeq: number;
    readonly resumeFromAtomIndex: number;
    readonly retrySafety: string;
    readonly uncertain: boolean;
    readonly evidence: string | undefined;
    readonly controlDetails: ControlErrorDetails | undefined;

    constructor(
        readonly details: InputSequenceFailureDetails,
        readonly failure: DriverFailure,
    ) {
        super(
            "partial_input",
            `input stopped after ${details.atoms_applied} complete atoms at atom ${details.failed_atom_index}`,
            { cause: failure },
        );
        this.sequenceId = details.sequence_id;
        this.atomCount = details.atom_count;
        this.completedAtoms = details.atoms_applied;
        this.bytesWritten = details.bytes_written;
        this.failedAtomIndex = details.failed_atom_index;
        this.failedAtomBytesWritten = details.failed_atom_bytes_written;
        this.failedOperationId = details.failed_operation_id;
        this.firstIntentEventSeq = details.first_intent_event_seq;
        this.lastResultEventSeq = details.last_result_event_seq;
        this.resumeFromAtomIndex = details.resume_from_atom_index;
        this.retrySafety = failure.retrySafety;
        this.uncertain = failure.uncertain;
        this.evidence = failure.evidence;
        this.controlDetails = failure.details;
    }
}

export class ResourceLimitError extends TermtraceError {
    constructor(
        readonly kind: "cell_count" | "serialized_bytes" | "response_bytes" | "match_count",
        readonly requested: number,
        readonly maximum: number,
    ) {
        super(
            "resource_limit",
            `${kind} limit exceeded (${requested} requested, maximum ${maximum})`,
        );
    }
}

export class ProtocolMismatchError extends TermtraceError {
    constructor(message: string, options?: ErrorOptions) {
        super("protocol_mismatch", message, options);
    }
}
export class TransportError extends TermtraceError {
    constructor(message: string, options?: ErrorOptions) {
        super("transport", message, options);
    }
}
export class RequestTimeoutError extends TermtraceError {
    readonly uncertain = true;
    readonly retrySafe = false;

    constructor(readonly timeoutMs: number) {
        super("request_timeout", `driver request exceeded its ${timeoutMs}ms deadline`);
    }
}

export class NoMatchError extends TermtraceError {
    constructor(
        readonly query: string,
        readonly stateHash?: string,
    ) {
        super(
            "no_match",
            `no screen match for ${bounded(query)}${stateHash === undefined ? "" : ` in state ${bounded(stateHash)}`}`,
        );
    }
}
export type MatchRangeDiagnostic = {
    readonly row: number;
    readonly startColumn: number;
    readonly endColumn: number;
};
export class MultipleMatchesError extends TermtraceError {
    constructor(
        readonly query: string,
        readonly count: number,
        readonly stateHash: string,
        readonly ranges: ReadonlyArray<ReadonlyArray<MatchRangeDiagnostic>>,
        readonly excerpts: ReadonlyArray<string>,
    ) {
        super(
            "multiple_matches",
            `expected one screen match for ${bounded(query)}, received ${count} in state ${bounded(stateHash)}; excerpts=${bounded(JSON.stringify(excerpts))}`,
        );
    }
}
export class LocatorTimeoutError extends TermtraceError {
    constructor(
        readonly query: string,
        readonly lastStateHash: string | undefined,
    ) {
        super("locator_timeout", `screen locator timed out for ${bounded(query)}`);
    }
}

const MAX_DIAGNOSTIC_CHARS = 240;
function bounded(value: string): string {
    return value.length <= MAX_DIAGNOSTIC_CHARS
        ? value
        : `${value.slice(0, MAX_DIAGNOSTIC_CHARS - 1)}…`;
}
