import { Termtrace, type MediaArtifactResult, type TermtraceError } from "@zigai/termtrace";
import { decimalUnsigned, timeoutMilliseconds, validateRegion } from "@zigai/termtrace/contracts";

declare const envelope: MediaArtifactResult;
const exact: bigint = envelope.members[0].byte_length;
const bytes: Uint8Array = envelope.directory_bytes;
const max: bigint = decimalUnsigned("340282366920938463463374607431768211455", 128);
timeoutMilliseconds(1250);
validateRegion({ top: 0, left: 0, bottom: 1, right: 1 }, 24, 80);
// @ts-expect-error Wide integers must never be Number.
envelope.source.boundary.next_event_seq = 9007199254740993;
// @ts-expect-error Bytes are not strings in memory.
envelope.directory_bytes = "AA==";
// @ts-expect-error Only explicitly supported integer widths.
decimalUnsigned("1", 32);
// @ts-expect-error Explicit numeric milliseconds, not a duration string.
timeoutMilliseconds("1s");
if (envelope.provenance.kind === "presentation_output") {
    const source: "semantic_rendering" | "native_replay" | "original_execution" =
        envelope.provenance.source_kind;
    void source;
}
async function consumer(tt: Termtrace): Promise<void> {
    const recording = await tt.record(["/bin/sh", "-c", "printf READY"], 30, 96);
    const frame = await recording.screen().frame();
    const rows: number = frame.rows;
    const output = await recording.readOutput(0n, 4096);
    const raw: Uint8Array = output.bytes;
    try {
        await recording.stop();
    } catch (error) {
        const failure = error as TermtraceError;
        if (failure.scope === "actor" && failure.effect.kind === "unknown") {
            const retry: typeof failure.retry = "same_request_id";
            void retry;
        }
    }
    void [rows, raw];
}
void [exact, bytes, max, consumer];
