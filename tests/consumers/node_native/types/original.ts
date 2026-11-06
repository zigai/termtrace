import {
    CancelToken,
    Termtrace,
    type OriginalCaptureError,
    type OriginalLaunchOptions,
    type OriginalRecording,
    type RecordOriginalOptions,
} from "@termtrace/core";

async function consumer(tt: Termtrace, signal: AbortSignal): Promise<boolean> {
    const options: RecordOriginalOptions = {
        grid: { cols: 80, rows: 24 },
        canvas: { width: 1280, height: 720 },
        fps: 30,
        signal,
        cancel: new CancelToken(),
        output_byte_budget: 1024n,
    };
    const original = await tt.recordOriginal(["/bin/true"], options);
    await original.session().resize(100, 30);
    const state: string = (await original.captureStatus()).state;
    void state;
    try {
        const result: Awaited<ReturnType<OriginalRecording["finish"]>> =
            await original.finish(5000);
        const seq: bigint = result.capture.source.boundary.next_event_seq;
        const exit: number | null | undefined = result.completion.exit_code;
        void seq;
        void exit;
        return result.canonical_success;
    } catch (caught) {
        const error = caught as OriginalCaptureError;
        const reason: string = error.reason;
        void reason;
        return false;
    }
}

async function attach(tt: Termtrace): Promise<bigint> {
    const named = await tt.attachOriginalTarget("name:demo", "/state");
    const exact = await tt.attachOriginalTarget(
        new Uint8Array([0x2f, 0xff]),
        new Uint8Array([0x2f]),
    );
    const bytes: Uint8Array = exact.pathBytes;
    const sessionBytes: Uint8Array = exact.session().pathBytes;
    void bytes;
    void sessionBytes;
    return (await named.captureStatus()).supervisor_pid;
}

function misuse(): void {
    // @ts-expect-error The captured terminal owns its grid; rows are not original options.
    const rows: OriginalLaunchOptions = { rows: 24 };
    // @ts-expect-error Capture rates use numeric frames per second.
    const rate: OriginalLaunchOptions = { fps: "30" };
    // @ts-expect-error Output byte budgets are u64 and therefore bigint.
    const budget: OriginalLaunchOptions = { output_byte_budget: 1024 };
    void rows;
    void rate;
    void budget;
}

void consumer;
void attach;
void misuse;
