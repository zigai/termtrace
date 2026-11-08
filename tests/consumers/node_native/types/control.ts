import {
    ActionResult,
    Termtrace,
    type LaunchOptions,
    type MutationFailure,
    type PointerOutcome,
    type SendStepSpec,
    type SessionStatus,
    type VisualProfile,
    type VisualProfileReport,
} from "@zigai/termtrace";

async function consumer(tt: Termtrace): Promise<void> {
    const options: LaunchOptions = {
        environment: { base: "clear", set: { X: new Uint8Array([1, 255]) }, copy: ["HOME"] },
        output_byte_budget: 1024n,
        max_duration_ms: 100n,
    };
    const recording = await tt.launch(["/bin/sh"], options);
    const builder = recording
        .actions()
        .withRequestId("019fddda-17c8-7000-87da-8785e71ab5e0")
        .sendBytes(new Uint8Array([0]));
    const id: string = builder.requestId;
    const result: ActionResult = await builder
        .untilScreenWhere({ predicate: "contains", text: "OK" }, 500)
        .run();
    const written: bigint = result.input.bytes_written;
    const hash: string | null | undefined = result.postcondition.state_hash;
    const pointer: PointerOutcome = await recording
        .pointer()
        .drag(0, 0, 4, 0, { steps: 3, strict: true });
    if (pointer.kind === "applied") {
        const raw: Uint8Array = pointer.bytes;
        void raw;
    }
    const steps: SendStepSpec[] = [
        { step: "input", atoms: [{ bytes: new Uint8Array([27]) }, { key: "enter" }] },
        { step: "pointer", column: 1, row: 0, operation: { kind: "scroll", lines: -1 } },
    ];
    const sent = await recording.send({
        steps,
        request_id: id,
        until: { condition: "idle", stable_for_ms: 50n },
    });
    const lastId: string = sent.last.request_id;
    const status: SessionStatus = await recording.status();
    if (status.kind === "live") {
        const seq: bigint = status.boundaries.durable.next_event_seq;
        void seq;
    }
    try {
        await recording.mark("x");
    } catch (error) {
        const failure = error as MutationFailure;
        const failed: string = failure.request_id;
        const incomplete: ActionResult | undefined = failure.result;
        void failed;
        void incomplete;
    }
    const stopped = await recording.requestStop({ waitMs: 1000 });
    if (stopped.kind === "completed") {
        const limit: bigint | undefined = stopped.completion.resource_limit?.limit;
        void limit;
    }
    void written;
    void hash;
    void lastId;

    // @ts-expect-error Byte fields are Uint8Array, never text.
    const textBytes: SendStepSpec = { step: "input", atoms: [{ bytes: "text" }] };
    // @ts-expect-error Output byte budgets are u64 and therefore bigint.
    const narrow: LaunchOptions = { output_byte_budget: 1024 };
    // @ts-expect-error Pointer buttons are a closed set.
    await recording.pointer().click(0, 0, { button: "side" });
    // @ts-expect-error Wide result fields are bigint.
    const lossy: number = result.input.bytes_written;
    // @ts-expect-error Mutations take no AbortSignal.
    await builder.run({ signal: new AbortController().signal });
    const profile: VisualProfile = {
        version: 1,
        native: {
            font_family: "Example Mono",
            font_size_points: "13.5",
            padding_points: { x: 0, y: 4 },
            dpi: 144,
        },
        semantic: { preset: "bundled-v1" },
    };
    const report: VisualProfileReport = await tt.preflightVisualProfile(profile, 80, 24);
    const profileHash: string = report.profile_hash;
    const dpi: number | undefined = report.native?.dpi;
    await tt.launch(["/bin/true"], { visual_profile: profile });
    // @ts-expect-error Font sizes are exact decimal strings, never floats.
    const floatSize: VisualProfile = { version: 1, native: { font_size_points: 13.5 } };
    // @ts-expect-error The only semantic preset is bundled-v1.
    const customPreset: VisualProfile = { version: 1, semantic: { preset: "custom" } };
    void textBytes;
    void narrow;
    void lossy;
    void profileHash;
    void dpi;
    void floatSize;
    void customPreset;

    const reporter = await tt.launch(["/bin/sh"], { cwd: "/srv/app", startup_timeout_ms: 500n });
    const pasted: ActionResult = await reporter
        .actions()
        .paste("words")
        .pace(20)
        .sendText("ab")
        .run();
    const scrolled: PointerOutcome = await reporter
        .pointer()
        .scroll(0, 0, { lines: -2, strict: true });
    const focused: PointerOutcome = await reporter.pointer().focus(false, { strict: true });
    const idleText: string = (await reporter.screen().waitForIdle(500)).text;
    const target = await tt.attachTarget("name:example");
    const cachedEntries: bigint = (await target.analysis().cacheStatus()).entries;
    reporter.detach();
    await reporter.discardCompleted();
    // @ts-expect-error Readiness deadlines are u64 milliseconds and therefore bigint.
    const numericDeadline: LaunchOptions = { startup_timeout_ms: 500 };
    // @ts-expect-error Wheel ticks are a signed count, never text.
    await reporter.pointer().scroll(0, 0, { lines: "2" });
    // @ts-expect-error Focus takes the new focus state, not options alone.
    await reporter.pointer().focus({ strict: true });
    // @ts-expect-error Cache entry counts are u64 and therefore bigint.
    const lossyEntries: number = (await target.analysis().cacheStatus()).entries;
    void pasted;
    void scrolled;
    void focused;
    void idleText;
    void cachedEntries;
    void numericDeadline;
    void lossyEntries;
}
void consumer;
