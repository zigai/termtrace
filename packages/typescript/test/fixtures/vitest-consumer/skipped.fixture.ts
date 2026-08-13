import { existsSync } from "node:fs";
import { resolve } from "node:path";

import { expect, test } from "vitest";
import { createTerminalTest } from "@termtrace/test/vitest";

const evidenceRoot = resolve(
    process.env.TERMTRACE_TEST_EVIDENCE_ROOT ?? ".termtrace-skipped-probe",
);
const unreachableTest = createTerminalTest({
    termtrace: {
        binaryPath: "/termtrace-adapter-must-not-launch",
        evidenceRoot,
    },
    launch: {
        command: ["/termtrace-fixture-must-not-launch"],
    },
});

unreachableTest.skip("skipped tests do not launch", () => {
    throw new Error("skipped body ran");
});

unreachableTest.todo("todo tests do not launch");

test("collection did not create evidence", () => {
    expect(existsSync(evidenceRoot)).toBe(false);
});
