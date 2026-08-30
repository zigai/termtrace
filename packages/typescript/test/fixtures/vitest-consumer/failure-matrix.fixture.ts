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
    launch: {
        command: ["/bin/sh", "-c", "trap 'exit 0' TERM; printf READY; while :; do sleep .1; done"],
        evidence: "on-failure",
    },
});

terminalTest("retains ordinary failure artifacts", async ({ session }) => {
    await session.output.waitForText("READY", { timeoutMs: 5_000 });
    throw new Error("PRIMARY_FAILURE_SENTINEL");
});

terminalTest("preserves primary failure when artifacts and cleanup fail", async ({ client }) => {
    await client.close();
    throw new Error("PRIMARY_WITH_CLEANUP_SENTINEL");
});

terminalTest("turns cleanup failure after pass into failure", async ({ client }) => {
    await client.close();
});

terminalTest("retains timeout evidence", { timeout: 100 }, async ({ signal }) => {
    await new Promise<void>((resolvePromise) =>
        signal.addEventListener("abort", () => resolvePromise(), { once: true }),
    );
});
