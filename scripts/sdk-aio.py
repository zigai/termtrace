#!/usr/bin/env python3
"""Generate the asynchronous Python wrappers in termtrace/aio.py from the native stub."""

import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "crates/bindings-python/python/termtrace"
STUB = PACKAGE / "_termtrace.pyi"
OUTPUT = PACKAGE / "aio.py"

# Native classes that perform I/O get an `Async` wrapper; every other class is returned as is.
WRAPPED = [
    "ActionBuilder",
    "Diagnostics",
    "Events",
    "Evidence",
    "Instrumentation",
    "Locator",
    "Native",
    "OriginalRecording",
    "Output",
    "Pointer",
    "Recording",
    "Registry",
    "Screen",
    "Session",
    "System",
    "Termtrace",
    "Tmux",
]

# Hand-written base class providing special behavior, per wrapped class.
BASES = {"Locator": "LocatorBase", "Recording": "RecordingBase"}

# Methods that run on the calling thread because they never block.
IMMEDIATE = {
    "ActionBuilder": {"*"},
    "Events": {"cursor_to_json", "cursor_from_json"},
    "Locator": {"and_", "in_row", "in_region", "with_style", "across_soft_wraps"},
    "OriginalRecording": {"session", "detach"},
    "Output": {"cursor_to_json", "cursor_from_json"},
    "Recording": {
        "actions",
        "analysis",
        "events",
        "evidence",
        "instrumentation",
        "native",
        "output",
        "pointer",
        "screen",
    },
    "Screen": {"locator", "get_by_text", "get_by_regex", "row", "region", "filter"},
    "Session": {
        "actions",
        "analysis",
        "events",
        "evidence",
        "instrumentation",
        "native",
        "output",
        "pointer",
        "screen",
    },
    "Termtrace": {
        "analysis",
        "evidence",
        "instrumentation",
        "native",
        "registry",
        "system",
        "tmux",
    },
}

# Mutations keep running after a cancelled await and are tracked until they return.
MUTATIONS = {
    "ActionBuilder": {"run", "execute"},
    "OriginalRecording": {"finish", "wait_finished", "force_close"},
    "Pointer": {"*"},
    "Recording": {
        "execute_action",
        "mark",
        "pointer_click",
        "pointer_focus",
        "pointer_scroll",
        "request_stop",
        "resize",
        "send",
        "signal",
        "wait_for_completion",
    },
    "Session": {"mark", "resize", "send", "signal"},
}

# Launches force-close a recording that starts after its await was cancelled.
LAUNCHES = {"Termtrace": {"launch", "record"}}

# Wrappers whose mutations belong to the recording or session that created them.
CHILDREN = {"ActionBuilder", "Pointer"}

SKIPPED = {"__enter__", "__exit__", "__repr__", "__new__", "__init__"}


def listed(table: dict[str, set[str]], cls: str, name: str) -> bool:
    names = table.get(cls, set())
    return name in names or "*" in names


class Rename(ast.NodeTransformer):
    def visit_Name(self, node: ast.Name) -> ast.AST:
        if node.id in WRAPPED:
            return ast.Name(id=f"Async{node.id}", ctx=node.ctx)
        return node


def annotation(node: ast.expr | None) -> str:
    return "" if node is None else ast.unparse(Rename().visit(node))


def returned(node: ast.expr | None) -> str | None:
    return node.id if isinstance(node, ast.Name) and node.id in WRAPPED else None


def parameters(function: ast.FunctionDef) -> tuple[str, list[str]]:
    args = function.args
    rendered = ["self"]
    forwarded = []
    positional = args.posonlyargs + args.args
    defaults = [None] * (len(positional) - len(args.defaults)) + list(args.defaults)
    for arg, default in zip(positional[1:], defaults[1:]):
        text = arg.arg + (f": {annotation(arg.annotation)}" if arg.annotation else "")
        rendered.append(
            text + (f" = {ast.unparse(default)}" if default is not None else "")
        )
        forwarded.append(arg.arg)
    if args.kwonlyargs:
        rendered.append("*")
        for arg, default in zip(args.kwonlyargs, args.kw_defaults):
            text = arg.arg + (
                f": {annotation(arg.annotation)}" if arg.annotation else ""
            )
            rendered.append(
                text + (f" = {ast.unparse(default)}" if default is not None else "")
            )
            forwarded.append(f"{arg.arg}={arg.arg}")
    return ", ".join(rendered), forwarded


def docstring(function: ast.FunctionDef, indent: str) -> list[str]:
    text = ast.get_docstring(function)
    if not text:
        return []
    lines = text.strip().splitlines()
    if len(lines) == 1:
        return [f'{indent}r"""{lines[0]}"""']
    return [
        f'{indent}r"""{lines[0]}',
        *[f"{indent}{line}".rstrip() for line in lines[1:]],
        f'{indent}"""',
    ]


def wrap(expression: str, cls: str | None) -> str:
    if cls is None:
        return expression
    pending = ", self._pending" if cls in CHILDREN else ""
    return f"Async{cls}._wrap({expression}{pending})"


def method(cls: str, function: ast.FunctionDef) -> list[str]:
    name = function.name
    signature, forwarded = parameters(function)
    result = annotation(function.returns) or "None"
    target = returned(function.returns)
    property_ = any(
        isinstance(d, ast.Name) and d.id == "property" for d in function.decorator_list
    )
    call = f"self._sync.{name}({', '.join(forwarded)})"
    lines: list[str] = []
    if property_:
        lines += ["    @property", f"    def {name}(self) -> {result}:"]
        lines += docstring(function, "        ")
        lines.append(f"        return {wrap(f'self._sync.{name}', target)}")
        return lines
    cancellable = any(
        arg.arg == "cancel" for arg in function.args.args + function.args.kwonlyargs
    )
    if cls == "ActionBuilder" and name == "run":
        # Sealing happens now, so the request cannot change while it is in flight.
        lines.append(f"    def {name}({signature}) -> Coroutine[Any, Any, {result}]:")
        lines += docstring(function, "        ")
        lines += [
            "        self._sync.seal()",
            f"        return offload_mutation(self._pending, lambda: {call})",
        ]
        return lines
    if listed(IMMEDIATE, cls, name) and not listed(MUTATIONS, cls, name):
        lines.append(
            f"    def {name}({signature}) -> {'Self' if target == cls else result}:"
        )
        lines += docstring(function, "        ")
        if target == cls:
            lines += [f"        {call}", "        return self"]
        else:
            lines.append(f"        return {wrap(call, target)}")
        return lines
    lines.append(f"    async def {name}({signature}) -> {result}:")
    lines += docstring(function, "        ")
    if cancellable:
        tokened = [
            "cancel=token"
            if arg == "cancel=cancel"
            else "token"
            if arg == "cancel"
            else arg
            for arg in forwarded
        ]
        inner = f"self._sync.{name}({', '.join(tokened)})"
        lines += [
            "        def call(token: CancelToken) -> typing.Any:",
            f"            return {inner}",
            "",
            f"        return {wrap('await run_cancellable(call, cancel)', target)}",
        ]
    elif listed(LAUNCHES, cls, name):
        lines.append(f"        return {wrap(f'await launch(lambda: {call})', target)}")
    elif listed(MUTATIONS, cls, name):
        lines.append(
            f"        return {wrap(f'await offload_mutation(self._pending, lambda: {call})', target)}"
        )
    else:
        lines.append(
            f"        return {wrap(f'await asyncio.to_thread(lambda: {call})', target)}"
        )
    return lines


def constructor(cls: str, function: ast.FunctionDef | None) -> list[str]:
    if function is None:
        return []
    signature, forwarded = parameters(function)
    return [
        f"    def __init__({signature}) -> None:",
        f"        self._sync = _native.{cls}({', '.join(forwarded)})",
        "        self._pending = set()",
        "",
    ]


def base_methods() -> dict[str, set[str]]:
    runtime = ast.parse((PACKAGE / "_async.py").read_text())
    methods = {}
    for node in runtime.body:
        if isinstance(node, ast.ClassDef):
            methods[node.name] = {
                item.name
                for item in ast.walk(node)
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
            }
    return methods


def generate(stub: ast.Module) -> str:
    classes = {node.name: node for node in stub.body if isinstance(node, ast.ClassDef)}
    inherited = base_methods()
    out = [
        '"""Asynchronous Termtrace wrappers. Generated from the native stub; run scripts/sdk-aio.py."""',
        "",
        "from __future__ import annotations",
        "",
        "import asyncio",
        "import builtins",
        "import os",
        "import typing",
        "from collections.abc import Coroutine",
        "from typing import TYPE_CHECKING, Any",
        "",
        "from termtrace import _termtrace as _native",
        "from termtrace import types",
        "from termtrace._async import (",
        "    LocatorBase,",
        "    RecordingBase,",
        "    Wrapper,",
        "    launch,",
        "    offload_mutation,",
        "    run_cancellable,",
        ")",
        "from termtrace._termtrace import *  # noqa: F403",
        "from termtrace._termtrace import CancelToken",
        "",
        "if TYPE_CHECKING:",
        "    from typing_extensions import Self",
        "",
        "    from termtrace import editing",
        "",
        "__all__ = [" + ", ".join(f'"Async{name}"' for name in WRAPPED) + "]",
    ]
    for cls in WRAPPED:
        node = classes[cls]
        functions = [item for item in node.body if isinstance(item, ast.FunctionDef)]
        new = next((item for item in functions if item.name == "__new__"), None)
        out += ["", "", f"class Async{cls}({BASES.get(cls, 'Wrapper')}):"]
        doc = ast.get_docstring(node)
        if doc:
            out.append(f'    r"""Asynchronous {cls}: {doc.strip().splitlines()[0]}"""')
        out.append("")
        out += constructor(cls, new)
        for function in functions:
            if cls in BASES and function.name in inherited[BASES[cls]] - {"wait_for"}:
                continue
            if function.name in SKIPPED or any(
                isinstance(d, ast.Name) and d.id == "staticmethod"
                for d in function.decorator_list
            ):
                continue
            out += method(cls, function)
            out.append("")
        while out[-1] == "":
            out.pop()
    return "\n".join(out) + "\n"


def main() -> int:
    OUTPUT.write_text(generate(ast.parse(STUB.read_text())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
