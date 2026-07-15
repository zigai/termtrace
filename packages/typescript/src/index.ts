import { DriverClient, method, parseAttachData } from "./driver-client.js";
import { TermtraceProtocolError, type JsonObject, type JsonValue } from "./protocol.js";

export {
  TermtraceOperationError,
  TermtraceProtocolError,
  type DriverFailure,
  type JsonObject,
  type JsonValue,
} from "./protocol.js";

export type TermtraceOptions = {
  readonly binaryPath: string;
  readonly evidenceRoot?: string;
};

export type LaunchOptions = {
  readonly command: ReadonlyArray<string>;
  readonly evidenceRoot?: string;
  readonly viewport?: { readonly cols: number; readonly rows: number };
  readonly workingDirectory?: string;
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

/** Context-managed entry point for attached canonical sessions. */
export class Termtrace implements AsyncDisposable {
  readonly #driver: DriverClient;
  readonly #evidenceRoot: string | undefined;

  private constructor(driver: DriverClient, evidenceRoot: string | undefined) {
    this.#driver = driver;
    this.#evidenceRoot = evidenceRoot;
  }

  static async create(options: TermtraceOptions): Promise<Termtrace> {
    const driver = new DriverClient(options.binaryPath);
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
    return new Session(this.#driver, data);
  }

  async attach(session: string): Promise<Session> {
    const data = parseAttachData(await this.#driver.request(method({ method: "attach", session })));
    return new Session(this.#driver, data);
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
};

/** Process-local handle with explicit canonical evidence identity. */
export class Session implements AsyncDisposable {
  readonly screen: Screen;
  readonly keyboard: Keyboard;
  readonly output: Output;
  readonly evidence: Evidence;
  readonly sessionId: string;
  readonly canonicalSession: string;
  readonly #driver: DriverClient;
  readonly #handle: string;
  #closed = false;

  constructor(driver: DriverClient, identity: SessionIdentity) {
    this.#driver = driver;
    this.#handle = identity.handle;
    this.sessionId = identity.sessionId;
    this.canonicalSession = identity.canonicalSession;
    this.screen = new Screen(this);
    this.keyboard = new Keyboard(this);
    this.output = new Output(this);
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

  completion(): Promise<JsonObject> {
    return this.#driver.request(method({ method: "completion", handle: this.#handle }));
  }

  complete(timeoutMs: number): Promise<JsonObject> {
    return this.#driver.request(method({ method: "complete", handle: this.#handle, timeout_ms: timeoutMs }));
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

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    await this.#driver.request(method({ method: "detach", handle: this.#handle }));
    this.#closed = true;
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
    const data = controlData(await this.snapshot());
    if (typeof data.viewport_text !== "string") {
      throw new TermtraceProtocolError("snapshot response is missing viewport text");
    }
    return data.viewport_text;
  }

  async waitForText(expected: string | RegExp, options: WaitOptions = {}): Promise<JsonObject> {
    const timeoutMs = nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs");
    const stableForMs = nonnegativeDuration(options.stableForMs ?? 0, "stableForMs");
    options.signal?.throwIfAborted();
    if (typeof expected === "string" && options.signal === undefined) {
      return this.session.control({
        command: "wait_screen",
        predicate: { predicate: "contains", text: expected },
        stable_for_ns: stableForMs * 1_000_000,
        timeout_ns: timeoutMs * 1_000_000,
      });
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

  bytes(bytes: Uint8Array, options: MutationOptions & { readonly inputKind?: "raw" | "text" } = {}): Promise<JsonObject> {
    const data = Buffer.from(bytes).toString("base64");
    return this.session.control(
      { command: "input", data, input_kind: options.inputKind ?? "raw", intent: null },
      options,
    );
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
