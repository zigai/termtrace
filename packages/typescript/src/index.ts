import { randomUUID } from "node:crypto";

import { DriverClient, method, type DriverRequestOptions } from "./driver-client.js";
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
    StableCaptureIncompleteError,
    StaleStateError,
} from "./errors.js";
import { Locator, locatorByText, locatorFor } from "./locators.js";
import type { LocatorSpec, StyleFilter, TextQuery } from "./queries.js";
import {
    encodeFrameRequest,
    parseScreenFrame,
    type FrameRequest,
    type Region,
    type StatePoint,
    type ScreenFrame,
} from "./screen-frame.js";
import {
    TermtraceOperationError,
    TermtraceProtocolError,
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
    type ScreenPredicate,
    type SemanticDiff,
    type SessionCompletion,
    type TimelinePage,
    type VerificationReport,
    type WaitOutcome,
} from "./protocol.js";

export {
    TermtraceOperationError,
    TermtraceProtocolError,
    type Artifact,
    type BundleResult,
    type ControlResult,
    type ControlErrorDetails,
    type DiagnoseReport,
    type DriverFailure,
    type EventPage,
    type JsonObject,
    type JsonValue,
    type InputSequenceFailureDetails,
    type Observation,
    type ObservationFrame,
    type ScreenPredicate,
    type SemanticDiff,
    type SessionCompletion,
    type TimelineEntry,
    type TimelinePage,
    type VerificationReport,
    type WaitOutcome,
} from "./protocol.js";
export {
    Duration,
    type ActionBoundaries,
    type ActionCompletion,
    type ActionInputResult,
    type ActionOptions,
    type ActionPostcondition,
    type CanonicalBoundary,
    type ActionResult,
    type InputAtom,
    type KeyInput,
    type InputAtomOutcome,
} from "./actions.js";
export * from "./errors.js";
export {
    allOf,
    evaluateQuery,
    matchHasStyle,
    regionQuery,
    rowQuery,
    styleQuery,
    textQuery,
    type CellRange,
    type LocatorSpec,
    type ScreenMatch,
    type StyleFilter,
    type TextQuery,
} from "./queries.js";
export {
    ScreenFrame,
    encodeFrameRequest,
    normalizeRegion,
    type CellStyle,
    type ColorSource,
    type ColorState,
    type CursorState,
    type FrameDetail,
    type FrameRequest,
    type ModeState,
    type Region,
    type Rgb,
    type ScreenAppearance,
    type ScreenCell,
    type ScreenFrameSource,
    type ScreenGeometry,
    type StatePoint,
    type ScreenRow,
    type ScreenSpan,
    type SpanCell,
    type WideKind,
} from "./screen-frame.js";
export { Locator, type LocatorWaitOptions, type LocatorWaitResult } from "./locators.js";
export {
    normalizeFrame,
    type NormalizedFrame,
    type SnapshotOptions,
    type SnapshotProfile,
} from "./snapshots.js";
export {
    AssertionMismatchError,
    assertMatches,
    assertMatchStyle,
    assertMatchText,
    assertNeutralRegion,
    assertNoMatches,
    assertNoWrappedRows,
    assertCursor,
    assertNoPresentedTransient,
    assertScreenMatch,
    assertScreenText,
    assertTerminalSnapshot,
    stableTerminalSnapshot,
    termtraceMatchers,
    type TerminalSnapshot,
} from "./assertions.js";

export type TermtraceOptions = {
    readonly binaryPath?: string;
    readonly evidenceRoot?: string;
};

export type LaunchOptions = {
    readonly command: ReadonlyArray<string>;
    readonly evidenceRoot?: string;
    readonly viewport?: { readonly cols: number; readonly rows: number };
    readonly workingDirectory?: string;
    readonly evidence?: "always" | "on-failure" | "manual";
    readonly environment?: EnvironmentOptions;
    readonly outputByteBudget?: number;
};

export type EnvironmentOptions = {
    readonly inheritEnv?: boolean;
    readonly set?: Readonly<Record<string, string | Uint8Array>>;
    readonly copy?: ReadonlyArray<string>;
    readonly unset?: ReadonlyArray<string>;
    readonly color?: "auto" | "always" | "never";
};

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

/** Context-managed entry point for attached canonical sessions. */
export class Termtrace implements AsyncDisposable {
    readonly #driver: DriverClient;
    readonly #evidenceRoot: string | undefined;

    private constructor(driver: DriverClient, evidenceRoot: string | undefined) {
        this.#driver = driver;
        this.#evidenceRoot = evidenceRoot;
    }

    static async create(options: TermtraceOptions = {}): Promise<Termtrace> {
        const driver = new DriverClient(
            options.binaryPath ?? process.env.TERMTRACE_BINARY ?? "termtrace",
        );
        await driver.request(method({ method: "hello" }), "hello");
        return new Termtrace(driver, options.evidenceRoot);
    }

    async launch(options: LaunchOptions): Promise<Session> {
        const evidenceRoot = options.evidenceRoot ?? this.#evidenceRoot;
        if (evidenceRoot === undefined) {
            throw new TypeError("launch requires evidenceRoot on Termtrace or this call");
        }
        const viewport = options.viewport ?? { cols: 160, rows: 45 };
        const command = options.command.map((argument) => Buffer.from(argument).toString("base64"));
        const data = await this.#driver.request(
            method({
                method: "launch",
                evidence_root: evidenceRoot,
                command,
                cols: viewport.cols,
                rows: viewport.rows,
                working_directory: options.workingDirectory ?? null,
                environment: encodeEnvironment(options.environment),
                resource_limits: {
                    output_bytes: options.outputByteBudget ?? null,
                },
            }),
            "attached",
        );
        return new Session(this.#driver, data, true, options.evidence ?? "always");
    }

    async attach(session: string): Promise<Session> {
        const data = await this.#driver.request(method({ method: "attach", session }), "attached");
        return new Session(this.#driver, data, false, "manual");
    }

    async close(): Promise<void> {
        await this.#driver.close();
    }

    async [Symbol.asyncDispose](): Promise<void> {
        await this.close();
    }
}

type SessionIdentity = DriverDataOf<"attached">;

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
            const result = await this.#driver.request(
                method({
                    method: "control",
                    handle: this.#handle,
                    command,
                    expected_presented_state_hash: expectedState,
                }),
                "control",
                {
                    ...(options.requestId === undefined ? {} : { requestId: options.requestId }),
                    ...(options.requestTimeoutMs === undefined
                        ? {}
                        : { timeoutMs: options.requestTimeoutMs }),
                    ...(options.signal === undefined ? {} : { signal: options.signal }),
                },
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
            if (failure.code === "input_sequence_busy")
                throw new InputSequenceBusyError(failure.message);
            if (failure.code === "invalid_atom") throw new InvalidAtomError(failure.message);
            if (failure.code === "invalid_duration")
                throw new InvalidDurationError(failure.message);
            if (failure.code === "invalid_query") throw new InvalidQueryError(failure.message);
            if (failure.code === "invalid_screen_region")
                throw new InvalidRegionError(failure.message);
            if (failure.code === "protocol_mismatch")
                throw new ProtocolMismatchError(failure.message);
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

export type ScreenWait = {
    readonly predicate: ScreenPredicate;
    readonly matched: boolean;
    readonly firstMatchAt: Duration | null;
    readonly observedAt: Duration;
    readonly stableFor: Duration;
    readonly quality:
        | "matched"
        | "matched_unsettled"
        | "child_completed"
        | "output_closed"
        | "deadline";
    readonly frame: ScreenFrame;
};

export type StableOptions = {
    readonly stableFor: Duration;
    readonly timeout: Duration;
    readonly frame?: FrameRequest;
    readonly allowIncomplete?: boolean;
};

export type StableCapture = {
    readonly kind: "stable_snapshot";
    readonly quality: "stable" | "child_completed" | "output_closed" | "deadline";
    readonly firstObservedAt: Duration;
    readonly lastObservedAt: Duration;
    readonly stableFor: Duration;
    readonly frame: ScreenFrame;
};

export class Screen {
    constructor(private readonly session: Session) {}

    snapshot(): Promise<ControlResult> {
        return this.session.control({ command: "snapshot" });
    }

    /** Preserve the compact observation text endpoint. */
    async text(): Promise<string> {
        const data = await this.session.request(
            { method: "observe", point: { selector: "latest_presented" }, include_hidden: false },
            "observation",
        );
        return data.observation.frame.viewport_text;
    }

    async frame(
        request: FrameRequest = {},
        options: { readonly signal?: AbortSignal } = {},
    ): Promise<ScreenFrame> {
        const data = await this.session.request(
            {
                method: "screen_frame",
                point: request.point ?? { selector: "latest_presented" },
                include_hidden: false,
                projection: encodeFrameRequest(request),
            },
            "screen_frame",
            options,
        );
        return parseScreenFrame(data.frame);
    }

    cells(region: Region): Promise<ScreenFrame> {
        return this.frame({ detail: "cells", region });
    }

    locator(spec: LocatorSpec): Locator {
        return locatorFor((request, options) => this.frame(request, options), spec);
    }

    getByText(
        query: TextQuery,
        options: { readonly exact?: boolean; readonly crossSoftWrap?: boolean } = {},
    ): Locator {
        return locatorByText(
            (request, captureOptions) => this.frame(request, captureOptions),
            query,
            options,
        );
    }

    row(row: number): Locator {
        return this.locator({ kind: "row", row });
    }

    region(region: Region): Locator {
        return this.locator({ kind: "region", region });
    }

    filter(style: StyleFilter): Locator {
        return this.locator({ kind: "style", style });
    }

    async waitForText(expected: string | RegExp, options: WaitOptions = {}): Promise<ScreenFrame> {
        const result = await this.getByText(expected).wait({
            timeout: Duration.milliseconds(options.timeoutMs ?? 10_000),
            interval: Duration.milliseconds(25),
            stableFor: Duration.milliseconds(options.stableForMs ?? 0),
            ...(options.signal === undefined ? {} : { signal: options.signal }),
        });
        return result.frame;
    }

    async waitForPredicate(
        predicate: ScreenPredicate,
        options: WaitOptions = {},
    ): Promise<ScreenWait> {
        const timeoutMs = nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs");
        const stableForMs = nonnegativeDuration(options.stableForMs ?? 0, "stableForMs");
        const data = await this.session.control(
            method({
                command: "wait_screen",
                predicate,
                stable_for_ns: millisecondsToNanoseconds(stableForMs, "stableForMs"),
                timeout_ns: millisecondsToNanoseconds(timeoutMs, "timeoutMs"),
                projection: encodeFrameRequest(),
            }),
        );
        if (data.kind !== "screen_wait")
            throw new TermtraceProtocolError("screen wait response has an invalid kind");
        const observed = exactWireInteger(data.observed_monotonic_ns, "screen wait observed time");
        const stableFor = exactWireInteger(data.stable_for_ns, "screen wait stable interval");
        const first =
            data.first_match_monotonic_ns === null || data.first_match_monotonic_ns === undefined
                ? null
                : Duration.nanoseconds(
                      BigInt(
                          exactWireInteger(
                              data.first_match_monotonic_ns,
                              "screen wait first match time",
                          ),
                      ),
                  );
        return Object.freeze({
            predicate,
            matched: data.matched,
            firstMatchAt: first,
            observedAt: Duration.nanoseconds(BigInt(observed)),
            stableFor: Duration.nanoseconds(BigInt(stableFor)),
            quality: data.quality,
            frame: parseScreenFrame(data.frame),
        });
    }

    waitForAbsence(text: string, options: WaitOptions = {}): Promise<ScreenWait> {
        return this.waitForPredicate({ predicate: "absent", text }, options);
    }

    async captureStable(options: StableOptions): Promise<StableCapture> {
        if (
            options.stableFor.nanoseconds === 0n ||
            options.timeout.nanoseconds === 0n ||
            options.stableFor.nanoseconds > options.timeout.nanoseconds
        ) {
            throw new InvalidDurationError("stableFor must be nonzero and no greater than timeout");
        }
        if (this.session.completed) {
            return Object.freeze({
                kind: "stable_snapshot",
                quality: "stable",
                firstObservedAt: Duration.nanoseconds(0n),
                lastObservedAt: Duration.nanoseconds(0n),
                stableFor: options.stableFor,
                frame: await this.frame(options.frame ?? {}),
            });
        }
        const data = await this.session.control(
            method({
                command: "capture_stable",
                stable_for_ns: options.stableFor.toWireNumber("stableFor"),
                timeout_ns: options.timeout.toWireNumber("timeout"),
                projection: encodeFrameRequest(options.frame),
            }),
        );
        if (data.kind !== "stable_snapshot")
            throw new TermtraceProtocolError("stable capture response has an invalid kind");
        const first = exactWireInteger(
            data.first_observed_monotonic_ns,
            "stable capture first observed time",
        );
        const last = exactWireInteger(data.observed_monotonic_ns, "stable capture observed time");
        const stable = exactWireInteger(data.stable_for_ns, "stable capture stable interval");
        const capture: StableCapture = Object.freeze({
            kind: data.kind,
            quality: data.quality,
            firstObservedAt: Duration.nanoseconds(BigInt(first)),
            lastObservedAt: Duration.nanoseconds(BigInt(last)),
            stableFor: Duration.nanoseconds(BigInt(stable)),
            frame: parseScreenFrame(data.frame),
        });
        if (data.quality !== "stable" && options.allowIncomplete !== true)
            throw new StableCaptureIncompleteError(capture, data.quality);
        return capture;
    }

    waitForIdle(options: IdleWaitOptions = {}): Promise<StableCapture> {
        return this.captureStable({
            stableFor: Duration.milliseconds(options.stableForMs ?? 100),
            timeout: Duration.milliseconds(options.timeoutMs ?? 10_000),
            ...(options.allowIncomplete === undefined
                ? {}
                : { allowIncomplete: options.allowIncomplete }),
        });
    }

    screenshot(
        destination: string,
        point: StatePoint = { selector: "latest_presented" },
        requestTimeoutMs?: number,
    ): Promise<Artifact> {
        return this.session.request(
            {
                method: "screenshot",
                point,
                destination,
                renderer: "semantic",
                viewport: null,
            },
            "artifact",
            requestTimeoutMs === undefined ? {} : { timeoutMs: requestTimeoutMs },
        );
    }
}

export class Keyboard {
    constructor(private readonly session: Session) {}

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
            throw new InvalidDurationError(
                "total input sequence pacing must not exceed 60 seconds",
            );
        }
        const encoded = encodeAction(atoms, {
            requestId,
            pace,
            ...(options.callerIntent === undefined ? {} : { callerIntent: options.callerIntent }),
            ...(options.verbose === undefined ? {} : { verboseAtomOutcomes: options.verbose }),
        });
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
    constructor(private readonly session: Session) {}

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

function encodeEnvironment(options: EnvironmentOptions | undefined): JsonObject {
    const set: Record<string, JsonValue> = {};
    for (const [name, value] of Object.entries(options?.set ?? {})) {
        const bytes = Buffer.from(value);
        set[name] = bytes.toString("base64");
    }
    return {
        base: options?.inheritEnv === false ? "clear" : "inherit",
        set,
        copy: [...(options?.copy ?? [])],
        unset: [...(options?.unset ?? [])],
        color: options?.color ?? "auto",
    };
}

function exactWireInteger(value: unknown, field: string): number {
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0)
        throw new TermtraceProtocolError(`${field} must be a nonnegative safe integer`);
    return value;
}

function nonnegativeDuration(value: number, field: string): number {
    if (!Number.isFinite(value) || value < 0 || !Number.isSafeInteger(value)) {
        throw new TypeError(`${field} must be a nonnegative integer`);
    }
    return value;
}

function millisecondsToNanoseconds(milliseconds: number, field: string): number {
    const nanoseconds = milliseconds * 1_000_000;
    if (!Number.isSafeInteger(nanoseconds)) {
        throw new TypeError(`${field} exceeds the exactly representable protocol maximum`);
    }
    return nanoseconds;
}
