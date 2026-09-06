import { expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { Termtrace } from "../src/index.js";

test("application spans retain process identity without changing the terminal transcript", async () => {
    const root = await mkdtemp(join(tmpdir(), "termtrace-application-"));
    const terminal = await Termtrace.create({
        binaryPath:
            process.env.TERMTRACE_BINARY ??
            resolve(import.meta.dir, "../../../target/debug/termtrace"),
        evidenceRoot: root,
    });
    try {
        const session = await terminal.launch({
            command: ["/bin/sh", "-c", "printf READY; read -r answer"],
        });
        await session.output.waitForText("READY");
        const span = await session.application.startSpan("work", { count: 2 });
        await session.application.event("progress", { nested: { ok: true } });
        await session.application.endSpan("work", span);
        const page = await session.events.page();
        const data = page.events.flatMap((record) =>
            record.event?.type === "application" ? [record.event.data] : [],
        );
        expect(data.map((event) => event.kind)).toEqual(["span_start", "event", "span_end"]);
        expect(
            data.every(
                (event) => event.process_instance_id === session.application.processInstanceId,
            ),
        ).toBe(true);
        expect(data.map((event) => event.span_id)).toEqual([span, null, span]);
        expect(data[1]?.attributes).toEqual({ nested: { ok: true } });
        await session.keyboard.type("done\n");
        expect((await session.complete(10_000)).exit_code).toBe(0);
        expect((await session.evidence.verify()).valid).toBe(true);
    } finally {
        await terminal.close();
        await rm(root, { recursive: true, force: true });
    }
});
