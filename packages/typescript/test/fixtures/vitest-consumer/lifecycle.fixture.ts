import { afterAll, expect, test } from "vitest";
import { createTerminalTest } from "@termtrace/test/vitest";

const evidenceRoot = process.env.TERMTRACE_TEST_EVIDENCE_ROOT;
if (evidenceRoot === undefined) {
    throw new Error("TERMTRACE_TEST_EVIDENCE_ROOT is required");
}

const termtrace =
    process.env.TERMTRACE_BINARY === undefined
        ? { evidenceRoot }
        : { binaryPath: process.env.TERMTRACE_BINARY, evidenceRoot };
const terminalTest = createTerminalTest({
    termtrace,
    launch: () => ({
        command: ["/bin/sh", "-c", "printf READY; exec cat"],
        viewport: { cols: 80, rows: 24 },
        evidence: "on-failure",
    }),
});

const parallelSessions: string[] = [];
const retryRoots: string[] = [];
let retryAttempt = 0;

terminalTest("uses rich locators and Vitest snapshots", async ({ session, screen }) => {
    await session.output.waitForText("READY", { timeoutMs: 5_000 });
    await expect(screen.getByText("READY")).toExist();
    await expect(screen.getByText(/READY/)).toHaveCount(1);
    await expect(screen).not.toHaveScreenText("ABSENT");
    await expect(screen.getByText("READY")).toHaveText(/READY/u);
    await expect(screen.getByText("READY")).not.toHaveText(/EAD/u);
    await expect(screen.getByText("READY")).not.toHaveNeutralRegion();
    await expect(screen).toHaveNoWrappedRows();
    expect(() => screen.getByText(/(?:)/)).toThrowError(
        expect.objectContaining({
            code: "invalid_query",
        }),
    );
    await expect(screen).toMatchTerminalSnapshot("text", { includeCursor: true }, "ready terminal");
});

terminalTest.concurrent("isolates parallel session one", async ({ session }) => {
    parallelSessions.push(session.sessionId);
    await expect(session).toHaveScreenText("READY");
});

terminalTest.concurrent("isolates parallel session two", async ({ session }) => {
    parallelSessions.push(session.sessionId);
    await expect(session).toHaveScreenText("READY");
});

terminalTest("isolates retry evidence", { retry: 1 }, async ({ evidenceRoot: attemptRoot }) => {
    retryRoots.push(attemptRoot);
    retryAttempt += 1;
    if (retryAttempt === 1) {
        throw new Error("EXPECTED_FIRST_ATTEMPT_FAILURE");
    }
    expect(new Set(retryRoots).size).toBe(2);
});

afterAll(() => {
    expect(new Set(parallelSessions).size).toBe(2);
});

test("ordinary Vitest remains available", () => {
    expect(true).toBe(true);
});
