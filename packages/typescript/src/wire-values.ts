import { TermtraceProtocolError } from "./transport/protocol.js";

export function exactWireInteger(value: number | null, field: string): number {
    if (value === null || !Number.isSafeInteger(value) || value < 0)
        throw new TermtraceProtocolError(`${field} must be a nonnegative safe integer`);
    return value;
}

export function nonnegativeDuration(value: number, field: string): number {
    if (!Number.isFinite(value) || value < 0 || !Number.isSafeInteger(value)) {
        throw new TypeError(`${field} must be a nonnegative integer`);
    }
    return value;
}

export function millisecondsToNanoseconds(milliseconds: number, field: string): number {
    const nanoseconds = milliseconds * 1_000_000;
    if (!Number.isSafeInteger(nanoseconds)) {
        throw new TypeError(`${field} exceeds the exactly representable protocol maximum`);
    }
    return nanoseconds;
}
