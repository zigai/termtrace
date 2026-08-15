import type { ControlErrorDetails } from "./generated/driver-response.js";

export type DriverFailure = {
    readonly _tag: "DriverFailure";
    readonly code: string;
    readonly message: string;
    readonly retrySafety: string;
    readonly uncertain: boolean;
    readonly evidence: string | undefined;
    readonly details: ControlErrorDetails | undefined;
};
