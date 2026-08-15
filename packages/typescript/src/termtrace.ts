import { DriverClient, method } from "./transport/driver-client.js";
import type { JsonObject, JsonValue } from "./transport/protocol.js";
import { Session } from "./session/session.js";

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
