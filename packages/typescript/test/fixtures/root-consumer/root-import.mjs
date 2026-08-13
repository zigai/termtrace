import { Termtrace } from "@termtrace/test";

if (typeof Termtrace.create !== "function") {
    throw new Error("root export did not resolve");
}
