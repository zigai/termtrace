import { createHash } from "node:crypto";
import { lstat, mkdir, rm } from "node:fs/promises";
import { join, relative, resolve, sep } from "node:path";

import type { Session } from "../../session/session.js";
import type { Termtrace } from "../../termtrace.js";
import type { FailureArtifactOptions, MutableArtifacts } from "./types.js";

const DEFAULT_SCREENSHOT_BYTES = 4 * 1024 * 1024;
const FINISH_HOOK_OVERHEAD_MS = 5_000;
const MAX_TIMER_MS = 2_147_483_647;
const FAILURE_CLEANUP_OPERATION_COUNT = 5;
const DEFAULT_BUNDLE_BYTES = 32 * 1024 * 1024;

export function finishHookTimeout(operationTimeoutMs: number): number {
    return Math.min(
        MAX_TIMER_MS,
        operationTimeoutMs * FAILURE_CLEANUP_OPERATION_COUNT + FINISH_HOOK_OVERHEAD_MS,
    );
}

export async function closeAfterSetupFailure(client: Termtrace, cause: unknown): Promise<void> {
    try {
        await client.close();
    } catch (cleanupFailure: unknown) {
        throwPrimary(cause, [cleanupFailure], undefined);
    }
}

export async function finishTerminalTest(input: {
    readonly session: Session;
    readonly client: Termtrace;
    readonly evidenceRoot: string;
    readonly artifacts: MutableArtifacts;
    readonly failureArtifacts: FailureArtifactOptions | undefined;
    readonly operationTimeoutMs: number;
    readonly evidencePolicy: "always" | "on-failure" | "manual";
    readonly bodyFailed: boolean;
    readonly primaryWasCaught: boolean;
    readonly primaryFailure: unknown;
}): Promise<void> {
    const secondaryFailures: unknown[] = [];
    const completionSucceeded = await completeSession(
        input.session,
        secondaryFailures,
        input.operationTimeoutMs,
    );
    if (input.bodyFailed) {
        secondaryFailures.push(
            ...(await captureFailureArtifacts(
                input.session,
                input.evidenceRoot,
                input.artifacts,
                input.failureArtifacts,
                input.operationTimeoutMs,
            )),
        );
    }
    if (completionSucceeded) {
        try {
            await input.session.finish(!input.bodyFailed);
        } catch (cause: unknown) {
            secondaryFailures.push(cause);
        }
    }
    try {
        await input.client.close();
    } catch (cause: unknown) {
        secondaryFailures.push(cause);
    }
    if (
        !input.bodyFailed &&
        secondaryFailures.length === 0 &&
        input.evidencePolicy === "on-failure"
    ) {
        try {
            await rm(input.evidenceRoot, {
                recursive: true,
                force: true,
                maxRetries: 5,
                retryDelay: 20,
            });
        } catch (cause: unknown) {
            secondaryFailures.push(cause);
        }
    }

    if (input.bodyFailed && input.primaryWasCaught) {
        throwPrimary(input.primaryFailure, secondaryFailures, input.artifacts);
    }
    if (input.bodyFailed) {
        const diagnostic = artifactDiagnostic(input.artifacts);
        if (secondaryFailures.length > 0) {
            throw lifecycleError(
                "Secondary Termtrace failure after a Vitest-reported test failure",
                secondaryFailures,
                input.artifacts,
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
    options: FailureArtifactOptions | undefined,
    operationTimeoutMs: number,
): Promise<unknown[]> {
    const failures: unknown[] = [];
    const artifactRoot = join(
        evidenceRoot,
        `session-${boundedArtifactPart(session.sessionId)}`,
        "failure",
    );
    await collectFailure(failures, async () => mkdir(artifactRoot, { recursive: true, mode: 0o700 }));

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
        const bytes = bundle.manifest.members.reduce((total, member) => total + member.byte_length, 0);
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
                throw new Error("failure bundle manifest contains a path outside its committed directory");
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

async function collectFailure<Result>(
    failures: unknown[],
    operation: () => Promise<Result>,
): Promise<void> {
    try {
        await operation();
    } catch (cause: unknown) {
        failures.push(cause);
    }
}

function throwPrimary(
    cause: unknown,
    secondaryFailures: readonly unknown[],
    artifacts: MutableArtifacts | undefined,
): never {
    if (!(cause instanceof Error)) {
        throw new AggregateError([cause, ...secondaryFailures], "Terminal test failed", {
            cause,
        });
    }
    try {
        const diagnostic = artifactDiagnostic(artifacts);
        if (diagnostic.length > 0 && !cause.message.includes(diagnostic)) {
            cause.message = `${cause.message}\n${diagnostic}`;
        }
        if (secondaryFailures.length > 0) {
            Object.defineProperty(cause, "termtraceSecondaryFailures", {
                configurable: true,
                enumerable: false,
                value: Object.freeze([...secondaryFailures]),
            });
            if (cause.cause === undefined) {
                Object.defineProperty(cause, "cause", {
                    configurable: true,
                    enumerable: false,
                    value: secondaryFailures[0],
                });
            }
        }
    } catch {
        // Frozen or exotic Error objects must still remain the primary thrown value.
    }
    throw cause;
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

function boundedArtifactPart(value: string): string {
    const normalized = value
        .normalize("NFKC")
        .replace(/[^A-Za-z0-9._-]+/gu, "-")
        .replace(/^[._-]+|[._-]+$/gu, "");
    const prefix = normalized.length === 0 ? "unnamed" : normalized.slice(0, 64);
    return `${prefix}-${digest(value)}`;
}

function digest(value: string): string {
    return createHash("sha256").update(value).digest("hex").slice(0, 12);
}
