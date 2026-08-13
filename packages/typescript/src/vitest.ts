import { createHash } from "node:crypto";
import { lstat, mkdir, rm } from "node:fs/promises";
import { basename, join, relative, resolve, sep } from "node:path";
import {
    AssertionMismatchError,
    Locator,
    MultipleMatchesError,
    NoMatchError,
    Screen,
    ScreenFrame,
    Session,
    Termtrace,
    allOf,
    assertCursor,
    assertNoPresentedTransient,
    assertScreenText,
    evaluateQuery,
    normalizeFrame,
    styleQuery,
    type ColorSource,
    type LaunchOptions,
    type Rgb,
    type ScreenCell,
    type ScreenMatch,
    type SnapshotOptions,
    type SnapshotProfile,
    type StyleFilter,
    type TermtraceOptions,
    type WideKind,
} from "./index.js";
import { Snapshots, expect, test, type MatcherState, type TestAPI, type TestContext } from "vitest";

const MAX_PATH_PART_LENGTH = 64;
const DEFAULT_SCREENSHOT_BYTES = 4 * 1024 * 1024;
const FINISH_HOOK_OVERHEAD_MS = 5_000;
const MAX_TIMER_MS = 2_147_483_647;
const FAILURE_CLEANUP_OPERATION_COUNT = 5;
const DEFAULT_BUNDLE_BYTES = 32 * 1024 * 1024;

type TerminalLaunchOptions = Omit<LaunchOptions, "evidenceRoot">;

export type TerminalTestTask = {
    readonly id: string;
    readonly name: string;
    readonly filePath: string;
    readonly worker: string;
    readonly retry: number;
    readonly repeat: number;
};

export type TerminalTestArtifacts = {
    readonly canonicalSession: string;
    readonly semanticScreenshot: string | undefined;
    readonly bundle: string | undefined;
    readonly verificationValid: boolean | undefined;
};

export type TerminalTestContext = {
    readonly client: Termtrace;
    readonly session: Session;
    readonly screen: Screen;
    readonly evidenceRoot: string;
    readonly task: TerminalTestTask;
    readonly artifacts: TerminalTestArtifacts;
};

export type TerminalTestFixtures = {
    readonly terminal: TerminalTestContext;
    readonly client: Termtrace;
    readonly session: Session;
    readonly screen: Screen;
    readonly evidenceRoot: string;
    readonly artifacts: TerminalTestArtifacts;
};

export type CreateTerminalTestOptions = {
    readonly termtrace: Omit<TermtraceOptions, "evidenceRoot"> & {
        readonly evidenceRoot: string;
    };
    readonly launch:
        | TerminalLaunchOptions
        | ((context: {
              readonly task: TerminalTestTask;
          }) => TerminalLaunchOptions | Promise<TerminalLaunchOptions>);
    readonly failureArtifacts?: {
        readonly snapshotProfile?: SnapshotProfile;
        readonly snapshotOptions?: SnapshotOptions;
        readonly bundleProfile?: "screen" | "debug" | "forensic";
        readonly maxScreenshotBytes?: number;
        readonly maxBundleBytes?: number;
    };
    readonly operationTimeoutMs?: number;
};

type MutableArtifacts = {
    canonicalSession: string;
    semanticScreenshot?: string;
    bundle?: string;
    verificationValid?: boolean;
};

/**
 * Create an isolated Vitest API. The adapter starts no process until a runnable test's
 * automatic fixture begins, and owns one driver and one launched session per attempt.
 */
export function createTerminalTest(
    options: CreateTerminalTestOptions,
): TestAPI<TerminalTestFixtures> {
    validateOptions(options);
    const operationTimeoutMs = options.operationTimeoutMs ?? 10_000;
    const finishHookTimeoutMs = Math.min(
        MAX_TIMER_MS,
        operationTimeoutMs * FAILURE_CLEANUP_OPERATION_COUNT + FINISH_HOOK_OVERHEAD_MS,
    );
    return test.extend<TerminalTestFixtures>({
        terminal: [
            async ({ task, signal, onTestFinished }, use) => {
                const taskIdentity = identifyTask(task);
                const evidenceRoot = evidenceRootFor(options.termtrace.evidenceRoot, taskIdentity);
                const launch = await resolveLaunch(options.launch, taskIdentity);
                const evidencePolicy = launch.evidence ?? "on-failure";
                const client = await Termtrace.create({
                    ...(options.termtrace.binaryPath === undefined
                        ? {}
                        : { binaryPath: options.termtrace.binaryPath }),
                    evidenceRoot,
                });
                let session: Session;
                try {
                    signal.throwIfAborted();
                    session = await client.launch({
                        ...launch,
                        evidenceRoot,
                        evidence: evidencePolicy,
                    });
                } catch (cause: unknown) {
                    await closeAfterSetupFailure(client, cause);
                    throw cause;
                }

                const mutableArtifacts: MutableArtifacts = {
                    canonicalSession: session.canonicalSession,
                };
                const artifacts: TerminalTestArtifacts = Object.freeze({
                    get canonicalSession() {
                        return mutableArtifacts.canonicalSession;
                    },
                    get semanticScreenshot() {
                        return mutableArtifacts.semanticScreenshot;
                    },
                    get bundle() {
                        return mutableArtifacts.bundle;
                    },
                    get verificationValid() {
                        return mutableArtifacts.verificationValid;
                    },
                });
                const context: TerminalTestContext = Object.freeze({
                    client,
                    session,
                    screen: session.screen,
                    evidenceRoot,
                    task: taskIdentity,
                    artifacts,
                });

                let primaryWasCaught = false;
                let primaryFailure: unknown;
                let finalized = false;
                onTestFinished(async (finishedContext) => {
                    if (finalized) return;
                    finalized = true;
                    const frameworkReportedFailure = finishedContext.task.result?.state === "fail";
                    const bodyFailed =
                        frameworkReportedFailure || primaryWasCaught || signal.aborted;
                    if (signal.aborted && !primaryWasCaught) {
                        primaryWasCaught = true;
                        primaryFailure = signal.reason;
                    }

                    const secondaryFailures: unknown[] = [];
                    const completionSucceeded = await completeSession(
                        session,
                        secondaryFailures,
                        operationTimeoutMs,
                    );
                    if (bodyFailed) {
                        secondaryFailures.push(
                            ...(await captureFailureArtifacts(
                                session,
                                evidenceRoot,
                                mutableArtifacts,
                                options.failureArtifacts,
                                operationTimeoutMs,
                            )),
                        );
                    }
                    if (completionSucceeded) {
                        try {
                            await session.finish(!bodyFailed);
                        } catch (cause: unknown) {
                            secondaryFailures.push(cause);
                        }
                    }
                    try {
                        await client.close();
                    } catch (cause: unknown) {
                        secondaryFailures.push(cause);
                    }
                    if (
                        !bodyFailed &&
                        secondaryFailures.length === 0 &&
                        evidencePolicy === "on-failure"
                    ) {
                        try {
                            await rm(evidenceRoot, {
                                recursive: true,
                                force: true,
                                maxRetries: 5,
                                retryDelay: 20,
                            });
                        } catch (cause: unknown) {
                            secondaryFailures.push(cause);
                        }
                    }

                    if (bodyFailed && primaryWasCaught) {
                        throwPrimary(primaryFailure, secondaryFailures, mutableArtifacts);
                    }
                    if (bodyFailed) {
                        const diagnostic = artifactDiagnostic(mutableArtifacts);
                        if (secondaryFailures.length > 0) {
                            throw lifecycleError(
                                "Secondary Termtrace failure after a Vitest-reported test failure",
                                secondaryFailures,
                                mutableArtifacts,
                            );
                        }
                        if (diagnostic.length > 0) {
                            throw new Error(diagnostic);
                        }
                        return;
                    }
                    if (secondaryFailures.length > 0) {
                        throw lifecycleError("Termtrace test cleanup failed", secondaryFailures);
                    }
                }, finishHookTimeoutMs);

                try {
                    await use(context);
                } catch (cause: unknown) {
                    primaryWasCaught = true;
                    primaryFailure = cause;
                }
            },
            { auto: true },
        ],
        client: async ({ terminal }, use) => use(terminal.client),
        session: async ({ terminal }, use) => use(terminal.session),
        screen: async ({ terminal }, use) => use(terminal.screen),
        evidenceRoot: async ({ terminal }, use) => use(terminal.evidenceRoot),
        artifacts: async ({ terminal }, use) => use(terminal.artifacts),
    });
}

async function resolveLaunch(
    launch: CreateTerminalTestOptions["launch"],
    taskIdentity: TerminalTestTask,
): Promise<TerminalLaunchOptions> {
    const resolved = typeof launch === "function" ? await launch({ task: taskIdentity }) : launch;
    if (resolved.command.length === 0) {
        throw new TypeError("terminal test launch command must not be empty");
    }
    return resolved;
}

function identifyTask(task: TestContext["task"]): TerminalTestTask {
    const retry = task.result?.retryCount ?? 0;
    const repeat = task.result?.repeatCount ?? 0;
    const workerSource = [
        process.env.VITEST_POOL_ID,
        process.env.VITEST_WORKER_ID,
        process.env.VITEST_POOL_SIZE,
        String(process.pid),
    ]
        .filter((value): value is string => value !== undefined)
        .join(":");
    return Object.freeze({
        id: task.id,
        name: task.name,
        filePath: task.file.filepath,
        worker: boundedPart(workerSource),
        retry,
        repeat,
    });
}

function evidenceRootFor(callerRoot: string, task: TerminalTestTask): string {
    const moduleIdentity = `${boundedPart(basename(task.filePath))}-${digest(task.filePath)}`;
    const testIdentity = `${boundedPart(task.name)}-${digest(task.id)}`;
    return join(
        resolve(callerRoot),
        "vitest",
        task.worker,
        moduleIdentity,
        testIdentity,
        `retry-${task.retry}-repeat-${task.repeat}`,
    );
}

function boundedPart(value: string): string {
    const normalized = value
        .normalize("NFKC")
        .replace(/[^A-Za-z0-9._-]+/gu, "-")
        .replace(/^[._-]+|[._-]+$/gu, "");
    const prefix = normalized.length === 0 ? "unnamed" : normalized.slice(0, MAX_PATH_PART_LENGTH);
    return `${prefix}-${digest(value)}`;
}

function digest(value: string): string {
    return createHash("sha256").update(value).digest("hex").slice(0, 12);
}

async function closeAfterSetupFailure(client: Termtrace, primaryFailure: unknown): Promise<void> {
    try {
        await client.close();
    } catch (cleanupFailure: unknown) {
        throwPrimary(primaryFailure, [cleanupFailure], undefined);
    }
}

async function completeSession(
    session: Session,
    failures: unknown[],
    operationTimeoutMs: number,
): Promise<boolean> {
    let stopFailed = false;
    let stopFailure: unknown;
    try {
        await session.stop({ requestTimeoutMs: operationTimeoutMs });
    } catch (cause: unknown) {
        stopFailed = true;
        stopFailure = cause;
    }
    try {
        await session.evidence.complete({
            timeoutMs: operationTimeoutMs,
            requestTimeoutMs: operationTimeoutMs,
        });
        return true;
    } catch (cause: unknown) {
        if (stopFailed) failures.push(stopFailure);
        failures.push(cause);
        return false;
    }
}

async function captureFailureArtifacts(
    session: Session,
    evidenceRoot: string,
    artifacts: MutableArtifacts,
    options: CreateTerminalTestOptions["failureArtifacts"],
    operationTimeoutMs: number,
): Promise<unknown[]> {
    const failures: unknown[] = [];
    const artifactRoot = join(evidenceRoot, `session-${boundedPart(session.sessionId)}`, "failure");
    await collectFailure(failures, async () =>
        mkdir(artifactRoot, { recursive: true, mode: 0o700 }),
    );

    await collectFailure(failures, async () => {
        const destination = join(artifactRoot, "semantic-screenshot");
        const screenshot = await session.screen.screenshot(
            destination,
            { selector: "latest_presented" },
            operationTimeoutMs,
        );
        const primary = resolve(screenshot.primary);
        const relativePrimary = relative(resolve(destination), primary);
        if (
            relativePrimary.length === 0 ||
            relativePrimary === ".." ||
            relativePrimary.startsWith(`..${sep}`)
        ) {
            throw new Error("semantic screenshot escaped its committed artifact directory");
        }
        const metadata = await lstat(primary);
        const maximum = options?.maxScreenshotBytes ?? DEFAULT_SCREENSHOT_BYTES;
        if (!metadata.isFile() || metadata.size > maximum) {
            throw new RangeError(`semantic screenshot exceeds the ${maximum}-byte adapter limit`);
        }
        artifacts.semanticScreenshot = primary;
    });

    await collectFailure(failures, async () => {
        const verification = await session.evidence.verify("content", operationTimeoutMs);
        artifacts.verificationValid = verification.valid;
        if (!verification.valid) {
            throw new Error(
                `canonical evidence verification failed with ${verification.findings.length} finding(s)`,
            );
        }
    });
    await collectFailure(failures, async () => {
        const destination = join(artifactRoot, "bundle");
        const bundle = await session.evidence.bundle(
            destination,
            options?.bundleProfile ?? "debug",
            operationTimeoutMs,
        );
        const bytes = bundle.manifest.members.reduce(
            (total, member) => total + member.byte_length,
            0,
        );
        const maximum = options?.maxBundleBytes ?? DEFAULT_BUNDLE_BYTES;
        if (!Number.isSafeInteger(bytes) || bytes > maximum) {
            throw new RangeError(`failure bundle exceeds the ${maximum}-byte adapter limit`);
        }
        const committedRoot = resolve(bundle.directory);
        const metadata = await lstat(committedRoot);
        if (!metadata.isDirectory()) {
            throw new Error("failure bundle verification did not find the committed directory");
        }
        for (const member of bundle.manifest.members) {
            const memberPath = resolve(committedRoot, member.path);
            const localPath = relative(committedRoot, memberPath);
            if (localPath.length === 0 || localPath === ".." || localPath.startsWith(`..${sep}`)) {
                throw new Error(
                    "failure bundle manifest contains a path outside its committed directory",
                );
            }
            const memberMetadata = await lstat(memberPath);
            if (!memberMetadata.isFile() || memberMetadata.size !== member.byte_length) {
                throw new Error(
                    `failure bundle member verification failed for ${member.path.slice(0, 240)}`,
                );
            }
        }
        artifacts.bundle = bundle.directory;
    });
    return failures;
}

async function collectFailure(
    failures: unknown[],
    operation: () => Promise<unknown>,
): Promise<void> {
    try {
        await operation();
    } catch (cause: unknown) {
        failures.push(cause);
    }
}

function throwPrimary(
    primaryFailure: unknown,
    secondaryFailures: readonly unknown[],
    artifacts: MutableArtifacts | undefined,
): never {
    if (!(primaryFailure instanceof Error)) {
        throw new AggregateError([primaryFailure, ...secondaryFailures], "Terminal test failed", {
            cause: primaryFailure,
        });
    }
    try {
        const diagnostic = artifactDiagnostic(artifacts);
        if (diagnostic.length > 0 && !primaryFailure.message.includes(diagnostic)) {
            primaryFailure.message = `${primaryFailure.message}\n${diagnostic}`;
        }
        if (secondaryFailures.length > 0) {
            Object.defineProperty(primaryFailure, "termtraceSecondaryFailures", {
                configurable: true,
                enumerable: false,
                value: Object.freeze([...secondaryFailures]),
            });
            if (primaryFailure.cause === undefined) {
                Object.defineProperty(primaryFailure, "cause", {
                    configurable: true,
                    enumerable: false,
                    value: secondaryFailures[0],
                });
            }
        }
    } catch {
        // Frozen or exotic Error objects must still remain the primary thrown value.
    }
    throw primaryFailure;
}

function lifecycleError(
    message: string,
    failures: readonly unknown[],
    artifacts?: MutableArtifacts,
): Error {
    const diagnostic = artifactDiagnostic(artifacts);
    const detailedMessage = diagnostic.length === 0 ? message : `${message}\n${diagnostic}`;
    const error = new Error(detailedMessage, {
        cause: new AggregateError(failures, message),
    });
    Object.defineProperty(error, "termtraceSecondaryFailures", {
        configurable: true,
        enumerable: false,
        value: Object.freeze([...failures]),
    });
    return error;
}

function artifactDiagnostic(artifacts: MutableArtifacts | undefined): string {
    if (artifacts === undefined) {
        return "";
    }
    const values = [
        `Termtrace evidence: ${artifacts.canonicalSession}`,
        artifacts.semanticScreenshot === undefined
            ? undefined
            : `Semantic screenshot: ${artifacts.semanticScreenshot}`,
        artifacts.bundle === undefined ? undefined : `Failure bundle: ${artifacts.bundle}`,
    ].filter((value): value is string => value !== undefined);
    return values.join("\n").slice(0, 2048);
}

function validateOptions(options: CreateTerminalTestOptions): void {
    if (options.termtrace.evidenceRoot.length === 0) {
        throw new TypeError("terminal test evidenceRoot must not be empty");
    }
    positiveLimit(options.failureArtifacts?.maxScreenshotBytes, "maxScreenshotBytes");
    positiveLimit(options.failureArtifacts?.maxBundleBytes, "maxBundleBytes");
    positiveLimit(options.operationTimeoutMs, "operationTimeoutMs");
}

function positiveLimit(value: number | undefined, name: string): void {
    if (value !== undefined && (!Number.isSafeInteger(value) || value <= 0)) {
        throw new RangeError(`${name} must be a positive safe integer`);
    }
}

type TerminalMatcherResult = {
    readonly pass: boolean;
    readonly message: () => string;
};

export type TerminalCursorExpectation = {
    readonly row?: number;
    readonly column?: number;
    readonly visible?: boolean;
};

type TerminalMatchers = {
    readonly toHaveScreenText: (
        received: unknown,
        expected: string | RegExp,
    ) => Promise<TerminalMatcherResult>;
    readonly toHaveCursor: (
        received: unknown,
        expected: TerminalCursorExpectation,
    ) => Promise<TerminalMatcherResult>;
    readonly toExist: (received: unknown) => Promise<TerminalMatcherResult>;
    readonly toHaveCount: (received: unknown, expected: number) => Promise<TerminalMatcherResult>;
    readonly toHaveText: (
        received: unknown,
        expected: string | RegExp,
    ) => Promise<TerminalMatcherResult>;
    readonly toHaveStyle: (
        received: unknown,
        expected: StyleFilter,
    ) => Promise<TerminalMatcherResult>;
    readonly toHaveNoWrappedRows: (received: unknown) => Promise<TerminalMatcherResult>;
    readonly toHaveNeutralRegion: (received: unknown) => Promise<TerminalMatcherResult>;
    readonly toHaveNoPresentedTransient: (received: unknown) => Promise<TerminalMatcherResult>;
    readonly toMatchTerminalSnapshot: (
        this: MatcherState,
        received: unknown,
        profile?: SnapshotProfile,
        options?: SnapshotOptions,
        hint?: string,
    ) => Promise<TerminalMatcherResult>;
};

export const terminalMatchers: TerminalMatchers = {
    async toHaveScreenText(received, expected) {
        const subject =
            received instanceof Screen
                ? await received.frame({ detail: "spans" })
                : requireScreenAssertionSubject(received);
        return assertionResult(
            () => assertScreenText(subject, expected),
            `expected screen not to contain ${String(expected)}`,
        );
    },
    async toHaveCursor(received, expected) {
        const subject =
            received instanceof Screen
                ? await received.frame({ detail: "spans" })
                : requireScreenAssertionSubject(received);
        return assertionResult(
            () => assertCursor(subject, expected),
            "expected cursor not to match",
        );
    },
    async toExist(received) {
        const matches = await requireLocator(received).all();
        return result(
            matches.length > 0,
            `expected locator to resolve, but it matched ${matches.length} range(s)`,
            "expected locator not to resolve",
        );
    },
    async toHaveCount(received, expected) {
        if (!Number.isSafeInteger(expected) || expected < 0) {
            throw new RangeError("expected locator count must be a nonnegative safe integer");
        }
        const matches = await requireLocator(received).all();
        return result(
            matches.length === expected,
            `expected ${expected} match(es), received ${matches.length}`,
            `expected count not to be ${expected}`,
        );
    },
    async toHaveText(received, expected) {
        const matches = await requireLocator(received).all();
        const matched = matches.length === 1 && textMatches(matches[0]?.text ?? "", expected);
        return result(
            matched,
            locatorMismatch("text", matches),
            `expected strict locator not to have text ${String(expected)}`,
        );
    },
    async toHaveStyle(received, expected) {
        const locator = requireLocator(received);
        const matches = await locator.all();
        const match = matches[0];
        const styled =
            match !== undefined &&
            matches.length === 1 &&
            evaluateQuery(match.frame, allOf(locator.spec, styleQuery(expected))).length === 1;
        return result(
            styled,
            locatorMismatch("style", matches),
            "expected strict locator not to have the requested style",
        );
    },
    async toHaveNoWrappedRows(received) {
        const frame = await resolveFrame(received);
        const wrapped = frame.rows.filter((row) => row.softWrapped || row.wrapContinuation);
        return result(
            wrapped.length === 0,
            `expected no wrapped rows, received ${wrapped.length}`,
            "expected at least one wrapped row",
        );
    },
    async toHaveNeutralRegion(received) {
        const matches = await requireLocator(received).all();
        const neutral = matches.length > 0 && matches.every(matchIsNeutral);
        return result(
            neutral,
            `expected a nonempty neutral/default region, received ${matches.length} non-neutral or absent match(es)`,
            "expected region not to be neutral/default styled",
        );
    },
    async toHaveNoPresentedTransient(received) {
        const session = requireSession(received);
        return assertionResult(
            () => assertNoPresentedTransient(session),
            "expected a presented transient diagnostic",
        );
    },
    async toMatchTerminalSnapshot(received, profile = "spans", options = {}, hint) {
        const frame = await resolveFrame(received);
        const normalized = normalizeFrame(frame, profile, options);
        return Snapshots.toMatchSnapshot.call(this, normalized, hint);
    },
};
const vitestMatcherRegistry = {
    toHaveScreenText(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveScreenText.call(
            this,
            received,
            parseTextExpectation(expected),
        );
    },
    toHaveCursor(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveCursor.call(this, received, parseCursorExpectation(expected));
    },
    toExist(this: MatcherState, received: unknown) {
        return terminalMatchers.toExist.call(this, received);
    },
    toHaveCount(this: MatcherState, received: unknown, expected: unknown) {
        if (typeof expected !== "number" || !Number.isSafeInteger(expected) || expected < 0) {
            throw new TypeError("matcher count must be a nonnegative safe integer");
        }
        return terminalMatchers.toHaveCount.call(this, received, expected);
    },
    toHaveText(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveText.call(this, received, parseTextExpectation(expected));
    },
    toHaveStyle(this: MatcherState, received: unknown, expected: unknown) {
        return terminalMatchers.toHaveStyle.call(this, received, parseStyleExpectation(expected));
    },
    toHaveNoWrappedRows(this: MatcherState, received: unknown) {
        return terminalMatchers.toHaveNoWrappedRows.call(this, received);
    },
    toHaveNeutralRegion(this: MatcherState, received: unknown) {
        return terminalMatchers.toHaveNeutralRegion.call(this, received);
    },
    toHaveNoPresentedTransient(this: MatcherState, received: unknown) {
        return terminalMatchers.toHaveNoPresentedTransient.call(this, received);
    },
    toMatchTerminalSnapshot(
        this: MatcherState,
        received: unknown,
        profile?: unknown,
        options?: unknown,
        hint?: unknown,
    ) {
        if (hint !== undefined && typeof hint !== "string") {
            throw new TypeError("snapshot hint must be a string");
        }
        return terminalMatchers.toMatchTerminalSnapshot.call(
            this,
            received,
            parseSnapshotProfile(profile),
            parseSnapshotOptions(options),
            hint,
        );
    },
};
expect.extend(vitestMatcherRegistry);

function parseTextExpectation(input: unknown): string | RegExp {
    if (typeof input === "string" || input instanceof RegExp) {
        return input;
    }
    throw new TypeError("text expectation must be a string or RegExp");
}

function parseCursorExpectation(input: unknown): TerminalCursorExpectation {
    const value = matcherObject(input, "cursor expectation");
    const row = optionalNonnegativeInteger(value.row, "cursor row");
    const column = optionalNonnegativeInteger(value.column, "cursor column");
    const visible = optionalBoolean(value.visible, "cursor visibility");
    return Object.freeze({
        ...(row === undefined ? {} : { row }),
        ...(column === undefined ? {} : { column }),
        ...(visible === undefined ? {} : { visible }),
    });
}

function parseStyleExpectation(input: unknown): StyleFilter {
    const value = matcherObject(input, "style expectation");
    const wideKind = optionalWideKind(value.wideKind);
    const underline = optionalNonnegativeInteger(value.underline, "underline");
    const foreground = optionalColorMatcher(value.foreground, "foreground");
    const background = optionalColorMatcher(value.background, "background");
    const underlineColor = optionalColorSource(value.underlineColor, "underline");
    const resolvedForeground = optionalRgbMatcher(value.resolvedForeground, "resolved foreground");
    const resolvedBackground = optionalRgbMatcher(value.resolvedBackground, "resolved background");
    return Object.freeze({
        ...optionalBooleanField(value, "bold"),
        ...optionalBooleanField(value, "faint"),
        ...optionalBooleanField(value, "italic"),
        ...optionalBooleanField(value, "blink"),
        ...optionalBooleanField(value, "inverse"),
        ...optionalBooleanField(value, "invisible"),
        ...optionalBooleanField(value, "strikethrough"),
        ...optionalBooleanField(value, "overline"),
        ...optionalBooleanField(value, "selected"),
        ...(underline === undefined ? {} : { underline }),
        ...(wideKind === undefined ? {} : { wideKind }),
        ...(foreground === undefined ? {} : { foreground }),
        ...(background === undefined ? {} : { background }),
        ...(underlineColor === undefined ? {} : { underlineColor }),
        ...(resolvedForeground === undefined ? {} : { resolvedForeground }),
        ...(resolvedBackground === undefined ? {} : { resolvedBackground }),
    });
}

function parseSnapshotProfile(input: unknown): SnapshotProfile {
    if (input === undefined || input === "spans") {
        return "spans";
    }
    if (input === "text" || input === "cells") {
        return input;
    }
    throw new TypeError("snapshot profile must be text, spans, or cells");
}

function parseSnapshotOptions(input: unknown): SnapshotOptions {
    if (input === undefined) {
        return Object.freeze({});
    }
    const value = matcherObject(input, "snapshot options");
    const includeCursor = optionalBoolean(value.includeCursor, "includeCursor");
    const includeModes = optionalBoolean(value.includeModes, "includeModes");
    const maxCells = optionalNonnegativeInteger(value.maxCells, "maxCells");
    if (maxCells === 0) {
        throw new TypeError("maxCells must be positive");
    }
    return Object.freeze({
        ...(includeCursor === undefined ? {} : { includeCursor }),
        ...(includeModes === undefined ? {} : { includeModes }),
        ...(maxCells === undefined ? {} : { maxCells }),
    });
}

function matcherObject(input: unknown, label: string): Readonly<Record<string, unknown>> {
    if (typeof input !== "object" || input === null || Array.isArray(input)) {
        throw new TypeError(`${label} must be an object`);
    }
    return Object.fromEntries(Object.entries(input));
}

function optionalBoolean(input: unknown, label: string): boolean | undefined {
    if (input === undefined || typeof input === "boolean") {
        return input;
    }
    throw new TypeError(`${label} must be a boolean`);
}

function optionalBooleanField(
    input: Readonly<Record<string, unknown>>,
    field:
        | "bold"
        | "faint"
        | "italic"
        | "blink"
        | "inverse"
        | "invisible"
        | "strikethrough"
        | "overline"
        | "selected",
): Readonly<Record<string, boolean>> {
    const value = optionalBoolean(input[field], field);
    if (value === undefined) {
        return Object.freeze({});
    }
    return Object.freeze({ [field]: value });
}

function optionalNonnegativeInteger(input: unknown, label: string): number | undefined {
    if (input === undefined) {
        return undefined;
    }
    if (typeof input === "number" && Number.isSafeInteger(input) && input >= 0) {
        return input;
    }
    throw new TypeError(`${label} must be a nonnegative safe integer`);
}

function optionalWideKind(input: unknown): WideKind | undefined {
    if (input === undefined) {
        return undefined;
    }
    if (
        input === "narrow" ||
        input === "wide" ||
        input === "spacer_tail" ||
        input === "spacer_head"
    ) {
        return input;
    }
    throw new TypeError("wideKind has an unknown value");
}

function optionalColorMatcher(
    input: unknown,
    label: string,
): ColorSource | `#${string}` | undefined {
    if (typeof input === "string") {
        return parseHexColor(input, label);
    }
    return optionalColorSource(input, label);
}

function optionalRgbMatcher(input: unknown, label: string): Rgb | `#${string}` | null | undefined {
    if (typeof input === "string") {
        return parseHexColor(input, label);
    }
    return optionalRgb(input, label);
}

function parseHexColor(input: string, label: string): `#${string}` {
    if (!/^#[0-9a-fA-F]{6}$/u.test(input)) {
        throw new TypeError(`${label} must use #RRGGBB syntax`);
    }
    return input as `#${string}`;
}

function optionalColorSource(input: unknown, label: string): ColorSource | undefined {
    if (input === undefined) {
        return undefined;
    }
    const value = matcherObject(input, `${label} color`);
    if (value.kind === "none") {
        return Object.freeze({ kind: "none" });
    }
    if (value.kind === "palette") {
        const index = optionalNonnegativeInteger(value.value, `${label} palette index`);
        if (index === undefined || index > 255) {
            throw new TypeError(`${label} palette index must be between 0 and 255`);
        }
        return Object.freeze({ kind: "palette", value: index });
    }
    if (value.kind === "rgb") {
        return Object.freeze({ kind: "rgb", value: parseRgb(value.value, label) });
    }
    throw new TypeError(`${label} color kind is invalid`);
}

function optionalRgb(input: unknown, label: string): Rgb | null | undefined {
    if (input === undefined || input === null) {
        return input;
    }
    return parseRgb(input, label);
}

function parseRgb(input: unknown, label: string): Rgb {
    const value = matcherObject(input, label);
    const red = optionalNonnegativeInteger(value.r, `${label} red`);
    const green = optionalNonnegativeInteger(value.g, `${label} green`);
    const blue = optionalNonnegativeInteger(value.b, `${label} blue`);
    if (
        red === undefined ||
        green === undefined ||
        blue === undefined ||
        red > 255 ||
        green > 255 ||
        blue > 255
    ) {
        throw new TypeError(`${label} RGB channels must be bytes`);
    }
    return Object.freeze({ r: red, g: green, b: blue });
}

async function assertionResult(
    operation: () => Promise<void>,
    negatedMessage: string,
): Promise<TerminalMatcherResult> {
    try {
        await operation();
        return result(true, "expected Termtrace assertion to pass", negatedMessage);
    } catch (cause: unknown) {
        if (
            !(cause instanceof AssertionMismatchError) &&
            !(cause instanceof NoMatchError) &&
            !(cause instanceof MultipleMatchesError)
        ) {
            throw cause;
        }
        return result(false, cause.message, negatedMessage);
    }
}

function result(
    pass: boolean,
    failedMessage: string,
    negatedMessage: string,
): TerminalMatcherResult {
    return { pass, message: () => (pass ? negatedMessage : failedMessage) };
}

function requireLocator(received: unknown): Locator {
    if (received instanceof Locator) {
        return received;
    }
    throw new TypeError("matcher requires a Termtrace Locator");
}

function requireSession(received: unknown): Session {
    if (received instanceof Session) {
        return received;
    }
    throw new TypeError("matcher requires a Termtrace Session");
}

function requireScreenAssertionSubject(received: unknown): Session | ScreenFrame {
    if (received instanceof Session || received instanceof ScreenFrame) {
        return received;
    }
    throw new TypeError("matcher requires a Termtrace Session, Screen, or ScreenFrame");
}

async function resolveFrame(received: unknown): Promise<ScreenFrame> {
    if (received instanceof ScreenFrame) {
        return received;
    }
    if (received instanceof Session) {
        return received.screen.frame({ detail: "spans" });
    }
    if (received instanceof Screen) {
        return received.frame({ detail: "spans" });
    }
    if (received instanceof Locator) {
        const matches = await received.all();
        const first = matches[0];
        if (first !== undefined) {
            return first.frame;
        }
        throw new AssertionMismatchError("locator did not resolve a frame for snapshotting");
    }
    throw new TypeError("matcher requires a Termtrace Session, Screen, ScreenFrame, or Locator");
}

function textMatches(actual: string, expected: string | RegExp): boolean {
    if (typeof expected === "string") {
        return actual === expected;
    }
    expected.lastIndex = 0;
    const result = expected.exec(actual);
    expected.lastIndex = 0;
    return result?.index === 0 && result[0] === actual;
}

function locatorMismatch(assertion: string, matches: ReadonlyArray<ScreenMatch>): string {
    if (matches.length !== 1) {
        return `expected strict locator ${assertion}, but it resolved ${matches.length} match(es)`;
    }
    return `strict locator did not have the expected ${assertion}; received ${matches[0]?.excerpt ?? ""}`;
}

function matchIsNeutral(match: ScreenMatch): boolean {
    for (const range of match.ranges) {
        const row = match.frame.rows.find((candidate) => candidate.row === range.row);
        if (row === undefined) {
            return false;
        }
        for (let column = range.startColumn; column < range.endColumn; column += 1) {
            if (row.cells !== null) {
                const cell = row.cells.find((candidate) => candidate.column === column);
                if (
                    cell === undefined ||
                    !cellContentIsNeutral(cell.grapheme, cell.wideKind, cell.appearance)
                ) {
                    return false;
                }
                continue;
            }
            const span = row.spans.find(
                (candidate) => candidate.startColumn <= column && candidate.endColumn > column,
            );
            const cell = span?.cells[column - (span?.startColumn ?? column)];
            if (
                span === undefined ||
                cell === undefined ||
                !cellContentIsNeutral(cell.grapheme, cell.wideKind, span.appearance)
            ) {
                return false;
            }
        }
    }
    return true;
}

function cellContentIsNeutral(
    grapheme: string,
    wideKind: WideKind,
    appearance: ScreenCell["appearance"],
): boolean {
    const style = appearance.style;
    return (
        (grapheme === "" || grapheme === " ") &&
        (wideKind === "narrow" || wideKind === "spacer_head" || wideKind === "spacer_tail") &&
        style.foreground.kind === "none" &&
        style.background.kind === "none" &&
        style.underlineColor.kind === "none" &&
        !style.bold &&
        !style.faint &&
        !style.italic &&
        !style.blink &&
        !style.inverse &&
        !style.invisible &&
        !style.strikethrough &&
        !style.overline &&
        style.underline === 0 &&
        !appearance.selected &&
        appearance.resolvedForeground === null &&
        appearance.resolvedBackground === null
    );
}

declare module "vitest" {
    interface Matchers<T> {
        toHaveScreenText(expected: string | RegExp): Promise<T>;
        toHaveCursor(expected: TerminalCursorExpectation): Promise<T>;
        toExist(): Promise<T>;
        toHaveCount(expected: number): Promise<T>;
        toHaveText(expected: string | RegExp): Promise<T>;
        toHaveStyle(expected: StyleFilter): Promise<T>;
        toHaveNoWrappedRows(): Promise<T>;
        toHaveNeutralRegion(): Promise<T>;
        toHaveNoPresentedTransient(): Promise<T>;
        toMatchTerminalSnapshot(
            profile?: SnapshotProfile,
            options?: SnapshotOptions,
            hint?: string,
        ): Promise<T>;
    }
}
