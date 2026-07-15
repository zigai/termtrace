import { spawn, type ChildProcessByStdio } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import { randomUUID } from "node:crypto";
import type { Readable, Writable } from "node:stream";

import {
  parseDriverResponse,
  parseString,
  TermtraceOperationError,
  TermtraceProtocolError,
  type JsonObject,
  type JsonValue,
} from "./protocol.js";

type PendingRequest = {
  readonly requestId: string;
  readonly resolve: (data: JsonObject) => void;
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

  request(method: JsonObject, requestId: string = randomUUID()): Promise<JsonObject> {
    if (this.#closed) {
      return Promise.reject(new TermtraceProtocolError("driver is closed"));
    }
    return new Promise<JsonObject>((resolve, reject) => {
      this.#pending.push({ requestId, resolve, reject });
      this.#process.stdin.write(`${JSON.stringify({ version: 1, request_id: requestId, method })}\n`);
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
      pending.resolve(response.data);
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

export type AttachData = {
  readonly handle: string;
  readonly sessionId: string;
  readonly canonicalSession: string;
  readonly completed: boolean;
};

export function parseAttachData(data: JsonObject): AttachData {
  if (data.kind !== "attached" || typeof data.completed !== "boolean") {
    throw new TermtraceProtocolError("attach response has an invalid kind or completion state");
  }
  return {
    handle: parseString(data.handle, "attach.handle"),
    sessionId: parseString(data.session_id, "attach.session_id"),
    canonicalSession: parseString(data.canonical_session, "attach.canonical_session"),
    completed: data.completed,
  };
}

export function method(fields: JsonObject): JsonObject {
  return fields;
}

export function controlCommand(command: JsonObject): JsonValue {
  return command;
}
