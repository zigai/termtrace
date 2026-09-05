import { expect, test } from "bun:test";
import { chmod, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import { Termtrace } from "../src/termtrace.js";
import { finishTerminalTest } from "../src/testing/vitest/lifecycle.js";
import type { MutableArtifacts } from "../src/testing/vitest/types.js";

const binary =
    process.env.TERMTRACE_BINARY ?? resolve(import.meta.dir, "../../../target/debug/termtrace");

test("successful finish retains evidence until the entire real driver lifecycle succeeds", async () => {
    const root = await mkdtemp(join(tmpdir(), "termtrace-finish-"));
    const client = await Termtrace.create({ binaryPath: binary, evidenceRoot: root });
    try {
        const session = await client.launch({
            command: ["/bin/sh", "-c", "printf READY"],
            evidence: "on-failure",
        });
        await session.evidence.complete();
        await session.finish(true);
        expect((await stat(join(session.canonicalSession, "completion.json"))).isFile()).toBe(true);
        await expect(session.discardAfterDriverClose()).rejects.toThrow();
        await client.close();
        await expect(stat(session.canonicalSession)).rejects.toThrow();
    } finally {
        await client.close();
        await rm(root, { recursive: true, force: true });
    }
});

test("a later body failure cancels an earlier success-only finish without replacing the primary error", async () => {
    const root = await mkdtemp(join(tmpdir(), "termtrace-late-failure-"));
    const client = await Termtrace.create({ binaryPath: binary, evidenceRoot: root });
    try {
        const session = await client.launch({
            command: ["/bin/sh", "-c", "printf READY"],
            evidence: "on-failure",
        });
        await session.evidence.complete();
        await session.finish(true);
        const primary = new Error("primary body failure");
        await expect(
            finishTerminalTest({
                session,
                client,
                evidenceRoot: root,
                artifacts: { canonicalSession: session.canonicalSession },
                failureArtifacts: { maxScreenshotBytes: 1, maxBundleBytes: 1 },
                operationTimeoutMs: 10_000,
                evidencePolicy: "on-failure",
                bodyFailed: true,
                primaryWasCaught: true,
                primaryFailure: primary,
            }),
        ).rejects.toBe(primary);
        const verifier = await Termtrace.create({ binaryPath: binary });
        try {
            const retained = await verifier.attach(session.canonicalSession);
            expect((await retained.evidence.verify()).valid).toBe(true);
        } finally {
            await verifier.close();
        }
    } finally {
        await client.close();
        await rm(root, { recursive: true, force: true });
    }
}, 20_000);

test("a real driver close failure after a passing body retains verified canonical evidence", async () => {
    const root = await mkdtemp(join(tmpdir(), "termtrace-close-failure-"));
    const wrapper = join(root, "driver");
    await writeFile(wrapper, `#!/bin/sh\n${JSON.stringify(binary)} "$@"\nexit 17\n`);
    await chmod(wrapper, 0o700);
    const client = await Termtrace.create({ binaryPath: wrapper, evidenceRoot: root });
    const session = await client.launch({
        command: ["/bin/sh", "-c", "printf READY"],
        evidence: "on-failure",
    });
    const artifacts: MutableArtifacts = { canonicalSession: session.canonicalSession };
    try {
        await expect(
            finishTerminalTest({
                session,
                client,
                evidenceRoot: root,
                artifacts,
                failureArtifacts: undefined,
                operationTimeoutMs: 10_000,
                evidencePolicy: "on-failure",
                bodyFailed: false,
                primaryWasCaught: false,
                primaryFailure: undefined,
            }),
        ).rejects.toThrow("Termtrace test cleanup failed");
        expect(artifacts.verificationValid).toBe(true);
        const verifier = await Termtrace.create({ binaryPath: binary });
        try {
            const retained = await verifier.attach(session.canonicalSession);
            expect((await retained.evidence.verify()).valid).toBe(true);
        } finally {
            await verifier.close();
        }
        await expect(client.close()).rejects.toThrow();
        expect((await stat(session.canonicalSession)).isDirectory()).toBe(true);
    } finally {
        await rm(root, { recursive: true, force: true });
    }
}, 20_000);

test("generation budgets reject semantic screenshots and bundles before publication", async () => {
    const root = await mkdtemp(join(tmpdir(), "termtrace-generation-budget-"));
    const client = await Termtrace.create({ binaryPath: binary, evidenceRoot: root });
    try {
        const session = await client.launch({
            command: ["/bin/sh", "-c", "printf READY"],
            viewport: { cols: 8, rows: 2 },
        });
        await session.evidence.complete();
        const screenshot = join(root, "screenshot");
        const bundle = join(root, "bundle");
        await expect(
            session.screen.screenshot(screenshot, { selector: "latest_presented" }, 10_000, 1),
        ).rejects.toThrow();
        await expect(session.evidence.bundle(bundle, "debug", 10_000, 1)).rejects.toThrow();
        await expect(stat(screenshot)).rejects.toThrow();
        await expect(stat(bundle)).rejects.toThrow();
        expect((await readdir(root)).some((name) => name.endsWith(".tmp"))).toBe(false);
        const reference = await session.evidence.bundle(bundle, "debug");
        const memberBytes = reference.manifest.members.reduce(
            (total, member) => total + member.byte_length,
            0,
        );
        const manifestBytes = (await stat(join(bundle, "manifest.json"))).size;
        const exact = join(root, "exact-budget");
        await session.evidence.bundle(exact, "debug", 10_000, memberBytes + manifestBytes);
        await expect(
            session.evidence.bundle(
                join(root, "short-budget"),
                "debug",
                10_000,
                memberBytes + manifestBytes - 1,
            ),
        ).rejects.toThrow();
        await expect(stat(join(root, "short-budget"))).rejects.toThrow();
        expect((await session.evidence.verify()).valid).toBe(true);
    } finally {
        await client.close();
        await rm(root, { recursive: true, force: true });
    }
}, 30_000);

test("cancelling an in-flight read leaves the real driver usable for finalization", async () => {
    const root = await mkdtemp(join(tmpdir(), "termtrace-cancel-read-"));
    const client = await Termtrace.create({ binaryPath: binary, evidenceRoot: root });
    try {
        const session = await client.launch({
            command: ["/bin/sh", "-c", "printf READY; sleep 30"],
        });
        await session.output.waitForText("READY");
        const controller = new AbortController();
        const reason = new Error("cancel read");
        const waiting = session.request(
            {
                method: "wait_output",
                data: Buffer.from("never").toString("base64"),
                timeout_ms: 100,
            },
            "output_wait",
            { signal: controller.signal },
        );
        const outcome = waiting.catch((cause: unknown) => cause);
        controller.abort(reason);
        expect(await outcome).toBe(reason);
        await session.finish(false);
        expect((await stat(join(session.canonicalSession, "completion.json"))).isFile()).toBe(true);
    } finally {
        await client.close();
        await rm(root, { recursive: true, force: true });
    }
}, 15_000);

test("aborting launch reaps an owned TERM-resistant child before returning", async () => {
    const root = await mkdtemp(join(tmpdir(), "termtrace-cancel-launch-"));
    const wrapper = join(root, "driver.mjs");
    const attached = join(root, "attached");
    const childPid = join(root, "child.pid");
    await writeFile(
        wrapper,
        `#!/usr/bin/env node
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { writeFileSync } from "node:fs";
const driver = spawn(${JSON.stringify(binary)}, process.argv.slice(2), { stdio: ["inherit", "pipe", "inherit"] });
createInterface({ input: driver.stdout }).on("line", (line) => {
    if (JSON.parse(line).result?.data?.kind === "attached") writeFileSync(${JSON.stringify(attached)}, "ready");
    else process.stdout.write(line + "\\n");
});
driver.on("exit", (code) => { process.exitCode = code ?? 1; });
`,
    );
    await chmod(wrapper, 0o700);
    const client = await Termtrace.create({ binaryPath: wrapper, evidenceRoot: root });
    const controller = new AbortController();
    const reason = new Error("cancel launch");
    try {
        const launching = client.launch({
            command: [
                "/bin/sh",
                "-c",
                `trap '' TERM; printf '%s' "$$" > ${JSON.stringify(childPid)}; while :; do sleep 1; done`,
            ],
            signal: controller.signal,
        });
        const outcome = launching.catch((cause: unknown) => cause);
        const deadline = Date.now() + 10_000;
        while (Date.now() < deadline) {
            try {
                await stat(attached);
                await stat(childPid);
                break;
            } catch {
                await sleep(10);
            }
        }
        await stat(attached);
        const pid = Number.parseInt(await readFile(childPid, "utf8"), 10);
        const started = Date.now();
        controller.abort(reason);
        expect(await outcome).toBe(reason);
        await client.close();
        expect(Date.now() - started).toBeLessThan(8_000);
        expect(() => process.kill(pid, 0)).toThrow();
        const canonical = (await readdir(root)).find((name) => name.endsWith(".tt"));
        expect(canonical).toBeDefined();
        if (canonical === undefined) throw new Error("launch did not record canonical evidence");
        const verifier = await Termtrace.create({ binaryPath: binary });
        try {
            const retained = await verifier.attach(join(root, canonical));
            expect((await retained.evidence.verify()).valid).toBe(true);
        } finally {
            await verifier.close();
        }
    } finally {
        controller.abort(reason);
        await client.close();
        await rm(root, { recursive: true, force: true });
    }
}, 25_000);
