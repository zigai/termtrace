import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { expect, test } from "bun:test";

import { DriverClient } from "../src/driver-client.js";

test("rejects requests issued after the driver exits", async () => {
  const directory = mkdtempSync(join(tmpdir(), "termtrace-driver-client-"));
  const binary = join(directory, "driver");
  writeFileSync(binary, "#!/bin/sh\nIFS= read -r request\nexit 0\n");
  chmodSync(binary, 0o700);
  const client = new DriverClient(binary);

  try {
    await expect(
      client.request(
        { method: "hello" },
        "hello",
        "00000000-0000-0000-0000-000000000001",
      ),
    ).rejects.toThrow("driver exited");
    await expect(
      client.request(
        { method: "hello" },
        "hello",
        "00000000-0000-0000-0000-000000000002",
      ),
    ).rejects.toThrow("driver is closed");
  } finally {
    await client.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
