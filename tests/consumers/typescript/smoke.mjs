import assert from "node:assert/strict";
import { Termtrace } from "@termtrace/test";

const terminal = await Termtrace.create({ evidenceRoot: "typescript-evidence" });
try {
    const session = await terminal.launch({
        command: ["/bin/sh", "-c", "printf installed-typescript-sdk; read -r answer"],
        evidence: "always",
    });
    assert.equal(
        (await session.output.waitForText("installed-typescript-sdk")).output_end_offset,
        "installed-typescript-sdk".length,
    );
    const span = await session.application.startSpan("installed SDK");
    await session.application.event("consumer diagnostic", { count: 1 });
    await session.application.endSpan("installed SDK", span);
    assert.equal((await session.events.page()).events.filter((event) => event.event_type === "application").length, 3);
    await session.keyboard.type("done\n");
    assert.equal((await session.complete(10_000)).exit_code, 0);
    assert.equal((await session.evidence.verify()).valid, true);
    await session.finish(true);
} finally {
    await terminal.close();
}
