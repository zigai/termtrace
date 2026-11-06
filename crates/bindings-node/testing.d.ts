import { EvidencePolicy, FinalizationReport, Recording, Termtrace } from "./index";

export interface WithTermtraceContext {
    termtrace: Termtrace;
    record: (...args: Parameters<Termtrace["record"]>) => Promise<Recording>;
    evidenceRoot: string;
    /** Explicit finalization commits this outcome; later body failures cannot undo deletion. */
    finalize(outcome?: FinalizationReport["outcome"]): Promise<FinalizationReport>;
    readonly report: FinalizationReport | undefined;
}

export interface WithTermtraceOptions {
    binary?: string;
    evidencePolicy?: EvidencePolicy;
    finalizationTimeoutMs?: number;
}

/** Diagnostics attached to extensible thrown errors as `error.termtrace`. */
export interface TermtraceTestDiagnostics {
    /** Evidence retained after a test or cleanup failure; reported to stderr too. */
    evidenceRoot: string | null;
    report?: FinalizationReport;
    /** Secondary cleanup failures, which never replace the original test failure. */
    cleanupErrors: unknown[];
}

/**
 * Run with isolated evidence and stop all recordings created through `record`.
 * Evidence is deleted only after the callback and cleanup both succeed.
 * Callback failures are rethrown unchanged; cleanup-only failures throw AggregateError.
 */
export declare function withTermtrace<T>(
    fn: (ctx: WithTermtraceContext) => Promise<T>,
    options?: WithTermtraceOptions,
): Promise<T>;

export {
    ExpectationMismatchError,
    MaskedSnapshotOptions,
    FinalizationReport,
    SecondaryFailure,
    normalizedSnapshot,
    maskedSnapshot,
    assertSnapshotFile,
    assertMaskedSnapshotFile,
    assertScreenText,
    assertCursor,
    assertNoWrappedRows,
    assertNeutralRegion,
    assertNoPresentedTransient,
    assertExists,
    assertNotExists,
    assertCount,
    assertMatchText,
    assertMatchStyle,
} from "./index";
/** Infrastructure/query failures propagate unchanged. Only typed mismatches pass negation. */
export declare function assertNot(expectation: () => Promise<unknown>): Promise<void>;
export declare function assertNot(expectation: () => void): void;
