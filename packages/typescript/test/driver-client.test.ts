import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import { expect, test } from "bun:test";

import { DriverClient } from "../src/transport/driver-client.js";
import { RequestTimeoutError } from "../src/errors.js";
import { TermtraceProtocolError, type DriverMethod } from "../src/transport/protocol.js";

test("rejects requests issued after the driver exits", async () => {
    const directory = mkdtempSync(join(tmpdir(), "termtrace-driver-client-"));
    const binary = join(directory, "driver");
    writeFileSync(binary, "#!/bin/sh\nIFS= read -r request\nexit 0\n");
    chmodSync(binary, 0o700);
    const client = new DriverClient(binary);

    try {
        await expect(
            client.request({ method: "hello" }, "hello", {
                requestId: "00000000-0000-0000-0000-000000000001",
            }),
        ).rejects.toThrow("driver exited");
        await expect(
            client.request({ method: "hello" }, "hello", {
                requestId: "00000000-0000-0000-0000-000000000002",
            }),
        ).rejects.toThrow("driver is closed");
    } finally {
        await client.close();
        rmSync(directory, { recursive: true, force: true });
    }
});

test("rejects malformed runtime input without consuming request correlation", async () => {
    const directory = mkdtempSync(join(tmpdir(), "termtrace-driver-validation-"));
    const binary = join(directory, "driver");
    const requestFile = join(directory, "request.json");
    const requestId = "00000000-0000-0000-0000-000000000030";
    writeFileSync(
        binary,
        `#!/bin/sh\nIFS= read -r request\nprintf '%s' "$request" > '${requestFile}'\nprintf '%s\\n' '{"version":5,"request_id":"${requestId}","result":{"status":"ok","data":{"kind":"hello","driver_protocol_version":5,"control_protocol_version":15,"event_log_version":6,"state_format_version":5,"methods":["hello"],"max_request_bytes":4194304,"max_response_bytes":4194304}}}'\n`,
    );
    chmodSync(binary, 0o700);
    const client = new DriverClient(binary);

    try {
        const malformedMethod: DriverMethod = { method: "hello" };
        Object.defineProperty(malformedMethod, "toJSON", { enumerable: true, value: () => null });
        const malformedRequest = client.request(malformedMethod, "hello", {
            requestId: "00000000-0000-0000-0000-000000000029",
        });
        await expect(malformedRequest).rejects.toBeInstanceOf(TermtraceProtocolError);

        await expect(
            client.request({ method: "hello" }, "hello", { requestId }),
        ).resolves.toMatchObject({ kind: "hello" });
        expect(JSON.parse(readFileSync(requestFile, "utf8"))).toMatchObject({
            request_id: requestId,
            method: { method: "hello" },
        });
    } finally {
        await client.close();
        rmSync(directory, { recursive: true, force: true });
    }
});

test("terminates a driver when a correlated request exceeds its deadline", async () => {
    const directory = mkdtempSync(join(tmpdir(), "termtrace-driver-timeout-"));
    const binary = join(directory, "driver");
    const childPidFile = join(directory, "child.pid");
    writeFileSync(
        binary,
        `#!/bin/sh\nIFS= read -r request\nsleep 30 &\nchild=$!\nprintf '%s' "$child" > '${childPidFile}'\nwait "$child"\n`,
    );
    chmodSync(binary, 0o700);
    const client = new DriverClient(binary);

    try {
        await expect(
            client.request({ method: "hello" }, "hello", {
                requestId: "00000000-0000-0000-0000-000000000003",
                timeoutMs: 20,
            }),
        ).rejects.toBeInstanceOf(RequestTimeoutError);
        const childPid = Number.parseInt(readFileSync(childPidFile, "utf8"), 10);
        expect(Number.isSafeInteger(childPid)).toBe(true);
        expect(() => process.kill(childPid, 0)).toThrow();
        await expect(
            client.request({ method: "hello" }, "hello", {
                requestId: "00000000-0000-0000-0000-000000000004",
            }),
        ).rejects.toThrow("driver is closed");
    } finally {
        await client.close();
        rmSync(directory, { recursive: true, force: true });
    }
}, 10_000);

test("aborting an in-flight request reaps the driver tree and rejects all FIFO work", async () => {
    const directory = mkdtempSync(join(tmpdir(), "termtrace-driver-abort-"));
    const binary = join(directory, "driver");
    const childPidFile = join(directory, "child.pid");
    writeFileSync(
        binary,
        `#!/bin/sh\nsleep 30 &\nchild=$!\nprintf '%s' "$child" > '${childPidFile}'\nIFS= read -r request\nwait "$child"\n`,
    );
    chmodSync(binary, 0o700);
    const client = new DriverClient(binary);
    const controller = new AbortController();
    const reason = new Error("cancel in-flight capture");

    try {
        const first = client.request({ method: "hello" }, "hello", {
            requestId: "00000000-0000-0000-0000-000000000005",
            signal: controller.signal,
        });
        const second = client.request({ method: "hello" }, "hello", {
            requestId: "00000000-0000-0000-0000-000000000006",
        });
        const firstOutcome = first.catch((cause: unknown) => cause);
        const secondOutcome = second.catch((cause: unknown) => cause);

        for (let attempt = 0; attempt < 100 && !existsSync(childPidFile); attempt += 1) {
            await sleep(5);
        }

        expect(existsSync(childPidFile)).toBe(true);

        controller.abort(reason);
        await expect(
            client.request({ method: "hello" }, "hello", {
                requestId: "00000000-0000-0000-0000-000000000007",
            }),
        ).rejects.toThrow("driver is closed");
        expect(await firstOutcome).toBe(reason);
        expect(await secondOutcome).toBe(reason);

        const childPid = Number.parseInt(readFileSync(childPidFile, "utf8"), 10);
        expect(Number.isSafeInteger(childPid)).toBe(true);
        expect(() => process.kill(childPid, 0)).toThrow();
        await expect(
            client.request({ method: "hello" }, "hello", {
                requestId: "00000000-0000-0000-0000-000000000007",
            }),
        ).rejects.toThrow("driver is closed");
    } finally {
        await client.close();
        rmSync(directory, { recursive: true, force: true });
    }
}, 10_000);
