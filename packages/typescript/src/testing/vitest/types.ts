import type { MatcherState, TestAPI } from "vitest";

import type { Screen } from "../../screen/screen.js";
import type { Session } from "../../session/session.js";
import type { SnapshotOptions, SnapshotProfile } from "../../screen/snapshots.js";
import type { StyleFilter } from "../../screen/queries.js";
import type { LaunchOptions, TermtraceOptions, Termtrace } from "../../termtrace.js";

export type TerminalTestTask = {
    readonly id: string;
    readonly name: string;
    readonly filePath: string;
    readonly worker: string;
    readonly retry: number;
    readonly repeat: number;
};

export type TerminalTestArtifacts = {
    readonly canonicalSession: string;
    readonly semanticScreenshot: string | undefined;
    readonly bundle: string | undefined;
    readonly verificationValid: boolean | undefined;
};

export type TerminalTestContext = {
    readonly client: Termtrace;
    readonly session: Session;
    readonly screen: Screen;
    readonly evidenceRoot: string;
    readonly task: TerminalTestTask;
    readonly artifacts: TerminalTestArtifacts;
};

export type TerminalTestFixtures = {
    readonly terminal: TerminalTestContext;
    readonly client: Termtrace;
    readonly session: Session;
    readonly screen: Screen;
    readonly evidenceRoot: string;
    readonly artifacts: TerminalTestArtifacts;
};

export type CreateTerminalTestOptions = {
    readonly termtrace: Omit<TermtraceOptions, "evidenceRoot"> & {
        readonly evidenceRoot: string;
    };
    readonly launch:
        | TerminalLaunchOptions
        | ((context: {
              readonly task: TerminalTestTask;
          }) => TerminalLaunchOptions | Promise<TerminalLaunchOptions>);
    readonly failureArtifacts?: FailureArtifactOptions;
    readonly operationTimeoutMs?: number;
};

export type TerminalCursorExpectation = {
    readonly row?: number;
    readonly column?: number;
    readonly visible?: boolean;
};

export type TerminalMatcherResult = {
    readonly pass: boolean;
    readonly message: () => string;
};

export type TerminalMatchers = {
    readonly toHaveScreenText: (
        received: unknown,
        expected: string | RegExp,
    ) => Promise<TerminalMatcherResult>;
    readonly toHaveCursor: (
        received: unknown,
        expected: TerminalCursorExpectation,
    ) => Promise<TerminalMatcherResult>;
    readonly toExist: (received: unknown) => Promise<TerminalMatcherResult>;
    readonly toHaveCount: (received: unknown, expected: number) => Promise<TerminalMatcherResult>;
    readonly toHaveText: (
        received: unknown,
        expected: string | RegExp,
    ) => Promise<TerminalMatcherResult>;
    readonly toHaveStyle: (
        received: unknown,
        expected: StyleFilter,
    ) => Promise<TerminalMatcherResult>;
    readonly toHaveNoWrappedRows: (received: unknown) => Promise<TerminalMatcherResult>;
    readonly toHaveNeutralRegion: (received: unknown) => Promise<TerminalMatcherResult>;
    readonly toHaveNoPresentedTransient: (received: unknown) => Promise<TerminalMatcherResult>;
    readonly toMatchTerminalSnapshot: (
        this: MatcherState,
        received: unknown,
        profile?: SnapshotProfile,
        options?: SnapshotOptions,
        hint?: string,
    ) => Promise<TerminalMatcherResult>;
};

export type TerminalTestApi = TestAPI<TerminalTestFixtures>;
export type TerminalLaunchOptions = Omit<LaunchOptions, "evidenceRoot">;

export type FailureArtifactOptions = {
    readonly snapshotProfile?: SnapshotProfile;
    readonly snapshotOptions?: SnapshotOptions;
    readonly bundleProfile?: "screen" | "debug" | "forensic";
    readonly maxScreenshotBytes?: number;
    readonly maxBundleBytes?: number;
};

export type MutableArtifacts = {
    canonicalSession: string;
    semanticScreenshot?: string;
    bundle?: string;
    verificationValid?: boolean;
};
