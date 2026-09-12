import { randomUUID } from "node:crypto";
import { hrtime, pid } from "node:process";

import type { ControlCommand, ControlResult, JsonObject } from "../transport/protocol.js";
import type { MutationOptions } from "./session.js";

type InstrumentationSession = {
    control(command: ControlCommand, options?: MutationOptions): Promise<ControlResult>;
};

/** Application diagnostics over the control socket, with explicit span completion. */
export class Instrumentation {
    readonly processInstanceId = randomUUID();
    readonly #started = hrtime.bigint();
    readonly #session: InstrumentationSession;

    constructor(session: InstrumentationSession) {
        this.#session = session;
    }

    async event(name: string, attributes: JsonObject = {}): Promise<void> {
        await this.#emit("event", name, null, null, attributes);
    }

    async startSpan(
        name: string,
        attributes: JsonObject = {},
        parentSpanId: string | null = null,
    ): Promise<string> {
        const spanId = randomUUID();
        await this.#emit("span_start", name, spanId, parentSpanId, attributes);
        return spanId;
    }

    async endSpan(name: string, spanId: string, attributes: JsonObject = {}): Promise<void> {
        await this.#emit("span_end", name, spanId, null, attributes);
    }

    async #emit(
        kind: "event" | "span_start" | "span_end",
        name: string,
        spanId: string | null,
        parentSpanId: string | null,
        attributes: JsonObject,
    ): Promise<void> {
        const monotonicNs = Number(hrtime.bigint() - this.#started);
        if (!Number.isSafeInteger(monotonicNs)) {
            throw new RangeError("application monotonic clock exceeds the exact wire range");
        }

        await this.#session.control({
            command: "application",
            data: {
                process_instance_id: this.processInstanceId,
                process_id: pid,
                monotonic_ns: monotonicNs,
                kind,
                name,
                span_id: spanId,
                parent_span_id: parentSpanId,
                attributes,
            },
        });
    }
}
