import { DriverClient, method } from "./driver-client.js";
import { randomUUID } from "node:crypto";
import {
  TermtraceProtocolError,
  type Artifact,
  type BundleResult,
  type ControlResult,
  type ControlErrorDetails,
  type DiagnoseReport,
  type DriverDataKind,
  type DriverDataOf,
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
  type VerificationReport,
  type WaitOutcome,
} from "./protocol.js";
export {
  assertCursor,
  assertNoPresentedTransient,
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
  readonly ifState?: string;
};

export type KeyboardAtom =
  | { readonly kind: "text"; readonly text: string; readonly intent?: string }
  | {
      readonly kind: "bytes";
      readonly bytes: Uint8Array;
      readonly inputKind?: "raw" | "text";
      readonly intent?: string;
    }
  | {
      readonly kind: "key";
      readonly key: string;
      readonly modifiers?: {
        readonly shift?: boolean;
        readonly control?: boolean;
        readonly alt?: boolean;
        readonly superKey?: boolean;
      };
    }
  | { readonly kind: "paste"; readonly data: string | Uint8Array };

export type InputSequenceOptions = MutationOptions & {
  readonly paceMs?: number;
  readonly verbose?: boolean;
};

export type WaitOptions = {
  readonly timeoutMs?: number;
  readonly stableForMs?: number;
  readonly signal?: AbortSignal;
};

export type StatePoint =
  | { readonly selector: "latest_presented" }
  | { readonly selector: "state_id"; readonly state_id: number }
  | { readonly selector: "event_seq"; readonly event_seq: number }
  | { readonly selector: "monotonic_ns"; readonly monotonic_ns: number }
  | { readonly selector: "marker"; readonly label: string };

/** Context-managed entry point for attached canonical sessions. */
export class Termtrace implements AsyncDisposable {
  readonly #driver: DriverClient;
  readonly #evidenceRoot: string | undefined;

  private constructor(driver: DriverClient, evidenceRoot: string | undefined) {
    this.#driver = driver;
    this.#evidenceRoot = evidenceRoot;
  }

  static async create(options: TermtraceOptions = {}): Promise<Termtrace> {
    const driver = new DriverClient(options.binaryPath ?? process.env.TERMTRACE_BINARY ?? "termtrace");
    await driver.request(method({ method: "hello" }), "hello");
    return new Termtrace(driver, options.evidenceRoot);
  }

  async launch(options: LaunchOptions): Promise<Session> {
    const evidenceRoot = options.evidenceRoot ?? this.#evidenceRoot;
    if (evidenceRoot === undefined) {
      throw new TypeError("launch requires evidenceRoot on Termtrace or this call");
    }
    const viewport = options.viewport ?? { cols: 80, rows: 24 };
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

  constructor(driver: DriverClient, identity: SessionIdentity, owned: boolean, evidencePolicy: "always" | "on-failure" | "manual") {
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

  async control(command: JsonObject, options: MutationOptions = {}): Promise<ControlResult> {
    const expectedState: JsonValue = options.ifState ?? null;
    const result = await this.#driver.request(
      method({
        method: "control",
        handle: this.#handle,
        command,
        expected_presented_state_hash: expectedState,
      }),
      "control",
      options.requestId,
    );
    return result.data;
  }

  request<Kind extends DriverDataKind>(
    methodFields: JsonObject,
    expectedKind: Kind,
    requestId?: string,
  ): Promise<DriverDataOf<Kind>> {
    return this.#driver.request(
      method({ ...methodFields, handle: this.#handle }),
      expectedKind,
      requestId,
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

  async complete(timeoutMs: number): Promise<SessionCompletion> {
    const result = await this.#driver.request(
      method({ method: "complete", handle: this.#handle, timeout_ms: timeoutMs }),
      "completion",
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

  resize(viewport: { readonly cols: number; readonly rows: number; readonly pixelWidth?: number; readonly pixelHeight?: number }, options: MutationOptions = {}): Promise<ControlResult> {
    return this.control({
      command: "resize",
      cols: viewport.cols,
      rows: viewport.rows,
      pixel_width: viewport.pixelWidth ?? viewport.cols * 8,
      pixel_height: viewport.pixelHeight ?? viewport.rows * 16,
    }, options);
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
    if (!this.#owned) throw new TypeError("finish is valid only for sessions launched by this client");
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

export class Screen {
  constructor(private readonly session: Session) {}

  snapshot(): Promise<ControlResult> {
    return this.session.control({ command: "snapshot" });
  }

  async text(): Promise<string> {
    return (await this.frame()).viewport_text;
  }

  async frame(point: StatePoint = { selector: "latest_presented" }): Promise<ObservationFrame> {
    const data = await this.session.request(
      { method: "observe", point, include_hidden: false },
      "observation",
    );
    return data.observation.frame;
  }

  async waitForText(expected: string | RegExp, options: WaitOptions = {}): Promise<ControlResult> {
    const timeoutMs = nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs");
    const stableForMs = nonnegativeDuration(options.stableForMs ?? 0, "stableForMs");
    millisecondsToNanoseconds(timeoutMs, "timeoutMs");
    millisecondsToNanoseconds(stableForMs, "stableForMs");
    const predicate: ScreenPredicate = typeof expected === "string"
      ? { predicate: "contains", text: expected }
      : { predicate: "matches_regex", pattern: rustRegexSource(expected) };
    options.signal?.throwIfAborted();
    if (options.signal === undefined) {
      return this.waitForPredicate(predicate, options);
    }

    const started = performance.now();
    let stableSince: number | undefined;
    for (;;) {
      options.signal?.throwIfAborted();
      const text = await this.text();
      const matches = typeof expected === "string" ? text.includes(expected) : regexMatches(expected, text);
      const now = performance.now();
      stableSince = matches ? (stableSince ?? now) : undefined;
      if (stableSince !== undefined && now - stableSince >= stableForMs) {
        return this.waitForPredicate(predicate, {
          timeoutMs: Math.max(1, Math.floor(timeoutMs - (now - started))),
        });
      }
      if (now - started >= timeoutMs) {
        throw new TermtraceProtocolError("screen wait timed out");
      }
      await delay(Math.min(25, Math.max(1, timeoutMs - (now - started))), options.signal);
    }
  }

  waitForPredicate(predicate: ScreenPredicate, options: WaitOptions = {}): Promise<ControlResult> {
    if (options.signal !== undefined) {
      throw new TypeError("signal cancellation is available only for text and RegExp polling waits");
    }
    const timeoutMs = nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs");
    const stableForMs = nonnegativeDuration(options.stableForMs ?? 0, "stableForMs");
    return this.session.control(method({
      command: "wait_screen",
      predicate,
      stable_for_ns: millisecondsToNanoseconds(stableForMs, "stableForMs"),
      timeout_ns: millisecondsToNanoseconds(timeoutMs, "timeoutMs"),
    }));
  }

  waitForAbsence(text: string, options: WaitOptions = {}): Promise<ControlResult> {
    return this.waitForPredicate({ predicate: "absent", text }, options);
  }

  async waitForIdle(options: IdleWaitOptions = {}): Promise<ControlResult> {
    if (options.signal !== undefined) {
      throw new TypeError("signal cancellation is not supported for actor-atomic idle waits");
    }
    const timeoutMs = nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs");
    const stableForMs = nonnegativeDuration(options.stableForMs ?? 100, "stableForMs");
    const data = await this.session.control({
      command: "capture_stable",
      stable_for_ns: millisecondsToNanoseconds(stableForMs, "stableForMs"),
      timeout_ns: millisecondsToNanoseconds(timeoutMs, "timeoutMs"),
    });
    if (data.kind !== "stable_snapshot" || typeof data.quality !== "string") {
      throw new TermtraceProtocolError("stable capture response has an invalid shape");
    }
    if (data.quality !== "stable" && options.allowIncomplete !== true) {
      throw new TermtraceProtocolError(
        `stable capture ended with ${data.quality}; set allowIncomplete to inspect it`,
      );
    }
    return data;
  }

  screenshot(destination: string, point: StatePoint = { selector: "latest_presented" }): Promise<Artifact> {
    return this.session.request({
      method: "screenshot",
      point,
      destination,
      renderer: "semantic",
      viewport: null,
    }, "artifact");
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
      },
      options,
    );
  }

  type(text: string, options: MutationOptions = {}): Promise<ControlResult> {
    return this.bytes(new TextEncoder().encode(text), { ...options, inputKind: "text" });
  }

  paste(text: string, options: MutationOptions = {}): Promise<ControlResult> {
    return this.session.control(
      { command: "paste", data: Buffer.from(text).toString("base64") },
      options,
    );
  }

  bytes(bytes: Uint8Array, options: MutationOptions & { readonly inputKind?: "raw" | "text" } = {}): Promise<ControlResult> {
    const data = Buffer.from(bytes).toString("base64");
    return this.session.control(
      { command: "input", data, input_kind: options.inputKind ?? "raw", intent: null },
      options,
    );
  }

  sequence(atoms: ReadonlyArray<KeyboardAtom>, options: InputSequenceOptions = {}): Promise<ControlResult> {
    if (atoms.length < 1 || atoms.length > 4096) {
      throw new RangeError("input sequence must contain 1 through 4096 atoms");
    }
    const paceMs = options.paceMs ?? 0;
    if (!Number.isSafeInteger(paceMs) || paceMs < 0 || paceMs > 60_000) {
      throw new RangeError("paceMs must be an integer between 0 and 60000");
    }
    if (paceMs * Math.max(atoms.length - 1, 0) > 60_000) {
      throw new RangeError("total input sequence pacing must not exceed 60000 ms");
    }
    const requestId = options.requestId ?? randomUUID();
    const encoded = atoms.map((atom, index): JsonObject => {
      const operationId = sequenceOperationId(requestId, index);
      switch (atom.kind) {
        case "text":
          return {
            input: "bytes",
            operation_id: operationId,
            data: Buffer.from(atom.text).toString("base64"),
            input_kind: "text",
            intent: atom.intent ?? null,
          };
        case "bytes":
          return {
            input: "bytes",
            operation_id: operationId,
            data: Buffer.from(atom.bytes).toString("base64"),
            input_kind: atom.inputKind ?? "raw",
            intent: atom.intent ?? null,
          };
        case "key":
          return {
            input: "key",
            operation_id: operationId,
            key: atom.key,
            modifiers: {
              shift: atom.modifiers?.shift ?? false,
              control: atom.modifiers?.control ?? false,
              alt: atom.modifiers?.alt ?? false,
              super_key: atom.modifiers?.superKey ?? false,
            },
          };
        case "paste":
          return {
            input: "paste",
            operation_id: operationId,
            data: Buffer.from(atom.data).toString("base64"),
          };
      }
    });
    return this.session.control(
      {
        command: "input_sequence",
        atoms: encoded,
        pace_ns: paceMs * 1_000_000,
        include_atom_outcomes: options.verbose ?? false,
      },
      { ...options, requestId },
    );
  }
}

function sequenceOperationId(sequenceId: string, index: number): string {
  const match = /^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-)([0-9a-f]{12})$/iu.exec(sequenceId);
  if (match === null) throw new TypeError("requestId must be a UUID");
  const prefix = match[1];
  const tail = match[2];
  if (prefix === undefined || tail === undefined) throw new TypeError("requestId must be a UUID");
  const derived = (BigInt(`0x${tail}`) ^ BigInt(index + 1)).toString(16).padStart(12, "0");
  return `${prefix}${derived}`;
}

export class Pointer {
  constructor(private readonly session: Session) {}

  click(column: number, row: number, button: "left" | "middle" | "right" = "left", options: MutationOptions & { readonly strict?: boolean } = {}): Promise<ControlResult> {
    return this.session.control({
      command: "pointer",
      column,
      row,
      operation: { kind: "click", button },
      strict: options.strict ?? false,
    }, options);
  }

  multiClick(column: number, row: number, count: 2 | 3 | 4, button: "left" | "middle" | "right" = "left", options: MutationOptions & { readonly strict?: boolean } = {}): Promise<ControlResult> {
    return this.session.control({
      command: "pointer",
      column,
      row,
      operation: { kind: "multi_click", button, count },
      strict: options.strict ?? false,
    }, options);
  }

  drag(fromColumn: number, fromRow: number, toColumn: number, toRow: number, options: MutationOptions & { readonly button?: "left" | "middle" | "right"; readonly steps?: number; readonly strict?: boolean } = {}): Promise<ControlResult> {
    return this.session.control({
      command: "pointer",
      column: fromColumn,
      row: fromRow,
      operation: { kind: "drag", button: options.button ?? "left", to_column: toColumn, to_row: toRow, steps: options.steps ?? 8 },
      strict: options.strict ?? false,
    }, options);
  }

  scroll(column: number, row: number, lines: number, options: MutationOptions & { readonly strict?: boolean } = {}): Promise<ControlResult> {
    return this.session.control({
      command: "pointer",
      column,
      row,
      operation: { kind: "scroll", lines },
      strict: options.strict ?? false,
    }, options);
  }

  focus(focused: boolean, options: MutationOptions & { readonly strict?: boolean } = {}): Promise<ControlResult> {
    return this.session.control({ command: "focus", focused, strict: options.strict ?? false }, options);
  }
}

export class Semantic {
  constructor(private readonly session: Session) {}

  async inspect(point: StatePoint = { selector: "latest_presented" }, includeHidden = false): Promise<Observation> {
    const result = await this.session.request(
      { method: "observe", point, include_hidden: includeHidden },
      "observation",
    );
    return result.observation;
  }

  async timeline(options: { readonly allStates?: boolean; readonly controls?: boolean } = {}): Promise<TimelineEntry[]> {
    const result = await this.session.request({
      method: "timeline",
      all_states: options.allStates ?? false,
      controls: options.controls ?? false,
    }, "timeline");
    return result.entries;
  }

  async diff(from: StatePoint, to: StatePoint): Promise<SemanticDiff> {
    const result = await this.session.request({ method: "diff", from, to }, "diff");
    return result.difference;
  }

  async diagnose(checks: ReadonlyArray<string> = [], broadChangePercent = 50): Promise<DiagnoseReport> {
    const result = await this.session.request({
      method: "diagnose",
      checks,
      broad_change_percent: broadChangePercent,
    }, "diagnose");
    return result.report;
  }
}

export class Events {
  constructor(private readonly session: Session) {}

  page(options: { readonly fromEventSeq?: number; readonly limit?: number; readonly metadataOnly?: boolean } = {}): Promise<EventPage> {
    return this.session.request({
      method: "events",
      from_event_seq: options.fromEventSeq ?? 0,
      limit: options.limit ?? 256,
      metadata_only: options.metadataOnly ?? false,
    }, "events");
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
    return this.session.request({
      method: "screenshot",
      point: options.point ?? { selector: "latest_presented" },
      destination,
      renderer: "native",
      viewport: options.viewport ?? null,
    }, "artifact");
  }
}

export class Evidence {
  constructor(private readonly session: Session) {}

  completion(): Promise<SessionCompletion> {
    return this.session.completion();
  }

  complete(options: Pick<WaitOptions, "timeoutMs"> = {}): Promise<SessionCompletion> {
    return this.session.complete(nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs"));
  }

  async verify(level: "structure" | "content" | "durability_record" = "content"): Promise<VerificationReport> {
    const result = await this.session.request({ method: "verify", level }, "verification");
    return result.report;
  }

  bundle(destination: string, profile: "screen" | "debug" | "forensic" = "screen"): Promise<BundleResult> {
    return this.session.request({ method: "bundle", destination, profile }, "bundle");
  }
}

export class Output {
  constructor(private readonly session: Session) {}

  waitForBytes(data: Uint8Array, options: Pick<WaitOptions, "timeoutMs"> = {}): Promise<WaitOutcome> {
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
    const bytes = typeof value === "string" ? Buffer.from(value) : Buffer.from(value);
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

function regexMatches(pattern: RegExp, text: string): boolean {
  pattern.lastIndex = 0;
  return pattern.test(text);
}

function rustRegexSource(pattern: RegExp): string {
  const unsupported = [...pattern.flags].filter((flag) => !["i", "m", "s", "u"].includes(flag));
  if (unsupported.length > 0) {
    throw new TypeError(`regular expression flags are unsupported by screen waits: ${unsupported.join("")}`);
  }
  const flags = [...pattern.flags].filter((flag) => flag !== "u").join("");
  return flags.length === 0 ? pattern.source : `(?${flags}:${pattern.source})`;
}

function delay(milliseconds: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    const finish = (): void => {
      signal?.removeEventListener("abort", abort);
      resolve();
    };
    const timer = setTimeout(finish, milliseconds);
    if (signal === undefined) {
      return;
    }
    const abort = (): void => {
      clearTimeout(timer);
      reject(signal.reason);
    };
    signal.addEventListener("abort", abort, { once: true });
  });
}
