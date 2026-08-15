import { expect } from "vitest";

import type { StyleFilter } from "../screen/queries.js";
import type { SnapshotOptions, SnapshotProfile } from "../screen/snapshots.js";
import { vitestMatcherRegistry } from "./vitest/matchers.js";
import type { TerminalCursorExpectation } from "./vitest/types.js";

export { createTerminalTest } from "./vitest/fixture.js";
export { terminalMatchers } from "./vitest/matchers.js";
export type {
    CreateTerminalTestOptions,
    TerminalCursorExpectation,
    TerminalTestArtifacts,
    TerminalTestContext,
    TerminalTestFixtures,
    TerminalTestTask,
} from "./vitest/types.js";

expect.extend(vitestMatcherRegistry);

declare module "vitest" {
    interface Matchers<T> {
        toHaveScreenText(expected: string | RegExp): Promise<T>;
        toHaveCursor(expected: TerminalCursorExpectation): Promise<T>;
        toExist(): Promise<T>;
        toHaveCount(expected: number): Promise<T>;
        toHaveText(expected: string | RegExp): Promise<T>;
        toHaveStyle(expected: StyleFilter): Promise<T>;
        toHaveNoWrappedRows(): Promise<T>;
        toHaveNeutralRegion(): Promise<T>;
        toHaveNoPresentedTransient(): Promise<T>;
        toMatchTerminalSnapshot(
            profile?: SnapshotProfile,
            options?: SnapshotOptions,
            hint?: string,
        ): Promise<T>;
    }
}
