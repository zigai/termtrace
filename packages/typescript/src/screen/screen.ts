import { Duration } from "../session/actions.js";
import type { IdleWaitOptions, WaitOptions } from "../session/session.js";
import { InvalidDurationError, StableCaptureIncompleteError } from "../errors.js";
import { Locator, locatorByText, locatorFor } from "./locator.js";
import type { LocatorSpec, StyleFilter, TextQuery } from "./queries.js";
import {
    encodeFrameRequest,
    parseScreenFrame,
    type FrameRequest,
    type Region,
    type StatePoint,
    type ScreenFrame,
} from "./frame.js";
import type { DriverRequestOptions } from "../transport/driver-client.js";
import {
    TermtraceProtocolError,
    type Artifact,
    type ControlResult,
    type ControlCommand,
    type DriverDataKind,
    type DriverDataOf,
    type ScreenPredicate,
    type SessionMethodFields,
} from "../transport/protocol.js";
import {
    exactWireInteger,
    millisecondsToNanoseconds,
    nonnegativeDuration,
} from "../wire-values.js";

type ScreenSession = {
    readonly completed: boolean;
    control(command: ControlCommand): Promise<ControlResult>;
    request<Kind extends DriverDataKind>(
        methodFields: SessionMethodFields,
        expectedKind: Kind,
        options?: DriverRequestOptions,
    ): Promise<DriverDataOf<Kind>>;
};

export type ScreenWait = {
    readonly predicate: ScreenPredicate;
    readonly matched: boolean;
    readonly firstMatchAt: Duration | null;
    readonly observedAt: Duration;
    readonly stableFor: Duration;
    readonly quality:
        | "matched"
        | "matched_unsettled"
        | "child_completed"
        | "output_closed"
        | "deadline";
    readonly frame: ScreenFrame;
};

export type StableOptions = {
    readonly stableFor: Duration;
    readonly timeout: Duration;
    readonly frame?: FrameRequest;
    readonly allowIncomplete?: boolean;
};

export type StableCapture = {
    readonly kind: "stable_snapshot";
    readonly quality: "stable" | "child_completed" | "output_closed" | "deadline";
    readonly firstObservedAt: Duration;
    readonly lastObservedAt: Duration;
    readonly stableFor: Duration;
    readonly frame: ScreenFrame;
};

export class Screen {
    constructor(private readonly session: ScreenSession) {}

    snapshot(): Promise<ControlResult> {
        return this.session.control({ command: "snapshot" });
    }

    /** Preserve the compact observation text endpoint. */
    async text(): Promise<string> {
        const data = await this.session.request(
            { method: "observe", point: { selector: "latest_presented" }, include_hidden: false },
            "observation",
        );
        return data.observation.frame.viewport_text;
    }

    async frame(
        request: FrameRequest = {},
        options: { readonly signal?: AbortSignal } = {},
    ): Promise<ScreenFrame> {
        const data = await this.session.request(
            {
                method: "screen_frame",
                point: request.point ?? { selector: "latest_presented" },
                include_hidden: false,
                projection: encodeFrameRequest(request),
            },
            "screen_frame",
            options,
        );
        return parseScreenFrame(data.frame);
    }

    cells(region: Region): Promise<ScreenFrame> {
        return this.frame({ detail: "cells", region });
    }

    locator(spec: LocatorSpec): Locator {
        return locatorFor((request, options) => this.frame(request, options), spec);
    }

    getByText(
        query: TextQuery,
        options: { readonly exact?: boolean; readonly crossSoftWrap?: boolean } = {},
    ): Locator {
        return locatorByText(
            (request, captureOptions) => this.frame(request, captureOptions),
            query,
            options,
        );
    }

    row(row: number): Locator {
        return this.locator({ kind: "row", row });
    }

    region(region: Region): Locator {
        return this.locator({ kind: "region", region });
    }

    filter(style: StyleFilter): Locator {
        return this.locator({ kind: "style", style });
    }

    async waitForText(expected: string | RegExp, options: WaitOptions = {}): Promise<ScreenFrame> {
        const timeout = Duration.milliseconds(options.timeoutMs ?? 10_000);
        const interval = Duration.milliseconds(25);
        const stableFor = Duration.milliseconds(options.stableForMs ?? 0);
        const waitOptions =
            options.signal === undefined
                ? { timeout, interval, stableFor }
                : { timeout, interval, stableFor, signal: options.signal };
        const result = await this.getByText(expected).wait(waitOptions);
        return result.frame;
    }

    async waitForPredicate(
        predicate: ScreenPredicate,
        options: WaitOptions = {},
    ): Promise<ScreenWait> {
        const timeoutMs = nonnegativeDuration(options.timeoutMs ?? 10_000, "timeoutMs");
        const stableForMs = nonnegativeDuration(options.stableForMs ?? 0, "stableForMs");
        const data = await this.session.control({
            command: "wait_screen",
            predicate,
            stable_for_ns: millisecondsToNanoseconds(stableForMs, "stableForMs"),
            timeout_ns: millisecondsToNanoseconds(timeoutMs, "timeoutMs"),
            projection: encodeFrameRequest(),
        });
        if (data.kind !== "screen_wait")
            throw new TermtraceProtocolError("screen wait response has an invalid kind");
        const observed = exactWireInteger(data.observed_monotonic_ns, "screen wait observed time");
        const stableFor = exactWireInteger(data.stable_for_ns, "screen wait stable interval");
        const first =
            data.first_match_monotonic_ns === null || data.first_match_monotonic_ns === undefined
                ? null
                : Duration.nanoseconds(
                      BigInt(
                          exactWireInteger(
                              data.first_match_monotonic_ns,
                              "screen wait first match time",
                          ),
                      ),
                  );
        return Object.freeze({
            predicate,
            matched: data.matched,
            firstMatchAt: first,
            observedAt: Duration.nanoseconds(BigInt(observed)),
            stableFor: Duration.nanoseconds(BigInt(stableFor)),
            quality: data.quality,
            frame: parseScreenFrame(data.frame),
        });
    }

    waitForAbsence(text: string, options: WaitOptions = {}): Promise<ScreenWait> {
        return this.waitForPredicate({ predicate: "absent", text }, options);
    }

    async captureStable(options: StableOptions): Promise<StableCapture> {
        if (
            options.stableFor.nanoseconds === 0n ||
            options.timeout.nanoseconds === 0n ||
            options.stableFor.nanoseconds > options.timeout.nanoseconds
        ) {
            throw new InvalidDurationError("stableFor must be nonzero and no greater than timeout");
        }
        if (this.session.completed) {
            return Object.freeze({
                kind: "stable_snapshot",
                quality: "stable",
                firstObservedAt: Duration.nanoseconds(0n),
                lastObservedAt: Duration.nanoseconds(0n),
                stableFor: options.stableFor,
                frame: await this.frame(options.frame ?? {}),
            });
        }
        const data = await this.session.control({
            command: "capture_stable",
            stable_for_ns: options.stableFor.toWireNumber("stableFor"),
            timeout_ns: options.timeout.toWireNumber("timeout"),
            projection: encodeFrameRequest(options.frame),
        });
        if (data.kind !== "stable_snapshot")
            throw new TermtraceProtocolError("stable capture response has an invalid kind");
        const first = exactWireInteger(
            data.first_observed_monotonic_ns,
            "stable capture first observed time",
        );
        const last = exactWireInteger(data.observed_monotonic_ns, "stable capture observed time");
        const stable = exactWireInteger(data.stable_for_ns, "stable capture stable interval");
        const capture: StableCapture = Object.freeze({
            kind: data.kind,
            quality: data.quality,
            firstObservedAt: Duration.nanoseconds(BigInt(first)),
            lastObservedAt: Duration.nanoseconds(BigInt(last)),
            stableFor: Duration.nanoseconds(BigInt(stable)),
            frame: parseScreenFrame(data.frame),
        });
        if (data.quality !== "stable" && options.allowIncomplete !== true)
            throw new StableCaptureIncompleteError(capture, data.quality);
        return capture;
    }

    waitForIdle(options: IdleWaitOptions = {}): Promise<StableCapture> {
        const stableFor = Duration.milliseconds(options.stableForMs ?? 100);
        const timeout = Duration.milliseconds(options.timeoutMs ?? 10_000);
        return this.captureStable(
            options.allowIncomplete === undefined
                ? { stableFor, timeout }
                : { stableFor, timeout, allowIncomplete: options.allowIncomplete },
        );
    }

    screenshot(
        destination: string,
        point: StatePoint = { selector: "latest_presented" },
        requestTimeoutMs?: number,
    ): Promise<Artifact> {
        return this.session.request(
            {
                method: "screenshot",
                point,
                destination,
                renderer: "semantic",
                viewport: null,
            },
            "artifact",
            requestTimeoutMs === undefined ? {} : { timeoutMs: requestTimeoutMs },
        );
    }
}
