import { defineConfig } from "vitest/config";
export default defineConfig({
    test: {
        coverage: { reporter: ["text"] },
        include: ["vitest/**/*.test.js"],
        fileParallelism: false,
        maxWorkers: 1,
    },
});
