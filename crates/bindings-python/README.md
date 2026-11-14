<h1 align="center">Termtrace for Python</h1>

<p align="center"><strong>Record and drive terminal programs from Python.</strong></p>

<p align="center">
  <a href="https://github.com/zigai/termtrace/actions/workflows/ci.yml"><img alt="tests" src="https://img.shields.io/github/actions/workflow/status/zigai/termtrace/ci.yml?branch=main&label=tests&labelColor=30363d"></a>
  <a href="https://pypi.org/project/termtrace/"><img alt="PyPI" src="https://img.shields.io/pypi/v/termtrace?label=PyPI&color=blue&logo=pypi&logoColor=white&labelColor=30363d"></a>
  <a href="https://github.com/zigai/termtrace/blob/main/crates/bindings-python/pyproject.toml"><img alt="Python" src="https://img.shields.io/badge/Python-3.10+-blue?logo=python&logoColor=white&labelColor=30363d"></a>
  <a href="https://pepy.tech/projects/termtrace"><img alt="downloads" src="https://img.shields.io/pypi/dm/termtrace?label=downloads&color=blue&labelColor=30363d"></a>
  <a href="https://github.com/zigai/termtrace/blob/main/LICENSE"><img alt="license" src="https://img.shields.io/github/license/zigai/termtrace?label=license&labelColor=30363d"></a>
</p>

This package is the Python SDK for [Termtrace](https://github.com/zigai/termtrace), a flight recorder for terminal programs. Start a program in a recorded terminal, wait for text on its screen, and send it keys.

- **Screen waits:** Block until the screen shows the text you expect, with a timeout.
- **Test fixture:** In `pytest`, `termtrace_session` records the programs a test starts and keeps the recording when the test fails.
- **Asynchronous API:** `termtrace.aio` offers the same calls for `asyncio` code.
- **Nothing else to install:** The wheel bundles the `termtrace` binary.

## Installation

```sh
uv add termtrace
```

Requires Python 3.10 or newer on Linux with glibc 2.39 or newer.

## Examples

### Drive a program

```python
import termtrace

with termtrace.Termtrace().record(["python3"]) as rec:
    rec.screen().wait_for_text(">>>", timeout_secs=5)
    rec.actions().send_text("print(6 * 7)").send_key("enter").until_screen("42").run()
    print(rec.screen().frame().text)
```

### Test with `pytest`

```python
def test_python_answers(termtrace_session):
    rec = termtrace_session(["python3"])
    rec.screen().wait_for_text(">>>", timeout_secs=5)
    rec.actions().send_text("print(6 * 7)").send_key("enter").until_screen("42").run()
    assert "42" in rec.screen().frame().text
```

### Use `asyncio`

```python
import asyncio

from termtrace.aio import AsyncTermtrace


async def main():
    async with await AsyncTermtrace().record(["python3"]) as rec:
        await rec.screen().wait_for_text(">>>", timeout_secs=5)
        await rec.actions().send_text("print(6 * 7)").send_key("enter").until_screen("42").run()
        print((await rec.screen().frame()).text)


asyncio.run(main())
```

## License

[MIT](https://github.com/zigai/termtrace/blob/main/LICENSE)
