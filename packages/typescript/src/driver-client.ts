import { spawn, type ChildProcessByStdio } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import { addAbortListener } from "node:events";
import { randomUUID } from "node:crypto";
import type { Readable, Writable } from "node:stream";
import { RequestTimeoutError } from "./errors.js";

import {
    encodeDriverRequest,
    expectDriverData,
    parseDriverResponse,
    parseJsonObject,
    TermtraceOperationError,
    TermtraceProtocolError,
    type DriverData,
    type DriverDataKind,
    type DriverDataOf,
    type JsonObject,
} from "./protocol.js";

const PROCESS_TREE_TERMINATION_GRACE_MS = 1_000;

export type DriverRequestOptions = {
    readonly requestId?: string;
    readonly timeoutMs?: number;
    readonly signal?: AbortSignal;
};

type PendingRequest = {
    readonly requestId: string;
    readonly accept: (data: DriverData) => void;
    readonly reject: (cause: unknown) => void;
    readonly timer: NodeJS.Timeout | undefined;
    readonly abortSubscription: Disposable | undefined;
};

/** Owns a driver subprocess and guarantees one correlated response per request. */
export class DriverClient {
    readonly #process: ChildProcessByStdio<Writable, Readable, Readable>;
    readonly #lines: Interface;
    readonly #pending: Array<PendingRequest> = [];
    #closed = false;
    #terminating = false;

    constructor(binaryPath: string) {
        this.#process = spawn(binaryPath, ["tooling", "driver"], {
            stdio: ["pipe", "pipe", "pipe"],
            detached: process.platform !== "win32",
        });
        this.#process.stderr.resume();
        this.#lines = createInterface({ input: this.#process.stdout, crlfDelay: Infinity });
        this.#lines.on("line", (line) => this.#acceptLine(line));
        this.#process.once("error", (cause) => {
            if (!this.#terminating) {
                this.#closeWithError(cause);
            }
        });
        this.#process.once("exit", () => {
            if (!this.#terminating) {
                this.#closeWithError(new TermtraceProtocolError("driver exited"));
            }
        });
        this.#process.stdin.on("error", (cause) => {
            if (!this.#terminating) {
                this.#closeWithError(cause);
            }
        });
    }

    request<Kind extends DriverDataKind>(
        method: JsonObject,
        expectedKind: Kind,
        options: DriverRequestOptions = {},
    ): Promise<DriverDataOf<Kind>> {
        if (this.#closed) {
            return Promise.reject(new TermtraceProtocolError("driver is closed"));
        }
        if (
            options.timeoutMs !== undefined &&
            (!Number.isSafeInteger(options.timeoutMs) || options.timeoutMs <= 0)
        ) {
            return Promise.reject(new TypeError("request timeout must be a positive safe integer"));
        }
        if (options.signal?.aborted === true) {
            return Promise.reject(options.signal.reason);
        }
        const requestId = options.requestId ?? randomUUID();
        const timeoutMs = options.timeoutMs;
        const signal = options.signal;
        return new Promise<DriverDataOf<Kind>>((resolve, reject) => {
            const accept = (data: DriverData): void => {
                try {
                    resolve(expectDriverData(data, expectedKind));
                } catch (cause: unknown) {
                    reject(cause);
                }
            };
            const timer =
                timeoutMs === undefined
                    ? undefined
                    : setTimeout(() => {
                          void this.#terminateWithFailure(new RequestTimeoutError(timeoutMs));
                      }, timeoutMs);
            timer?.unref();
            const abortSubscription =
                signal === undefined
                    ? undefined
                    : addAbortListener(signal, () => {
                          void this.#terminateWithFailure(signal.reason);
                      });
            this.#pending.push({
                requestId,
                accept,
                reject,
                timer,
                abortSubscription,
            });
            this.#process.stdin.write(`${encodeDriverRequest(method, requestId)}\n`);
        });
    }

    async close(): Promise<void> {
        if (this.#closed) {
            return;
        }
        this.#closed = true;
        this.#process.stdin.end();
        if (this.#process.exitCode !== null) {
            return;
        }
        await new Promise<void>((resolve) => this.#process.once("exit", () => resolve()));
    }

    async #terminateWithFailure(failure: unknown): Promise<void> {
        if (this.#closed) {
            return;
        }
        this.#closed = true;
        this.#terminating = true;
        this.#lines.close();
        const exited =
            this.#process.exitCode === null
                ? new Promise<void>((resolve) => this.#process.once("exit", () => resolve()))
                : Promise.resolve();
        this.#signalProcessTree("SIGTERM");
        await new Promise<void>((resolve) =>
            setTimeout(resolve, PROCESS_TREE_TERMINATION_GRACE_MS),
        );
        if (process.platform !== "win32" || this.#process.exitCode === null) {
            this.#signalProcessTree("SIGKILL");
        }
        await exited;
        this.#rejectAll(failure);
    }

    #signalProcessTree(signal: NodeJS.Signals): void {
        const pid = this.#process.pid;
        if (process.platform !== "win32" && pid !== undefined) {
            try {
                process.kill(-pid, signal);
                return;
            } catch {
                if (this.#process.exitCode !== null) {
                    return;
                }
            }
        }
        this.#process.kill(signal);
    }

    #acceptLine(line: string): void {
        const pending = this.#pending.shift();
        if (pending === undefined) {
            this.#rejectAll(new TermtraceProtocolError("driver emitted an unsolicited response"));
            return;
        }
        this.#clearPendingGuards(pending);
        try {
            const input: unknown = JSON.parse(line);
            const response = parseDriverResponse(input, pending.requestId);
            if (response._tag === "Failure") {
                pending.reject(new TermtraceOperationError(response.failure));
                return;
            }
            pending.accept(response.data);
        } catch (cause: unknown) {
            pending.reject(cause);
        }
    }

    #closeWithError(cause: unknown): void {
        this.#closed = true;
        this.#rejectAll(cause);
    }

    #rejectAll(cause: unknown): void {
        for (const pending of this.#pending.splice(0)) {
            this.#clearPendingGuards(pending);
            pending.reject(cause);
        }
    }

    #clearPendingGuards(pending: PendingRequest): void {
        clearTimeout(pending.timer);
        pending.abortSubscription?.[Symbol.dispose]();
    }
}

export function method(fields: unknown): JsonObject {
    return parseJsonObject(fields);
}
