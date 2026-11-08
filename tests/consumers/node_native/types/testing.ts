import { type ScreenFrame, type QueryResult, type ScreenMatch } from "@zigai/termtrace";
import {
    withTermtrace,
    assertScreenText,
    assertCount,
    assertExists,
    assertNotExists,
    assertCursor,
    assertNoWrappedRows,
    assertNoPresentedTransient,
    assertNeutralRegion,
    assertMatchText,
    assertMatchStyle,
    assertSnapshotFile,
    assertMaskedSnapshotFile,
    normalizedSnapshot,
    maskedSnapshot,
    assertNot,
    ExpectationMismatchError,
    type FinalizationReport,
} from "@zigai/termtrace/testing";
async function consumer(
    frame: ScreenFrame,
    query: QueryResult,
    selected: ScreenMatch,
): Promise<void> {
    assertScreenText(frame, "READY");
    assertCount(query, 1);
    assertExists(query);
    assertNotExists(query);
    assertCursor(frame, frame.cursor);
    assertNoWrappedRows(frame);
    assertNoPresentedTransient(frame);
    assertNeutralRegion(frame, { top: 0, left: 0, right: 1, bottom: 1 });
    assertMatchText(selected, "READY");
    assertMatchStyle(selected, { bold: true });
    assertSnapshotFile(frame, "/tmp/snapshot", { profile: "spans", update: false });
    assertMaskedSnapshotFile(frame, "/tmp/masked", { profile: "cells", mask_regions: [] }, true);
    const text: string = normalizedSnapshot(frame, { profile: "text" });
    const masked: string = maskedSnapshot(frame, { mask_regions: [] });
    assertNot(() => assertScreenText(frame, "ABSENT"));
    const report: FinalizationReport = await withTermtrace(
        async (ctx) => {
            const rec = await ctx.record(["/bin/sh"]);
            const result = await rec.actions().sendText("x").run();
            if (result.frame) assertScreenText(result.frame, "x");
            return ctx.finalize("passed");
        },
        { evidencePolicy: "manual", finalizationTimeoutMs: 2500 },
    );
    const failure = new ExpectationMismatchError("mismatch");
    const truncated: boolean = failure.truncated;
    // @ts-expect-error Exact captured frames, not strings or arbitrary summaries.
    assertScreenText("READY", "READY");
    // @ts-expect-error Snapshot profiles use the generated Rust vocabulary.
    maskedSnapshot(frame, { profile: "unknown" });
    // @ts-expect-error Wide cursor facts stay optional integers, not strings.
    assertCursor(frame, { ...frame.cursor, row: "0" });
    // @ts-expect-error Query assertions require captured QueryResult, never a lazy locator/string.
    assertCount("READY", 1);
    // @ts-expect-error Updates require boolean opt-in.
    assertSnapshotFile(frame, "/tmp/baseline", { update: "yes" });
    void text;
    void masked;
    void report;
    void truncated;
}
void consumer;
