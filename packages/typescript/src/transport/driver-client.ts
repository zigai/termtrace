import { spawn, type ChildProcessByStdio } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import { addAbortListener } from "node:events";
import { randomUUID } from "node:crypto";
import type { Readable, Writable } from "node:stream";
import { RequestTimeoutError } from "../errors.js";

import {
    encodeDriverRequest,
    expectDriverData,
    parseDriverResponse,
    TermtraceOperationError,
    TermtraceProtocolError,
    type DriverData,
    type DriverDataKind,
    type DriverDataOf,
    type DriverMethod,
    type JsonValue,
} from "./protocol.js";

const PROCESS_TREE_TERMINATION_GRACE_MS = 5_000;
const DRIVER_CLOSE_TIMEOUT_MS = 5_000;
const MAX_PENDING_REQUESTS = 256;

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
    readonly #exited: Promise<void>;
    #termination: Promise<void> | undefined;
    #closePromise: Promise<void> | undefined;
    #closedSuccessfully = false;

    constructor(binaryPath: string) {
        this.#process = spawn(binaryPath, ["agent"], {
            stdio: ["pipe", "pipe", "pipe"],
            detached: process.platform !== "win32",
        });
        this.#exited = new Promise<void>((resolve) => {
            this.#process.once("exit", () => resolve());
            this.#process.once("error", () => resolve());
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

    get closedSuccessfully(): boolean {
        return this.#closedSuccessfully;
    }
    get isClosed(): boolean {
        return this.#closed;
    }

    /** Stop and reap all work owned by this driver after unsuccessful finalization. */
    terminate(cause: unknown): Promise<void> {
        return this.#terminateWithFailure(cause);
    }

    request<Kind extends DriverDataKind>(
        method: DriverMethod,
        expectedKind: Kind,
        options: DriverRequestOptions = {},
    ): Promise<DriverDataOf<Kind>> {
        if (this.#closed) {
            return Promise.reject(new TermtraceProtocolError("driver is closed"));
        }
        if (this.#pending.length >= MAX_PENDING_REQUESTS) {
            return Promise.reject(new TermtraceProtocolError("driver request queue is full"));
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
        let encodedRequest: string;
        try {
            encodedRequest = encodeDriverRequest(method, requestId);
        } catch (cause: unknown) {
            return Promise.reject(cause);
        }
        return new Promise<DriverDataOf<Kind>>((resolve, reject) => {
            const accept = (data: DriverData): void => {
                try {
                    resolve(expectDriverData(data, expectedKind));
                } catch (cause: unknown) {
                    reject(cause);
                }
            };
            const cancel = (cause: unknown): void => {
                if (method.method === "launch" || method.method === "hello") {
                    void this.#terminateWithFailure(cause);
                    return;
                }
                const pending = this.#pending.find((request) => request.requestId === requestId);
                if (pending !== undefined) this.#clearPendingGuards(pending);
                reject(cause);
            };
            const timer =
                timeoutMs === undefined
                    ? undefined
                    : setTimeout(() => {
                          cancel(new RequestTimeoutError(timeoutMs));
                      }, timeoutMs);
            timer?.unref();
            const abortSubscription =
                signal === undefined
                    ? undefined
                    : addAbortListener(signal, () => {
                          cancel(signal.reason);
                      });
            this.#pending.push({
                requestId,
                accept,
                reject,
                timer,
                abortSubscription,
            });
            this.#process.stdin.write(`${encodedRequest}\n`);
        });
    }

    close(): Promise<void> {
        this.#closePromise ??= this.#closeProcess();
        return this.#closePromise;
    }

    async #closeProcess(): Promise<void> {
        if (this.#closed) {
            await this.#termination;
            await this.#exited;
            return;
        }
        this.#closed = true;
        this.#process.stdin.end();
        let timer: NodeJS.Timeout | undefined;
        const timedOut = await Promise.race([
            this.#exited.then(() => false),
            new Promise<boolean>((resolve) => {
                timer = setTimeout(() => resolve(true), DRIVER_CLOSE_TIMEOUT_MS);
            }),
        ]);
        clearTimeout(timer);
        if (timedOut) {
            const cause = new RequestTimeoutError(DRIVER_CLOSE_TIMEOUT_MS);
            await this.#terminateWithFailure(cause);
            throw cause;
        }
        if (this.#process.exitCode !== 0) {
            throw new TermtraceProtocolError("driver failed during close");
        }
        this.#closedSuccessfully = true;
    }

    #terminateWithFailure(cause: unknown): Promise<void> {
        if (this.#termination !== undefined) return this.#termination;
        this.#closed = true;
        this.#terminating = true;
        this.#lines.close();
        this.#termination = this.#terminateProcessTree(cause);
        return this.#termination;
    }

    async #terminateProcessTree(cause: unknown): Promise<void> {
        this.#signalProcessTree("SIGTERM");
        const deadline = Date.now() + PROCESS_TREE_TERMINATION_GRACE_MS;
        while (this.#processTreeExists() && Date.now() < deadline) {
            await new Promise<void>((resolve) => setTimeout(resolve, 25));
        }
        if (process.platform !== "win32" || this.#process.exitCode === null) {
            this.#signalProcessTree("SIGKILL");
        }
        await this.#exited;
        this.#rejectAll(cause);
    }

    #processTreeExists(): boolean {
        const pid = this.#process.pid;
        if (process.platform !== "win32" && pid !== undefined) {
            try {
                process.kill(-pid, 0);
                return true;
            } catch {
                return false;
            }
        }
        return this.#process.exitCode === null && this.#process.signalCode === null;
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
            const input: JsonValue = JSON.parse(line);
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
