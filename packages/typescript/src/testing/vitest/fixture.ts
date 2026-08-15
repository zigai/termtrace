import { createHash } from "node:crypto";
import { basename, join, resolve } from "node:path";
import { test, type TestContext } from "vitest";

import type { Session } from "../../session/session.js";
import { Termtrace } from "../../termtrace.js";
import { closeAfterSetupFailure, finishHookTimeout, finishTerminalTest } from "./lifecycle.js";
import type {
    CreateTerminalTestOptions,
    MutableArtifacts,
    TerminalLaunchOptions,
    TerminalTestApi,
    TerminalTestArtifacts,
    TerminalTestContext,
    TerminalTestFixtures,
    TerminalTestTask,
} from "./types.js";

const MAX_PATH_PART_LENGTH = 64;

/**
 * Create an isolated Vitest API. The adapter starts no process until a runnable test's
 * automatic fixture begins, and owns one driver and one launched session per attempt.
 */
export function createTerminalTest(options: CreateTerminalTestOptions): TerminalTestApi {
    validateOptions(options);
    const operationTimeoutMs = options.operationTimeoutMs ?? 10_000;
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
                    await finishTerminalTest({
                        session,
                        client,
                        evidenceRoot,
                        artifacts: mutableArtifacts,
                        failureArtifacts: options.failureArtifacts,
                        operationTimeoutMs,
                        evidencePolicy,
                        bodyFailed,
                        primaryWasCaught,
                        primaryFailure,
                    });
                }, finishHookTimeout(operationTimeoutMs));

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
        worker: boundedTaskPart(workerSource),
        retry,
        repeat,
    });
}

function evidenceRootFor(callerRoot: string, task: TerminalTestTask): string {
    const moduleIdentity = `${boundedTaskPart(basename(task.filePath))}-${digest(task.filePath)}`;
    const testIdentity = `${boundedTaskPart(task.name)}-${digest(task.id)}`;
    return join(
        resolve(callerRoot),
        "vitest",
        task.worker,
        moduleIdentity,
        testIdentity,
        `retry-${task.retry}-repeat-${task.repeat}`,
    );
}

function boundedTaskPart(value: string): string {
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
