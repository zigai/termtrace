import { createRequire } from "node:module";

const packagesByPlatform = new Map([
    ["linux:x64", "@termtrace/cli-linux-x64"],
    ["linux:arm64", "@termtrace/cli-linux-arm64"],
]);

/** Resolve the matching optional binary package without relying on npm's script PATH. */
export function packagedBinary(): string | undefined {
    const platformPackage = packagesByPlatform.get(`${process.platform}:${process.arch}`);
    if (platformPackage === undefined) {
        return undefined;
    }
    try {
        return createRequire(import.meta.url).resolve(`${platformPackage}/bin/termtrace`);
    } catch (cause: unknown) {
        if (cause instanceof Error && "code" in cause && cause.code === "MODULE_NOT_FOUND") {
            return undefined;
        }
        throw cause;
    }
}

/** Explicit and environment overrides precede the bundled binary and PATH fallback. */
export function resolveBinary(explicit?: string): string {
    return explicit ?? process.env.TERMTRACE_BINARY ?? packagedBinary() ?? "termtrace";
}
