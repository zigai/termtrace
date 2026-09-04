import { setTimeout as sleep } from "node:timers/promises";
import { Duration } from "../session/actions.js";
import {
    InvalidDurationError,
    InvalidQueryError,
    LocatorTimeoutError,
    MultipleMatchesError,
    NoMatchError,
} from "../errors.js";
import {
    allOf,
    evaluateQuery,
    regionQuery,
    rowQuery,
    styleQuery,
    type LocatorSpec,
    type ScreenMatch,
    type StyleFilter,
} from "./queries.js";
import type { FrameRequest, Region, ScreenFrame } from "./frame.js";

export type LocatorWaitOptions = {
    readonly timeout?: Duration;
    readonly interval?: Duration;
    readonly stableFor?: Duration;
    readonly signal?: AbortSignal;
};
export type LocatorWaitResult = {
    readonly frame: ScreenFrame;
    readonly matches: ReadonlyArray<ScreenMatch>;
};
export type FrameCapture = (
    request: FrameRequest,
    options?: { readonly signal?: AbortSignal },
) => Promise<ScreenFrame>;

export class Locator {
    constructor(
        private readonly capture: FrameCapture,
        readonly spec: LocatorSpec,
    ) {
        Object.freeze(this);
    }

    and(spec: LocatorSpec): Locator {
        return new Locator(this.capture, allOf(this.spec, spec));
    }
    row(row: number): Locator {
        return this.and(rowQuery(row));
    }
    region(region: Region): Locator {
        return this.and(regionQuery(region));
    }
    filter(style: StyleFilter): Locator {
        return this.and(styleQuery(style));
    }

    async all(): Promise<ReadonlyArray<ScreenMatch>> {
        return (await this.observeOnce()).matches;
    }
    async count(): Promise<number> {
        return (await this.observeOnce()).matches.length;
    }
    async single(): Promise<ScreenMatch> {
        const result = await this.observeOnce();
        if (result.matches.length === 0)
            throw new NoMatchError(describe(this.spec), result.frame.stateHash);
        if (result.matches.length !== 1) {
            throw new MultipleMatchesError(
                describe(this.spec),
                result.matches.length,
                result.frame.stateHash,
                result.matches.slice(0, 8).map((match) => match.ranges),
                result.matches.slice(0, 8).map((match) => match.excerpt),
            );
        }
        const match = result.matches[0];
        if (match === undefined)
            throw new NoMatchError(describe(this.spec), result.frame.stateHash);
        return match;
    }
    async first(): Promise<ScreenMatch> {
        const result = await this.observeOnce();
        const match = result.matches[0];
        if (match === undefined)
            throw new NoMatchError(describe(this.spec), result.frame.stateHash);
        return match;
    }
    async nth(index: number): Promise<ScreenMatch> {
        if (!Number.isSafeInteger(index) || index < 0)
            throw new InvalidQueryError("locator index must be a nonnegative safe integer");
        const result = await this.observeOnce();
        const match = result.matches[index];
        if (match === undefined)
            throw new NoMatchError(
                `${describe(this.spec)} at index ${index}`,
                result.frame.stateHash,
            );
        return match;
    }

    async wait(options: LocatorWaitOptions = {}): Promise<LocatorWaitResult> {
        const timeout = options.timeout ?? Duration.seconds(10);
        const interval = options.interval ?? Duration.milliseconds(25);
        const stableFor = options.stableFor ?? Duration.nanoseconds(0n);
        if (timeout.nanoseconds === 0n)
            throw new InvalidDurationError("locator timeout must be nonzero");
        if (interval.nanoseconds === 0n)
            throw new InvalidDurationError("locator interval must be nonzero");
        if (stableFor.nanoseconds > timeout.nanoseconds)
            throw new InvalidDurationError("locator stable interval must not exceed its timeout");
        const timeoutMs = Number(timeout.nanoseconds / 1_000_000n);
        const intervalMs = Math.max(1, Number(interval.nanoseconds / 1_000_000n));
        const stableForMs = Number(stableFor.nanoseconds / 1_000_000n);
        const deadline = performance.now() + timeoutMs;
        let matchingState: { readonly hash: string; readonly since: number } | undefined;
        let lastFrame: ScreenFrame | undefined;
        for (;;) {
            options.signal?.throwIfAborted();
            const remainingBeforeCapture = deadline - performance.now();
            if (remainingBeforeCapture <= 0)
                throw new LocatorTimeoutError(describe(this.spec), lastFrame?.stateHash);
            const captureGuard = new AbortController();
            const guardSignal =
                options.signal === undefined
                    ? captureGuard.signal
                    : AbortSignal.any([options.signal, captureGuard.signal]);
            const deadlineGuard = sleep(remainingBeforeCapture, undefined, {
                signal: guardSignal,
            }).then(() => {
                throw new LocatorTimeoutError(describe(this.spec), lastFrame?.stateHash);
            });
            let frame: ScreenFrame;
            try {
                frame = await Promise.race([
                    this.capture(frameRequest(this.spec), { signal: guardSignal }),
                    deadlineGuard,
                ]);
            } catch (cause) {
                options.signal?.throwIfAborted();
                throw cause;
            } finally {
                captureGuard.abort();
            }
            options.signal?.throwIfAborted();
            const observedAt = performance.now();
            if (observedAt >= deadline)
                throw new LocatorTimeoutError(describe(this.spec), lastFrame?.stateHash);
            lastFrame = frame;
            const matches = evaluateQuery(frame, this.spec);
            if (matches.length > 0) {
                if (stableFor.nanoseconds === 0n) return Object.freeze({ frame, matches });
                if (matchingState === undefined || matchingState.hash !== frame.stateHash)
                    matchingState = { hash: frame.stateHash, since: observedAt };
                else if (observedAt - matchingState.since >= stableForMs)
                    return Object.freeze({ frame, matches });
            } else {
                matchingState = undefined;
            }
            const remaining = deadline - observedAt;
            if (remaining <= 0)
                throw new LocatorTimeoutError(describe(this.spec), lastFrame.stateHash);
            await sleep(Math.min(intervalMs, remaining), undefined, { signal: options.signal });
        }
    }

    private async observeOnce(): Promise<LocatorWaitResult> {
        const frame = await this.capture(frameRequest(this.spec));
        return Object.freeze({ frame, matches: evaluateQuery(frame, this.spec) });
    }
}

function frameRequest(spec: LocatorSpec): FrameRequest {
    const region = findRegion(spec);
    return region === undefined ? { detail: "spans" } : { detail: "spans", region };
}
function findRegion(spec: LocatorSpec): Region | undefined {
    if (spec.kind === "region") return spec.region;
    if (spec.kind !== "all") return undefined;
    let selected: Region | undefined;
    for (const nested of spec.specs) {
        const region = findRegion(nested);
        if (region === undefined) continue;
        if (selected === undefined) {
            selected = region;
            continue;
        }
        const top = Math.max(selected.top, region.top);
        const left = Math.max(selected.left, region.left);
        const bottom = Math.min(selected.top + selected.height, region.top + region.height);
        const right = Math.min(selected.left + selected.width, region.left + region.width);
        if (top >= bottom || left >= right) return undefined;
        selected = { top, left, width: right - left, height: bottom - top };
    }
    return selected;
}
function describe(spec: LocatorSpec): string {
    switch (spec.kind) {
        case "text":
            return String(spec.query);
        case "style":
            return `style ${JSON.stringify(spec.style)}`;
        case "row":
            return `row ${spec.row}`;
        case "region":
            return `region ${JSON.stringify(spec.region)}`;
        case "all":
            return spec.specs.map(describe).join(" and ");
    }
}
