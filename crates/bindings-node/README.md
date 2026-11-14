<h1 align="center">Termtrace for Node.js</h1>

<p align="center"><strong>Record and drive terminal programs from Node.js and TypeScript.</strong></p>

<p align="center">
  <a href="https://github.com/zigai/termtrace/actions/workflows/ci.yml"><img alt="tests" src="https://img.shields.io/github/actions/workflow/status/zigai/termtrace/ci.yml?branch=main&label=tests&labelColor=30363d"></a>
  <a href="https://www.npmjs.com/package/@zigai/termtrace"><img alt="npm" src="https://img.shields.io/npm/v/@zigai/termtrace?label=npm&color=blue&logo=npm&labelColor=30363d"></a>
  <a href="https://www.npmjs.com/package/@zigai/termtrace"><img alt="downloads" src="https://img.shields.io/npm/dm/@zigai/termtrace?label=downloads&color=blue&labelColor=30363d"></a>
  <a href="https://github.com/zigai/termtrace/blob/main/LICENSE"><img alt="license" src="https://img.shields.io/github/license/zigai/termtrace?label=license&labelColor=30363d"></a>
</p>

This package is the Node.js SDK for [Termtrace](https://github.com/zigai/termtrace), a flight recorder for terminal programs. Start a program in a recorded terminal, wait for text on its screen, and send it keys.

- **Screen waits:** Wait until the screen shows the text you expect, with a timeout.
- **Test helper:** `withTermtrace` stops the programs a test starts and keeps the recording when the test fails.
- **TypeScript types:** The package ships its own type declarations.
- **Nothing else to install:** The package bundles the `termtrace` binary.

## Installation

```sh
npm install @zigai/termtrace
```

Requires Node.js 18 or newer on Linux with glibc 2.39 or newer.

## Examples

### Drive a program

```js
const { Termtrace } = require("@zigai/termtrace");

async function main() {
  const rec = await new Termtrace().record(["python3"]);
  try {
    await rec.screen().waitForText(">>>", 5000);
    await rec.actions().sendText("print(6 * 7)").sendKey("enter").untilScreen("42").run();
    console.log((await rec.screen().frame()).text);
  } finally {
    await rec.stop();
  }
}

main();
```

### Test with node:test

```js
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { withTermtrace } = require("@zigai/termtrace/testing");

test("python answers", async () => {
  await withTermtrace(async ({ record }) => {
    const rec = await record(["python3"]);
    await rec.screen().waitForText(">>>", 5000);
    await rec.actions().sendText("print(6 * 7)").sendKey("enter").untilScreen("42").run();
    assert.match((await rec.screen().frame()).text, /42/);
  });
});
```

## License

[MIT](https://github.com/zigai/termtrace/blob/main/LICENSE)
