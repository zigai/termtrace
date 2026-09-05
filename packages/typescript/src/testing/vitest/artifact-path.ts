import { createHash } from "node:crypto";

/** Bound the readable prefix and retain a digest of the complete, unnormalized identity. */
export function boundedPathPart(value: string): string {
    const normalized = value
        .normalize("NFKC")
        .replace(/[^A-Za-z0-9._-]+/gu, "-")
        .replace(/^[._-]+|[._-]+$/gu, "");
    const prefix = normalized.length === 0 ? "unnamed" : normalized.slice(0, 64);
    return `${prefix}-${digest(value)}`;
}

export function digest(value: string): string {
    return createHash("sha256").update(value).digest("hex").slice(0, 12);
}
