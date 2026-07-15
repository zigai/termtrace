import { DriverClient, method, parseAttachData } from "./driver-client.js";
import { TermtraceProtocolError, type JsonObject, type JsonValue } from "./protocol.js";

export {
  TermtraceOperationError,
  TermtraceProtocolError,
  type DriverFailure,
  type JsonObject,
  type JsonValue,
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
};

export type MutationOptions = {
  readonly requestId?: string;
  readonly ifState?: string;
};

export type WaitOptions = {
  readonly timeoutMs?: number;
  readonly stableForMs?: number;
  readonly signal?: AbortSignal;
};

export type ScreenPredicate = JsonObject;
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

  static async create(options: TermtraceOptions): Promise<Termtrace> {
    const driver = new DriverClient(options.binaryPath ?? process.env.TERMTRACE_BINARY ?? "termtrace");
    const hello = await driver.request(method({ method: "hello" }));
    if (hello.kind !== "hello") {
      await driver.close();
      throw new TermtraceProtocolError("driver hello response has an invalid kind");
    }
    return new Termtrace(driver, options.evidenceRoot);
  }

  async launch(options: LaunchOptions): Promise<Session> {
    const evidenceRoot = options.evidenceRoot ?? this.#evidenceRoot;
    if (evidenceRoot === undefined) {
      throw new TypeError("launch requires evidenceRoot on Termtrace or this call");
    }
    const viewport = options.viewport ?? { cols: 80, rows: 24 };
    const command = options.command.map((argument) => Buffer.from(argument).toString("base64"));
    const data = parseAttachData(
      await this.#driver.request(
        method({
          method: "launch",
          evidence_root: evidenceRoot,
          command,
          cols: viewport.cols,
          rows: viewport.rows,
          working_directory: options.workingDirectory ?? null,
        }),
      ),
    );
    return new Session(this.#driver, data, true, options.evidence ?? "always");
  }

  async attach(session: string): Promise<Session> {
    const data = parseAttachData(await this.#driver.request(method({ method: "attach", session })));
    return new Session(this.#driver, data, false, "manual");
  }

  async close(): Promise<void> {
    await this.#driver.close();
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }
}

type SessionIdentity = {
  readonly handle: string;
  readonly sessionId: string;
  readonly canonicalSession: string;
  readonly completed: boolean;
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

  constructor(driver: DriverClient, identity: SessionIdentity, owned: boolean, evidencePolicy: "always" | "on-failure" | "manual") {
    this.#driver = driver;
    this.#handle = identity.handle;
    this.#owned = owned;
    this.#evidencePolicy = evidencePolicy;
    this.#completed = identity.completed ?? false;
    this.sessionId = identity.sessionId;
    this.canonicalSession = identity.canonicalSession;
    this.screen = new Screen(this);
    this.keyboard = new Keyboard(this);
    this.output = new Output(this);
    this.pointer = new Pointer(this);
    this.semantic = new Semantic(this);
    this.events = new Events(this);
    this.native = new Native(this);
    this.evidence = new Evidence(this);
  }

  control(command: JsonObject, options: MutationOptions = {}): Promise<JsonObject> {
    const expectedState: JsonValue = options.ifState ?? null;
    return this.#driver.request(
      method({
        method: "control",
        handle: this.#handle,
        command,
        expected_presented_state_hash: expectedState,
      }),
      options.requestId,
    );
  }

  request(methodFields: JsonObject, requestId?: string): Promise<JsonObject> {
    return this.#driver.request(
      method({ ...methodFields, handle: this.#handle }),
      requestId,
    );
  }

  completion(): Promise<JsonObject> {
    return this.#driver.request(method({ method: "completion", handle: this.#handle }));
  }

  complete(timeoutMs: number): Promise<JsonObject> {
    return this.#driver.request(method({ method: "complete", handle: this.#handle, timeout_ms: timeoutMs })).then((result) => {
      this.#completed = true;
      return result;
    });
  }

  waitForOutput(data: Uint8Array, timeoutMs: number): Promise<JsonObject> {
    return this.#driver.request(
      method({
        method: "wait_output",
        handle: this.#handle,
        data: Buffer.from(data).toString("base64"),
        timeout_ms: timeoutMs,
      }),
    );
  }

  stop(options: MutationOptions = {}): Promise<JsonObject> {
    return this.control({ command: "stop" }, options);
  }

  resize(viewport: { readonly cols: number; readonly rows: number; readonly pixelWidth?: number; readonly pixelHeight?: number }, options: MutationOptions = {}): Promise<JsonObject> {
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
    await this.#driver.request(method({ method: "detach", handle: this.#handle }));
    this.#closed = true;
  }

  /** Complete owned work and apply the explicit success-sensitive retention policy. */
  async finish(passed: boolean): Promise<void> {
    if (this.#closed) return;
    if (!this.#owned) throw new TypeError("finish is valid only for sessions launched by this client");
    await this.#ensureCompleted();
    if (passed && this.#evidencePolicy === "on-failure") {
      await this.#driver.request(method({ method: "discard_completed", handle: this.#handle }));
      this.#closed = true;
      return;
    }
    await this.#driver.request(method({ method: "detach", handle: this.#handle }));
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

  snapshot(): Promise<JsonObject> {
    return this.session.control({ command: "snapshot" });
  }

  async text(): Promise<string> {
    const frame = await this.frame();
    if (typeof frame.viewport_text !== "string") {
      throw new TermtraceProtocolError("snapshot response is missing viewport text");
    }
    return frame.viewport_text;
  }

  async frame(point: StatePoint = { selector: "latest_presented" }): Promise<JsonObject> {
    const data = await this.session.request({ method: "observe", point, include_hidden: false });
    if (data.kind !== "observation" || typeof data.observation !== "object" || data.observation === null || Array.isArray(data.observation)) {
      throw new TermtraceProtocolError("observe response has an invalid shape");
    }
    const observation = data.observation as JsonObject;
    if (typeof observation.frame !== "object" || observation.frame === null || Array.isArray(observation.frame)) {
      throw new TermtraceProtocolError("observation is missing its compact frame");
    }
    return observation.frame as JsonObject;
  }

  async waitForText(expected: string | RegExp, options: WaitOptions = {}): Promise<JsonObject> {
    const timeoutMs = nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs");
    const stableForMs = nonnegativeDuration(options.stableForMs ?? 0, "stableForMs");
    options.signal?.throwIfAborted();
    if (options.signal === undefined) {
      const predicate: ScreenPredicate = typeof expected === "string"
        ? { predicate: "contains", text: expected }
        : { predicate: "matches_regex", pattern: rustRegexSource(expected) };
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
        return { kind: "screen_wait", matched: true, viewport_text: text };
      }
      if (now - started >= timeoutMs) {
        throw new TermtraceProtocolError("screen wait timed out");
      }
      await delay(Math.min(25, Math.max(1, timeoutMs - (now - started))), options.signal);
    }
  }

  waitForPredicate(predicate: ScreenPredicate, options: WaitOptions = {}): Promise<JsonObject> {
    if (options.signal !== undefined) {
      throw new TypeError("signal cancellation is available only for text and RegExp polling waits");
    }
    const timeoutMs = nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs");
    const stableForMs = nonnegativeDuration(options.stableForMs ?? 0, "stableForMs");
    return this.session.control({
      command: "wait_screen",
      predicate,
      stable_for_ns: stableForMs * 1_000_000,
      timeout_ns: timeoutMs * 1_000_000,
    });
  }

  waitForAbsence(text: string, options: WaitOptions = {}): Promise<JsonObject> {
    return this.waitForPredicate({ predicate: "absent", text }, options);
  }

  screenshot(destination: string, point: StatePoint = { selector: "latest_presented" }): Promise<JsonObject> {
    return this.session.request({
      method: "screenshot",
      point,
      destination,
      renderer: "semantic",
      viewport: null,
    });
  }
}

export class Keyboard {
  constructor(private readonly session: Session) {}

  press(key: string, options: MutationOptions = {}): Promise<JsonObject> {
    return this.session.control(
      {
        command: "key",
        key,
        modifiers: { shift: false, control: false, alt: false, super_key: false },
      },
      options,
    );
  }

  type(text: string, options: MutationOptions = {}): Promise<JsonObject> {
    return this.bytes(new TextEncoder().encode(text), { ...options, inputKind: "text" });
  }

  paste(text: string, options: MutationOptions = {}): Promise<JsonObject> {
    return this.session.control(
      { command: "paste", data: Buffer.from(text).toString("base64") },
      options,
    );
  }

  bytes(bytes: Uint8Array, options: MutationOptions & { readonly inputKind?: "raw" | "text" } = {}): Promise<JsonObject> {
    const data = Buffer.from(bytes).toString("base64");
    return this.session.control(
      { command: "input", data, input_kind: options.inputKind ?? "raw", intent: null },
      options,
    );
  }
}

export class Pointer {
  constructor(private readonly session: Session) {}

  click(column: number, row: number, button: "left" | "middle" | "right" = "left", options: MutationOptions & { readonly strict?: boolean } = {}): Promise<JsonObject> {
    return this.session.control({
      command: "pointer",
      column,
      row,
      operation: { kind: "click", button },
      strict: options.strict ?? false,
    }, options);
  }

  multiClick(column: number, row: number, count: 2 | 3 | 4, button: "left" | "middle" | "right" = "left", options: MutationOptions & { readonly strict?: boolean } = {}): Promise<JsonObject> {
    return this.session.control({
      command: "pointer",
      column,
      row,
      operation: { kind: "multi_click", button, count },
      strict: options.strict ?? false,
    }, options);
  }

  drag(fromColumn: number, fromRow: number, toColumn: number, toRow: number, options: MutationOptions & { readonly button?: "left" | "middle" | "right"; readonly steps?: number; readonly strict?: boolean } = {}): Promise<JsonObject> {
    return this.session.control({
      command: "pointer",
      column: fromColumn,
      row: fromRow,
      operation: { kind: "drag", button: options.button ?? "left", to_column: toColumn, to_row: toRow, steps: options.steps ?? 8 },
      strict: options.strict ?? false,
    }, options);
  }

  scroll(column: number, row: number, lines: number, options: MutationOptions & { readonly strict?: boolean } = {}): Promise<JsonObject> {
    return this.session.control({
      command: "pointer",
      column,
      row,
      operation: { kind: "scroll", lines },
      strict: options.strict ?? false,
    }, options);
  }

  focus(focused: boolean, options: MutationOptions & { readonly strict?: boolean } = {}): Promise<JsonObject> {
    return this.session.control({ command: "focus", focused, strict: options.strict ?? false }, options);
  }
}

export class Semantic {
  constructor(private readonly session: Session) {}

  inspect(point: StatePoint = { selector: "latest_presented" }, includeHidden = false): Promise<JsonObject> {
    return this.session.request({ method: "observe", point, include_hidden: includeHidden });
  }

  timeline(options: { readonly allStates?: boolean; readonly controls?: boolean } = {}): Promise<JsonObject> {
    return this.session.request({
      method: "timeline",
      all_states: options.allStates ?? false,
      controls: options.controls ?? false,
    });
  }

  diff(from: StatePoint, to: StatePoint): Promise<JsonObject> {
    return this.session.request({ method: "diff", from, to });
  }

  diagnose(checks: ReadonlyArray<string> = [], broadChangePercent = 50): Promise<JsonObject> {
    return this.session.request({
      method: "diagnose",
      checks,
      broad_change_percent: broadChangePercent,
    });
  }
}

export class Events {
  constructor(private readonly session: Session) {}

  page(options: { readonly fromEventSeq?: number; readonly limit?: number; readonly metadataOnly?: boolean } = {}): Promise<JsonObject> {
    return this.session.request({
      method: "events",
      from_event_seq: options.fromEventSeq ?? 0,
      limit: options.limit ?? 256,
      metadata_only: options.metadataOnly ?? false,
    });
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
  ): Promise<JsonObject> {
    return this.session.request({
      method: "screenshot",
      point: options.point ?? { selector: "latest_presented" },
      destination,
      renderer: "native",
      viewport: options.viewport ?? null,
    });
  }
}

export class Evidence {
  constructor(private readonly session: Session) {}

  completion(): Promise<JsonObject> {
    return this.session.completion();
  }

  complete(options: Pick<WaitOptions, "timeoutMs"> = {}): Promise<JsonObject> {
    return this.session.complete(nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs"));
  }

  verify(level: "structure" | "content" | "durability_record" = "content"): Promise<JsonObject> {
    return this.session.request({ method: "verify", level });
  }

  bundle(destination: string, profile: "screen" | "debug" | "forensic" = "screen"): Promise<JsonObject> {
    return this.session.request({ method: "bundle", destination, profile });
  }
}

export class Output {
  constructor(private readonly session: Session) {}

  waitForBytes(data: Uint8Array, options: Pick<WaitOptions, "timeoutMs"> = {}): Promise<JsonObject> {
    return this.session.waitForOutput(
      data,
      nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs"),
    );
  }

  waitForText(text: string, options: Pick<WaitOptions, "timeoutMs"> = {}): Promise<JsonObject> {
    return this.waitForBytes(new TextEncoder().encode(text), options);
  }
}

function controlData(value: JsonObject): JsonObject {
  if (value.kind !== "control" || typeof value.data !== "object" || value.data === null || Array.isArray(value.data)) {
    throw new TermtraceProtocolError("driver control response has an invalid shape");
  }
  return value.data as JsonObject;
}

function nonnegativeDuration(value: number, field: string): number {
  if (!Number.isFinite(value) || value < 0 || !Number.isSafeInteger(value)) {
    throw new TypeError(`${field} must be a nonnegative integer`);
  }
  return value;
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
