import { DriverClient } from "./transport/driver-client.js";
import { resolveBinary } from "./transport/binary.js";
import type { DriverMethodOf } from "./transport/protocol.js";
import { Session } from "./session/session.js";

export type TermtraceOptions = {
    readonly binaryPath?: string;
    readonly evidenceRoot?: string;
    readonly signal?: AbortSignal;
};

export type LaunchOptions = {
    readonly command: ReadonlyArray<string>;
    readonly evidenceRoot?: string;
    readonly viewport?: { readonly cols: number; readonly rows: number };
    readonly workingDirectory?: string;
    readonly evidence?: "always" | "on-failure" | "manual";
    readonly environment?: EnvironmentOptions;
    readonly outputByteBudget?: number;
    readonly signal?: AbortSignal;
};

export type EnvironmentOptions = {
    readonly inheritEnv?: boolean;
    readonly set?: Readonly<Record<string, string | Uint8Array>>;
    readonly copy?: ReadonlyArray<string>;
    readonly unset?: ReadonlyArray<string>;
    readonly color?: "auto" | "always" | "never";
};

/** Context-managed entry point for attached canonical sessions. */
export class Termtrace implements AsyncDisposable {
    readonly #driver: DriverClient;
    readonly #evidenceRoot: string | undefined;
    readonly #sessions = new Set<Session>();
    #closing: Promise<void> | undefined;

    private constructor(driver: DriverClient, evidenceRoot: string | undefined) {
        this.#driver = driver;
        this.#evidenceRoot = evidenceRoot;
    }

    static async create(options: TermtraceOptions = {}): Promise<Termtrace> {
        options.signal?.throwIfAborted();

        const driver = new DriverClient(resolveBinary(options.binaryPath));

        try {
            const requestOptions =
                options.signal === undefined
                    ? { timeoutMs: 10_000 }
                    : { timeoutMs: 10_000, signal: options.signal };
            await driver.request({ method: "hello" }, "hello", requestOptions);
        } catch (cause: unknown) {
            await driver.close();
            throw cause;
        }

        return new Termtrace(driver, options.evidenceRoot);
    }

    async launch(options: LaunchOptions): Promise<Session> {
        options.signal?.throwIfAborted();

        const evidenceRoot = options.evidenceRoot ?? this.#evidenceRoot;
        if (evidenceRoot === undefined) {
            throw new TypeError("launch requires evidenceRoot on Termtrace or this call");
        }

        const viewport = options.viewport ?? { cols: 160, rows: 45 };
        const command = options.command.map((argument) => Buffer.from(argument).toString("base64"));
        const data = await this.#driver.request(
            {
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
            },
            "attached",
            options.signal === undefined ? {} : { signal: options.signal },
        );
        const session = new Session(this.#driver, data, true, options.evidence ?? "always");
        this.#sessions.add(session);

        return session;
    }

    async attach(session: string): Promise<Session> {
        const data = await this.#driver.request({ method: "attach", session }, "attached");
        return new Session(this.#driver, data, false, "manual");
    }

    close(options: { readonly requestTimeoutMs?: number } = {}): Promise<void> {
        this.#closing ??= this.#closeOwned(options.requestTimeoutMs ?? 10_000);
        return this.#closing;
    }

    async #closeOwned(requestTimeoutMs: number): Promise<void> {
        const failures: unknown[] = [];
        for (const session of this.#sessions) {
            try {
                await session.finish(false, requestTimeoutMs);
            } catch (cause: unknown) {
                failures.push(cause);
            }
        }

        if (failures.length > 0) {
            await this.#driver.terminate(
                new AggregateError(failures, "Termtrace finalization failed"),
            );
        }

        try {
            await this.#driver.close();
        } catch (cause: unknown) {
            failures.push(cause);
        }

        if (failures.length > 0) {
            throw new AggregateError(failures, "Termtrace client cleanup failed");
        }

        for (const session of this.#sessions) {
            await session.discardAfterDriverClose();
        }

        this.#sessions.clear();
    }

    async [Symbol.asyncDispose](): Promise<void> {
        await this.close();
    }
}

type EnvironmentSpec = NonNullable<DriverMethodOf<"launch">["environment"]>;

function encodeEnvironment(options: EnvironmentOptions | undefined): EnvironmentSpec {
    const set: Record<string, string> = {};
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
