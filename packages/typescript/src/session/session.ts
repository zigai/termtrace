import { randomUUID } from "node:crypto";

import { DriverClient, method, type DriverRequestOptions } from "../transport/driver-client.js";
import {
    Duration,
    encodeAction,
    parseActionResult,
    type ActionOptions,
    type ActionResult,
    type InputAtom,
} from "./actions.js";
import {
    ActionBusyError,
    InvalidAtomError,
    InvalidDurationError,
    InvalidQueryError,
    InvalidRegionError,
    InputSequenceBusyError,
    PartialInputError,
    ProtocolMismatchError,
    ResourceLimitError,
    StaleStateError,
} from "../errors.js";
import { Screen } from "../screen/screen.js";
import type { StatePoint } from "../screen/frame.js";
import {
    TermtraceOperationError,
    type Artifact,
    type BundleResult,
    type ControlResult,
    type DiagnoseReport,
    type DriverDataKind,
    type DriverDataOf,
    type EventPage,
    type JsonObject,
    type JsonValue,
    type Observation,
    type SemanticDiff,
    type SessionCompletion,
    type TimelinePage,
    type VerificationReport,
    type WaitOutcome,
} from "../transport/protocol.js";
import { exactWireInteger, nonnegativeDuration } from "../wire-values.js";

export type IdleWaitOptions = WaitOptions & {
    readonly allowIncomplete?: boolean;
};

export type MutationOptions = {
    readonly requestId?: string;
    readonly requestTimeoutMs?: number;
    readonly ifState?: string;
    readonly callerIntent?: string;
    readonly signal?: AbortSignal;
};

export type KeyboardAtom = InputAtom;

export type InputSequenceOptions = MutationOptions & {
    readonly paceMs?: number;
    readonly verbose?: boolean;
};
export type WaitOptions = {
    readonly timeoutMs?: number;
    readonly stableForMs?: number;
    readonly signal?: AbortSignal;
};

type ControlSession = {
    control(command: JsonObject, options?: MutationOptions): Promise<ControlResult>;
};

type SessionIdentity = DriverDataOf<"attached">;
type DriverRequestOptionsBuilder = {
    -readonly [Key in keyof DriverRequestOptions]: DriverRequestOptions[Key];
};
type SequenceActionOptionsBuilder = {
    -readonly [Key in keyof Pick<
        ActionOptions,
        "requestId" | "pace" | "callerIntent" | "verboseAtomOutcomes"
    >]: ActionOptions[Key];
};

/** Process-local handle with explicit canonical evidence identity. */
export class Session implements AsyncDisposable {
    readonly screen: Screen;
    readonly keyboard: Keyboard;
    readonly output: Output;
    readonly pointer: Pointer;
    readonly semantic: Semantic;
    readonly events: Events;
    readonly native: Native;
    readonly evidence: Evidence;
    readonly sessionId: string;
    readonly canonicalSession: string;
    readonly #driver: DriverClient;
    readonly #handle: string;
    readonly #owned: boolean;
    readonly #evidencePolicy: "always" | "on-failure" | "manual";
    #closed = false;
    #completed = false;

    constructor(
        driver: DriverClient,
        identity: SessionIdentity,
        owned: boolean,
        evidencePolicy: "always" | "on-failure" | "manual",
    ) {
        this.#driver = driver;
        this.#handle = identity.handle;
        this.#owned = owned;
        this.#evidencePolicy = evidencePolicy;
        this.#completed = identity.completed;
        this.sessionId = identity.session_id;
        this.canonicalSession = identity.canonical_session;
        this.screen = new Screen(this);
        this.keyboard = new Keyboard(this);
        this.output = new Output(this);
        this.pointer = new Pointer(this);
        this.semantic = new Semantic(this);
        this.events = new Events(this);
        this.native = new Native(this);
        this.evidence = new Evidence(this);
    }
    get completed(): boolean {
        return this.#completed;
    }

    async control(command: JsonObject, options: MutationOptions = {}): Promise<ControlResult> {
        const expectedState: JsonValue = options.ifState ?? null;
        try {
            const requestOptions: DriverRequestOptionsBuilder = {};
            if (options.requestId !== undefined) requestOptions.requestId = options.requestId;
            if (options.requestTimeoutMs !== undefined)
                requestOptions.timeoutMs = options.requestTimeoutMs;
            if (options.signal !== undefined) requestOptions.signal = options.signal;
            const result = await this.#driver.request(
                method({
                    method: "control",
                    handle: this.#handle,
                    command,
                    expected_presented_state_hash: expectedState,
                }),
                "control",
                requestOptions,
            );
            return result.data;
        } catch (cause: unknown) {
            if (!(cause instanceof TermtraceOperationError)) throw cause;
            const failure = cause.failure;
            if (failure.code === "stale_state") {
                throw new StaleStateError(
                    options.ifState ?? "",
                    failure.details?.current_presented_state_hash ?? "",
                    failure.retrySafety !== "unknown",
                );
            }
            if (failure.code === "action_busy") throw new ActionBusyError(failure.message);
            if (failure.code === "input_sequence_busy") throw new InputSequenceBusyError(failure.message);
            if (failure.code === "invalid_atom") throw new InvalidAtomError(failure.message);
            if (failure.code === "invalid_duration") throw new InvalidDurationError(failure.message);
            if (failure.code === "invalid_query") throw new InvalidQueryError(failure.message);
            if (failure.code === "invalid_screen_region") throw new InvalidRegionError(failure.message);
            if (failure.code === "protocol_mismatch") throw new ProtocolMismatchError(failure.message);
            if (
                failure.code === "input_sequence_incomplete" &&
                failure.details?.input_sequence !== undefined &&
                failure.details.input_sequence !== null
            ) {
                const details = failure.details.input_sequence;
                throw new PartialInputError(details, failure);
            }
            if (failure.code === "screen_frame_resource_limit") {
                const screen = failure.details?.screen_frame;
                if (
                    (screen?.limit_kind === "cell_count" ||
                        screen?.limit_kind === "serialized_bytes" ||
                        screen?.limit_kind === "response_bytes" ||
                        screen?.limit_kind === "match_count") &&
                    screen.requested !== undefined &&
                    screen.maximum !== undefined
                ) {
                    throw new ResourceLimitError(
                        screen.limit_kind,
                        exactWireInteger(screen.requested, "requested resource"),
                        exactWireInteger(screen.maximum, "maximum resource"),
                    );
                }
            }
            throw cause;
        }
    }
    async act(atoms: ReadonlyArray<InputAtom>, options: ActionOptions = {}): Promise<ActionResult> {
        const encoded = encodeAction(atoms, options);
        const data = await this.control(method(encoded.command), {
            ...options,
            requestId: encoded.requestId,
        });
        return parseActionResult(data, encoded.requestId, options.allowIncomplete === true);
    }

    request<Kind extends DriverDataKind>(
        methodFields: JsonObject,
        expectedKind: Kind,
        options: DriverRequestOptions = {},
    ): Promise<DriverDataOf<Kind>> {
        return this.#driver.request(
            method({ ...methodFields, handle: this.#handle }),
            expectedKind,
            options,
        );
    }

    async completion(): Promise<SessionCompletion> {
        const result = await this.#driver.request(
            method({ method: "completion", handle: this.#handle }),
            "completion",
        );
        this.#completed = true;
        return result.completion;
    }

    async complete(timeoutMs: number, requestTimeoutMs?: number): Promise<SessionCompletion> {
        const result = await this.#driver.request(
            method({ method: "complete", handle: this.#handle, timeout_ms: timeoutMs }),
            "completion",
            requestTimeoutMs === undefined ? {} : { timeoutMs: requestTimeoutMs },
        );
        this.#completed = true;
        return result.completion;
    }

    async waitForOutput(data: Uint8Array, timeoutMs: number): Promise<WaitOutcome> {
        const result = await this.#driver.request(
            method({
                method: "wait_output",
                handle: this.#handle,
                data: Buffer.from(data).toString("base64"),
                timeout_ms: timeoutMs,
            }),
            "output_wait",
        );
        return result.outcome;
    }

    stop(options: MutationOptions = {}): Promise<ControlResult> {
        return this.control({ command: "stop" }, options);
    }

    resize(
        viewport: {
            readonly cols: number;
            readonly rows: number;
            readonly pixelWidth?: number;
            readonly pixelHeight?: number;
        },
        options: MutationOptions = {},
    ): Promise<ControlResult> {
        return this.control(
            {
                command: "resize",
                cols: viewport.cols,
                rows: viewport.rows,
                pixel_width: viewport.pixelWidth ?? viewport.cols * 12,
                pixel_height: viewport.pixelHeight ?? viewport.rows * 24,
            },
            options,
        );
    }

    async close(): Promise<void> {
        if (this.#closed) {
            return;
        }
        if (this.#owned && this.#evidencePolicy !== "manual") {
            await this.#ensureCompleted();
        }
        await this.#driver.request(method({ method: "detach", handle: this.#handle }), "detached");
        this.#closed = true;
    }

    /** Complete owned work and apply the explicit success-sensitive retention policy. */
    async finish(passed: boolean): Promise<void> {
        if (this.#closed) return;
        if (!this.#owned)
            throw new TypeError("finish is valid only for sessions launched by this client");
        await this.#ensureCompleted();
        if (passed && this.#evidencePolicy === "on-failure") {
            await this.#driver.request(
                method({ method: "discard_completed", handle: this.#handle }),
                "discarded",
            );
            this.#closed = true;
            return;
        }
        await this.#driver.request(method({ method: "detach", handle: this.#handle }), "detached");
        this.#closed = true;
    }

    async #ensureCompleted(): Promise<void> {
        if (this.#completed) return;
        try {
            await this.stop();
        } catch (cause: unknown) {
            // A naturally completed process has no live actor; completion below is authoritative.
            if (!(cause instanceof Error)) throw cause;
        }
        await this.complete(10_000);
    }
    async [Symbol.asyncDispose](): Promise<void> {
        await this.close();
    }
}

export class Keyboard {
    constructor(private readonly session: ControlSession) {}

    press(key: string, options: MutationOptions = {}): Promise<ControlResult> {
        return this.session.control(
            {
                command: "key",
                key,
                modifiers: { shift: false, control: false, alt: false, super_key: false },
                caller_intent: options.callerIntent ?? null,
            },
            options,
        );
    }

    type(text: string, options: MutationOptions = {}): Promise<ControlResult> {
        return this.bytes(new TextEncoder().encode(text), { ...options, inputKind: "text" });
    }

    paste(text: string, options: MutationOptions = {}): Promise<ControlResult> {
        return this.session.control(
            {
                command: "paste",
                data: Buffer.from(text).toString("base64"),
                caller_intent: options.callerIntent ?? null,
            },
            options,
        );
    }

    bytes(
        bytes: Uint8Array,
        options: MutationOptions & { readonly inputKind?: "raw" | "text" } = {},
    ): Promise<ControlResult> {
        const data = Buffer.from(bytes).toString("base64");
        return this.session.control(
            {
                command: "input",
                data,
                input_kind: options.inputKind ?? "raw",
                caller_intent: options.callerIntent ?? null,
            },
            options,
        );
    }

    sequence(
        atoms: ReadonlyArray<KeyboardAtom>,
        options: InputSequenceOptions = {},
    ): Promise<ControlResult> {
        const requestId = options.requestId ?? randomUUID();
        const pace = Duration.milliseconds(options.paceMs ?? 0);
        if (pace.nanoseconds * BigInt(atoms.length) > 60_000_000_000n) {
            throw new InvalidDurationError("total input sequence pacing must not exceed 60 seconds");
        }
        const actionOptions: SequenceActionOptionsBuilder = { requestId, pace };
        if (options.callerIntent !== undefined) actionOptions.callerIntent = options.callerIntent;
        if (options.verbose !== undefined) actionOptions.verboseAtomOutcomes = options.verbose;
        const encoded = encodeAction(atoms, actionOptions);
        return this.session.control(
            method({
                command: "input_sequence",
                atoms: encoded.command.atoms,
                pace_ns: pace.toWireNumber("paceMs"),
                include_atom_outcomes: options.verbose ?? false,
            }),
            { ...options, requestId },
        );
    }
}

export class Pointer {
    constructor(private readonly session: ControlSession) {}

    click(
        column: number,
        row: number,
        button: "left" | "middle" | "right" = "left",
        options: MutationOptions & { readonly strict?: boolean } = {},
    ): Promise<ControlResult> {
        return this.session.control(
            {
                command: "pointer",
                column,
                row,
                operation: { kind: "click", button },
                strict: options.strict ?? false,
            },
            options,
        );
    }

    multiClick(
        column: number,
        row: number,
        count: 2 | 3 | 4,
        button: "left" | "middle" | "right" = "left",
        options: MutationOptions & { readonly strict?: boolean } = {},
    ): Promise<ControlResult> {
        return this.session.control(
            {
                command: "pointer",
                column,
                row,
                operation: { kind: "multi_click", button, count },
                strict: options.strict ?? false,
            },
            options,
        );
    }

    drag(
        fromColumn: number,
        fromRow: number,
        toColumn: number,
        toRow: number,
        options: MutationOptions & {
            readonly button?: "left" | "middle" | "right";
            readonly steps?: number;
            readonly strict?: boolean;
        } = {},
    ): Promise<ControlResult> {
        return this.session.control(
            {
                command: "pointer",
                column: fromColumn,
                row: fromRow,
                operation: {
                    kind: "drag",
                    button: options.button ?? "left",
                    to_column: toColumn,
                    to_row: toRow,
                    steps: options.steps ?? 8,
                },
                strict: options.strict ?? false,
            },
            options,
        );
    }

    scroll(
        column: number,
        row: number,
        lines: number,
        options: MutationOptions & { readonly strict?: boolean } = {},
    ): Promise<ControlResult> {
        return this.session.control(
            {
                command: "pointer",
                column,
                row,
                operation: { kind: "scroll", lines },
                strict: options.strict ?? false,
            },
            options,
        );
    }

    focus(
        focused: boolean,
        options: MutationOptions & { readonly strict?: boolean } = {},
    ): Promise<ControlResult> {
        return this.session.control(
            { command: "focus", focused, strict: options.strict ?? false },
            options,
        );
    }
}

export class Semantic {
    constructor(private readonly session: Session) {}

    async inspect(
        point: StatePoint = { selector: "latest_presented" },
        includeHidden = false,
    ): Promise<Observation> {
        const result = await this.session.request(
            { method: "observe", point, include_hidden: includeHidden },
            "observation",
        );
        return result.observation;
    }

    async timeline(
        options: {
            readonly allStates?: boolean;
            readonly controls?: boolean;
            readonly offset?: number;
            readonly limit?: number;
        } = {},
    ): Promise<TimelinePage> {
        return this.session.request(
            {
                method: "timeline",
                all_states: options.allStates ?? false,
                controls: options.controls ?? false,
                offset: options.offset ?? 0,
                limit: options.limit ?? 256,
            },
            "timeline",
        );
    }

    async diff(from: StatePoint, to: StatePoint): Promise<SemanticDiff> {
        const result = await this.session.request({ method: "diff", from, to }, "diff");
        return result.difference;
    }

    async diagnose(
        checks: ReadonlyArray<string> = [],
        broadChangePercent = 50,
    ): Promise<DiagnoseReport> {
        const result = await this.session.request(
            {
                method: "diagnose",
                checks,
                broad_change_percent: broadChangePercent,
            },
            "diagnose",
        );
        return result.report;
    }
}

export class Events {
    constructor(private readonly session: Session) {}

    page(
        options: {
            readonly fromEventSeq?: number;
            readonly limit?: number;
            readonly metadataOnly?: boolean;
        } = {},
    ): Promise<EventPage> {
        return this.session.request(
            {
                method: "events",
                from_event_seq: options.fromEventSeq ?? 0,
                limit: options.limit ?? 256,
                metadata_only: options.metadataOnly ?? false,
            },
            "events",
        );
    }
}

export class Native {
    constructor(private readonly session: Session) {}

    screenshot(
        destination: string,
        options: {
            readonly point?: StatePoint;
            readonly viewport?: { readonly width: number; readonly height: number };
        } = {},
    ): Promise<Artifact> {
        return this.session.request(
            {
                method: "screenshot",
                point: options.point ?? { selector: "latest_presented" },
                destination,
                renderer: "native",
                viewport: options.viewport ?? null,
            },
            "artifact",
        );
    }
}

export class Evidence {
    constructor(private readonly session: Session) {}

    completion(): Promise<SessionCompletion> {
        return this.session.completion();
    }

    complete(
        options: Pick<WaitOptions, "timeoutMs"> & { readonly requestTimeoutMs?: number } = {},
    ): Promise<SessionCompletion> {
        return this.session.complete(
            nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs"),
            options.requestTimeoutMs,
        );
    }

    async verify(
        level: "structure" | "content" | "durability_record" = "content",
        requestTimeoutMs?: number,
    ): Promise<VerificationReport> {
        const result = await this.session.request(
            { method: "verify", level },
            "verification",
            requestTimeoutMs === undefined ? {} : { timeoutMs: requestTimeoutMs },
        );
        return result.report;
    }

    bundle(
        destination: string,
        profile: "screen" | "debug" | "forensic" = "screen",
        requestTimeoutMs?: number,
    ): Promise<BundleResult> {
        return this.session.request(
            { method: "bundle", destination, profile },
            "bundle",
            requestTimeoutMs === undefined ? {} : { timeoutMs: requestTimeoutMs },
        );
    }
}

export class Output {
    constructor(private readonly session: Session) {}

    waitForBytes(
        data: Uint8Array,
        options: Pick<WaitOptions, "timeoutMs"> = {},
    ): Promise<WaitOutcome> {
        return this.session.waitForOutput(
            data,
            nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs"),
        );
    }

    waitForText(text: string, options: Pick<WaitOptions, "timeoutMs"> = {}): Promise<WaitOutcome> {
        return this.waitForBytes(new TextEncoder().encode(text), options);
    }
}
