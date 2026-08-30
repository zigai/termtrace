import { afterAll, describe, expect, test } from "bun:test";
import type { Dirent } from "node:fs";
import { cp, mkdtemp, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";

const packageRoot = resolve(import.meta.dir, "..");
const fixtureRoot = join(import.meta.dir, "fixtures", "vitest-consumer");
const rootFixture = join(import.meta.dir, "fixtures", "root-consumer");
const temporaryRoot = await mkdtemp(join(tmpdir(), "termtrace-vitest-consumer-"));

afterAll(async () => {
    await rm(temporaryRoot, { recursive: true, force: true });
});

describe("packed Vitest consumer", () => {
    test("keeps root imports optional and exercises real Vitest lifecycle", async () => {
        const repositoryRoot = resolve(packageRoot, "..", "..");
        const built = await run(["cargo", "build", "--locked", "--bin", "termtrace"], repositoryRoot);
        expect(built.exitCode, built.output).toBe(0);
        const packageBuild = await run(["bun", "run", "build"], packageRoot);
        expect(packageBuild.exitCode, packageBuild.output).toBe(0);
        const termtraceBinary = join(repositoryRoot, "target", "debug", "termtrace");
        const packDirectory = join(temporaryRoot, "pack");
        await mkdir(packDirectory, { recursive: true });
        const packed = await run(["bun", "pm", "pack", "--destination", packDirectory], packageRoot);
        expect(packed.exitCode, packed.output).toBe(0);
        const tarballName = (await readdir(packDirectory)).find((entry) => entry.endsWith(".tgz"));
        expect(tarballName).toBeDefined();
        if (tarballName === undefined) {
            return;
        }
        const tarball = join(packDirectory, tarballName);

        const rootConsumer = join(temporaryRoot, "root-consumer");
        await cp(rootFixture, rootConsumer, { recursive: true });
        await writeConsumerPackage(rootConsumer, tarball, false);
        const rootInstall = await run(["bun", "install", "--offline"], rootConsumer);
        expect(rootInstall.exitCode, rootInstall.output).toBe(0);
        await expect(stat(join(rootConsumer, "node_modules", "vitest"))).rejects.toThrow();
        const rootImport = await run(["node", "root-import.mjs"], rootConsumer);
        expect(rootImport.exitCode, rootImport.output).toBe(0);

        const vitestConsumer = join(temporaryRoot, "vitest-consumer");
        await cp(fixtureRoot, vitestConsumer, { recursive: true });
        await writeConsumerPackage(vitestConsumer, tarball, true);
        const vitestInstall = await run(["bun", "install", "--offline"], vitestConsumer);
        expect(vitestInstall.exitCode, vitestInstall.output).toBe(0);
        const typeProbe = await run(["bun", "run", "tsc", "--noEmit"], vitestConsumer);
        expect(typeProbe.exitCode, typeProbe.output).toBe(0);

        const evidenceRoot = join(temporaryRoot, "evidence");
        const environment = {
            ...process.env,
            CI: "",
            TERMTRACE_TEST_EVIDENCE_ROOT: evidenceRoot,
            TERMTRACE_BINARY: termtraceBinary,
        };
        const firstRun = await run(
            ["bun", "run", "vitest", "run", "--config", "vitest.config.ts"],
            vitestConsumer,
            environment,
        );
        expect(firstRun.exitCode, firstRun.output).toBe(0);
        const retainedAfterPassingRun = await filesBelow(evidenceRoot);
        const canonicalSessionRoots = new Set(
            retainedAfterPassingRun
                .filter((path) => path.includes(".tt"))
                .map((path) => path.slice(0, path.indexOf(".tt") + 3)),
        );
        expect(canonicalSessionRoots.size).toBe(1);

        const snapshot = (await filesBelow(vitestConsumer)).find(
            (path) => path.includes("__snapshots__") && path.endsWith(".snap"),
        );
        expect(snapshot).toBeDefined();
        if (snapshot === undefined) {
            return;
        }
        const snapshotText = await readFile(snapshot, "utf8");
        expect(snapshotText).toContain("READY");
        await writeFile(snapshot, snapshotText.replace("READY", "STALE"), "utf8");
        const mismatch = await run(
            ["bun", "run", "vitest", "run", "lifecycle.fixture.ts", "--config", "vitest.config.ts"],
            vitestConsumer,
            environment,
        );
        expect(mismatch.exitCode).not.toBe(0);
        expect(mismatch.output).toContain("Snapshot");
        const update = await run(
            [
                "bun",
                "run",
                "vitest",
                "run",
                "lifecycle.fixture.ts",
                "--config",
                "vitest.config.ts",
                "--update",
            ],
            vitestConsumer,
            environment,
        );
        expect(update.exitCode, update.output).toBe(0);

        const failure = await run(
            ["bun", "run", "vitest", "run", "--config", "vitest.failure.config.ts"],
            vitestConsumer,
            environment,
        );
        expect(failure.exitCode).not.toBe(0);
        expect(failure.output).toContain("PRIMARY_FAILURE_SENTINEL");
        expect(failure.output).toContain("PRIMARY_WITH_CLEANUP_SENTINEL");
        expect(failure.output).toContain("Termtrace test cleanup failed");
        expect(failure.output).toContain("retains timeout evidence");
        expect(failure.output).toContain("Termtrace evidence:");
        expect(failure.output).toContain("Semantic screenshot:");
        expect(failure.output).toContain("Failure bundle:");
        const evidenceFiles = await filesBelow(evidenceRoot);
        expect(evidenceFiles.map((path) => basename(path))).toContain("screen.png");
        expect(evidenceFiles.some((path) => path.includes("bundle"))).toBe(true);
        expect(evidenceFiles.map((path) => basename(path))).toContain("completion.json");
        expect(evidenceFiles.map((path) => basename(path))).toContain("manifest.json");
        expect(evidenceFiles.some((path) => path.endsWith(".sock"))).toBe(false);
    }, 180_000);
});

async function writeConsumerPackage(
    directory: string,
    tarball: string,
    includeVitest: boolean,
): Promise<void> {
    const common = {
        name: `termtrace-${basename(directory)}`,
        private: true,
        type: "module",
        dependencies: {
            "@termtrace/test": `file:${tarball}`,
        },
    };
    const packageJson = includeVitest
            ? {
                ...common,
                  devDependencies: {
                      "@types/node": "^24.0.0",
                      typescript: "^5.8.3",
                      vitest: "^4.1.3",
                  },
              }
        : common;
    await writeFile(
        join(directory, "package.json"),
        `${JSON.stringify(packageJson, null, 2)}\n`,
        "utf8",
    );
}

async function run(
    command: readonly string[],
    cwd: string,
    environment: Readonly<Record<string, string | undefined>> = process.env,
): Promise<{ readonly exitCode: number; readonly output: string }> {
    const child = Bun.spawn([...command], {
        cwd,
        env: { ...environment },
        stdout: "pipe",
        stderr: "pipe",
    });
    const [stdout, stderr, exitCode] = await Promise.all([
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
        child.exited,
    ]);
    return {
        exitCode,
        output: `${stdout}\n${stderr}`.slice(-16_384),
    };
}

async function filesBelow(directory: string): Promise<string[]> {
    const files: string[] = [];
    let entries: Dirent[];
    try {
        entries = await readdir(directory, { withFileTypes: true });
    } catch (cause: unknown) {
        if (cause instanceof Error && "code" in cause && cause.code === "ENOENT") {
            return files;
        }
        throw cause;
    }
    for (const entry of entries) {
        const path = join(directory, entry.name);
        if (entry.isDirectory()) {
            files.push(...(await filesBelow(path)));
            continue;
        }
        files.push(path);
    }
    return files;
}
