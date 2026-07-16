import { spawn, type ChildProcessByStdio } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import { randomUUID } from "node:crypto";
import type { Readable, Writable } from "node:stream";

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

type PendingRequest = {
  readonly requestId: string;
  readonly accept: (data: DriverData) => void;
  readonly reject: (cause: unknown) => void;
};

/** Owns a driver subprocess and guarantees one correlated response per request. */
export class DriverClient {
  readonly #process: ChildProcessByStdio<Writable, Readable, Readable>;
  readonly #lines: Interface;
  readonly #pending: Array<PendingRequest> = [];
  #closed = false;

  constructor(binaryPath: string) {
    this.#process = spawn(binaryPath, ["driver"], { stdio: ["pipe", "pipe", "pipe"] });
    this.#process.stderr.resume();
    this.#lines = createInterface({ input: this.#process.stdout, crlfDelay: Infinity });
    this.#lines.on("line", (line) => this.#acceptLine(line));
    this.#process.once("error", (cause) => this.#rejectAll(cause));
    this.#process.once("exit", () => this.#rejectAll(new TermtraceProtocolError("driver exited")));
  }

  request<Kind extends DriverDataKind>(
    method: JsonObject,
    expectedKind: Kind,
    requestId: string = randomUUID(),
  ): Promise<DriverDataOf<Kind>> {
    if (this.#closed) {
      return Promise.reject(new TermtraceProtocolError("driver is closed"));
    }
    return new Promise<DriverDataOf<Kind>>((resolve, reject) => {
      const accept = (data: DriverData): void => {
        try {
          resolve(expectDriverData(data, expectedKind));
        } catch (cause: unknown) {
          reject(cause);
        }
      };
      this.#pending.push({ requestId, accept, reject });
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

  #acceptLine(line: string): void {
    const pending = this.#pending.shift();
    if (pending === undefined) {
      this.#rejectAll(new TermtraceProtocolError("driver emitted an unsolicited response"));
      return;
    }
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

  #rejectAll(cause: unknown): void {
    for (const pending of this.#pending.splice(0)) {
      pending.reject(cause);
    }
  }
}

export function method(fields: unknown): JsonObject {
  return parseJsonObject(fields);
}
