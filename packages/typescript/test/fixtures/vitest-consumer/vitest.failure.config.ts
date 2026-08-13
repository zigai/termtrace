import { defineConfig } from "vitest/config";

export default defineConfig({
    test: {
        include: ["failure-matrix.fixture.ts"],
        testTimeout: 15_000,
        hookTimeout: 30_000,
        fileParallelism: false,
        maxWorkers: 1,
    },
});
