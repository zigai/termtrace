import {
    CancelToken,
    LocatorSpec,
    ScreenFrame,
    type CursorState,
    type LocatorFailure,
    type MatchedCell,
    type Observation,
    type Screen,
    type StableScreenCapture,
    type StatePoint,
    type StyleFilter,
} from "@termtrace/core";

async function screenConsumer(screen: Screen, signal: AbortSignal): Promise<void> {
    const frame = await screen.frame({
        detail: "cells",
        region: { top: 0, left: 0, bottom: 2, right: 10 },
    });
    const height: number = frame.rows;
    const content = frame.content;
    if (content.detail === "cells") {
        const cell = content.rows[0].cells[0];
        const wide: "narrow" | "wide" | "spacer_tail" | "spacer_head" = cell.wide_kind;
        void wide;
    }
    const cursor: CursorState = frame.cursor;
    const row: number | null | undefined = cursor.row;
    const source = frame.source;
    if (source.source === "actor") {
        const observed: bigint = source.observed_monotonic_ns;
        void observed;
    }
    const style: StyleFilter = {
        bold: true,
        resolved_foreground: null,
        foreground: { kind: "palette", value: 1 },
    };
    const spec = LocatorSpec.text("Save")
        .withStyle(style)
        .inRegion({ top: 0, left: 0, bottom: 1, right: 20 });
    const strict = await screen.locator(spec).single();
    const cells: MatchedCell[] = strict.cells;
    const width: number = cells[0].width;
    const waited = await screen
        .getByText("Loading")
        .waitFor({ condition: "absent", timeoutMs: 5000, signal });
    const counted = await screen
        .getByRegex("(?i)save")
        .waitFor({ condition: { count: 2n }, cancel: new CancelToken() });
    const offline = ScreenFrame.fromJson("{}").query(LocatorSpec.exact("READY")).count();
    const point: StatePoint = { selector: "state_id", state_id: 9007199254740993n };
    const resolved = await screen.frameAt(point, { includeHidden: true });
    const observation: Observation = await screen.observe({ point });
    const stateId: bigint | null | undefined = observation.state_id;
    const capture: StableScreenCapture = await screen.captureStable({
        stableForMs: 50,
        reaction: { timeout_ms: 100n },
    });
    const zstd: Uint8Array | null = capture.semantic_state_zstd ?? null;
    const outcome = await screen.waitFor(
        { predicate: "row_contains", row: 0, text: "READY" },
        { timeoutMs: 1000 },
    );
    const satisfied: boolean = outcome.satisfied;
    try {
        await screen.getByText("x").waitFor({ timeoutMs: 1 });
    } catch (error) {
        const failure = error as LocatorFailure;
        if (failure.kind === "locator_timeout" || failure.kind === "cancelled") {
            const identity: string | null | undefined = failure.frame_hash;
            void identity;
        }
    }
    // @ts-expect-error Host regexes are not accepted; patterns are Rust regex syntax strings.
    screen.getByRegex(/save/i);
    // @ts-expect-error u64 state identifiers are bigint, never Number.
    const lossy: StatePoint = { selector: "state_id", state_id: 1 };
    // @ts-expect-error Unknown conditions are rejected.
    await screen.getByText("x").waitFor({ condition: "present" });
    // @ts-expect-error Durations are explicit milliseconds.
    await screen.getByText("x").waitFor({ timeout: "1s" });
    // @ts-expect-error Only resolved colors accept null.
    const nullBold: StyleFilter = { bold: null };
    // @ts-expect-error Structured rows live under content, not the legacy numeric height.
    frame.rows.map((r: unknown) => r);
    void [
        height,
        row,
        strict,
        width,
        waited,
        counted,
        offline,
        resolved,
        stateId,
        zstd,
        satisfied,
        lossy,
        nullBold,
    ];
}
void screenConsumer;
