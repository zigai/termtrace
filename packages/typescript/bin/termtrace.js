#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";

const packagesByPlatform = new Map([
    ["linux:x64", "@termtrace/cli-linux-x64"],
    ["linux:arm64", "@termtrace/cli-linux-arm64"],
]);
const platform = `${process.platform}:${process.arch}`;
const platformPackage = packagesByPlatform.get(platform);
if (platformPackage === undefined) {
    console.error(`termtrace: unsupported platform ${platform}`);
    process.exit(2);
}

const require = createRequire(import.meta.url);
let binary;
try {
    binary = require.resolve(`${platformPackage}/bin/termtrace`);
} catch (cause) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    console.error(`termtrace: platform package ${platformPackage} is unavailable: ${detail}`);
    process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error !== undefined) {
    console.error(`termtrace: could not launch ${binary}: ${result.error.message}`);
    process.exit(1);
}

if (result.signal !== null) {
    console.error(`termtrace: process terminated by ${result.signal}`);
    process.exit(1);
}

process.exit(result.status ?? 1);
