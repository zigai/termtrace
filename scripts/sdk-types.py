#!/usr/bin/env python3
"""Generate Python and TypeScript SDK types from the Rust SDK type catalog."""

import json
import keyword
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PYTHON_OUT = ROOT / "crates/bindings-python/python/termtrace/types.py"
PYTHON_STUB = ROOT / "crates/bindings-python/python/termtrace/_termtrace.pyi"
TYPESCRIPT_OUT = ROOT / "crates/bindings-node/types.d.ts"
HEADER = ROOT / "crates/bindings-node/dts-header.d.ts"
HEADER_START = "// <sdk-types>"
HEADER_END = "// </sdk-types>"
FRAME_EXTENSION = "x-termtrace-frame"
WIDE_FORMATS = {"int64", "uint64", "int128", "uint128", "int", "uint"}


def load_catalog() -> dict:
    output = subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "termtrace-service", "--example", "sdk_types"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return json.loads(output)


def pascal(text: str) -> str:
    return "".join(part[:1].upper() + part[1:] for part in re.split(r"[^0-9A-Za-z]+", str(text)) if part)


def variant_name(parent: str, branch: dict, index: int) -> str:
    for spec in branch.get("properties", {}).values():
        if isinstance(spec, dict) and "const" in spec:
            return parent + pascal(spec["const"])
    if len(branch.get("required", [])) == 1 and set(branch.get("properties", {})) == set(branch["required"]):
        return parent + pascal(branch["required"][0])
    return f"{parent}Variant{index}"


class Emitter:
    def __init__(self, definitions: dict[str, dict]) -> None:
        self.definitions = definitions
        self.refs: dict[str, str] = {}
        self.python: dict[str, str] = {}
        self.typescript: dict[str, str] = {}

    def run(self) -> None:
        for name, schema in self.definitions.items():
            self.define(name, schema)

    def define(self, name: str, schema: dict) -> None:
        if name in self.python:
            return
        self.python[name] = ""
        self.typescript[name] = ""
        description = schema.get("description") if isinstance(schema, dict) else None
        if self.is_object(schema):
            self.python[name] = self.typed_dict(name, schema, description)
            self.typescript[name] = self.interface(name, schema, description)
            return
        self.python[name] = f'{name}: TypeAlias = "{self.type(schema, name, True)}"'
        self.typescript[name] = self.jsdoc(description, "") + f"export type {name} = {self.type(schema, name, False)}"

    @staticmethod
    def is_object(schema: object) -> bool:
        return isinstance(schema, dict) and "properties" in schema and not any(
            key in schema for key in ("oneOf", "anyOf", "allOf")
        )

    def type(self, schema: object, name: str, python: bool) -> str:
        if schema is True or schema == {}:
            return "JsonValue"
        if schema is False:
            return "Never" if python else "never"
        if not isinstance(schema, dict):
            raise TypeError(f"{name}: unsupported schema {schema!r}")
        if "$ref" in schema:
            name = str(schema["$ref"]).rsplit("/", 1)[-1]
            return self.refs.get(name, name)
        if schema.get("x-termtrace-path"):
            return "str" if python else "string | Uint8Array"
        if schema.get("x-termtrace-native-integer"):
            return "int" if python else "bigint"
        if "const" in schema:
            text = repr(schema["const"]) if python else json.dumps(schema["const"])
            return f"Literal[{text}]" if python else text
        if "enum" in schema:
            values = [repr(v) if python else json.dumps(v) for v in schema["enum"]]
            return "Literal[" + ", ".join(values) + "]" if python else " | ".join(values)
        for union in ("oneOf", "anyOf"):
            if union in schema:
                return self.union([self.branch(name, schema, branch, i, python) for i, branch in enumerate(schema[union])], python)
        if "allOf" in schema:
            parts = schema["allOf"]
            if len(parts) != 1:
                raise TypeError(f"{name}: allOf with {len(parts)} parts is not supported")
            return self.type(parts[0], name, python)
        kind = schema.get("type")
        if isinstance(kind, list):
            return self.union([self.type({**schema, "type": k}, name, python) for k in kind], python)
        if kind == "null":
            return "None" if python else "null"
        if kind == "boolean":
            return "bool" if python else "boolean"
        if kind == "integer":
            if schema.get("format") in WIDE_FORMATS:
                return "int" if python else "bigint"
            return "int" if python else "number"
        if kind == "number":
            return "float" if python else "number"
        if kind == "string":
            if schema.get("contentEncoding") == "base64":
                return "bytes" if python else "Uint8Array"
            return "str" if python else "string"
        if kind == "array":
            if "prefixItems" in schema:
                items = [self.type(item, f"{name}Item{i}", python) for i, item in enumerate(schema["prefixItems"])]
                return f"tuple[{', '.join(items)}]" if python else f"[{', '.join(items)}]"
            item = self.type(schema.get("items", {}), f"{name}Item", python)
            return f"list[{item}]" if python else f"Array<{item}>"
        if kind == "object" or "properties" in schema:
            if "properties" not in schema:
                value = self.type(schema.get("additionalProperties", {}), f"{name}Value", python)
                return f"dict[str, {value}]" if python else f"Record<string, {value}>"
            self.define(name, schema)
            return name
        return "JsonValue"

    def branch(self, parent: str, schema: dict, branch: dict, index: int, python: bool) -> str:
        if "properties" in schema and isinstance(branch, dict):
            branch = {
                **branch,
                "properties": {**schema["properties"], **branch.get("properties", {})},
                "required": list(dict.fromkeys([*schema.get("required", []), *branch.get("required", [])])),
            }
        if self.is_object(branch):
            name = variant_name(parent, branch, index)
            self.define(name, branch)
            return name
        return self.type(branch, f"{parent}Variant{index}", python)

    @staticmethod
    def union(types: list[str], python: bool) -> str:
        types = list(dict.fromkeys(types))
        if python:
            literals = [t[len("Literal[") : -1] for t in types if t.startswith("Literal[")]
            if len(literals) > 1:
                first = next(i for i, t in enumerate(types) if t.startswith("Literal["))
                rest = [t for t in types if not t.startswith("Literal[")]
                types = [*rest[:first], "Literal[" + ", ".join(literals) + "]", *rest[first:]]
        return " | ".join(types)

    def typed_dict(self, name: str, schema: dict, description: str | None) -> str:
        fields = schema["properties"]
        required = set(schema.get("required", []))
        functional = any(keyword.iskeyword(field) or not field.isidentifier() for field in fields)
        entries = []
        for field, spec in fields.items():
            field_type = self.type(spec, name + pascal(field), True)
            if field not in required:
                field_type = f"NotRequired[{field_type}]"
            entries.append((field, field_type))
        if functional:
            if schema.get(FRAME_EXTENSION):
                entries.append(("frame", "FrameHandle"))
            body = ", ".join(f"{field!r}: {json.dumps(field_type)}" for field, field_type in entries)
            return f'{name} = TypedDict("{name}", {{{body}}})'
        lines = [f"class {name}(TypedDict):"]
        if description:
            lines.append(f"    {json.dumps(description.strip())}")
            lines.append("")
        lines += [f"    {field}: {field_type}" for field, field_type in entries]
        if schema.get(FRAME_EXTENSION):
            lines.append("    frame: FrameHandle")
        if not entries:
            lines.append("    pass")
        return "\n".join(lines)

    @staticmethod
    def jsdoc(description: str | None, indent: str) -> str:
        if not description:
            return ""
        text = description.strip().replace("*/", "*\\/")
        return f"{indent}/** {' '.join(text.split())} */\n"

    def interface(self, name: str, schema: dict, description: str | None) -> str:
        required = set(schema.get("required", []))
        lines = [self.jsdoc(description, "") + f"export interface {name} {{"]
        for field, spec in schema["properties"].items():
            doc = self.jsdoc(spec.get("description") if isinstance(spec, dict) else None, "  ")
            optional = "" if field in required else "?"
            key = field if field.isidentifier() else json.dumps(field)
            lines.append(f"{doc}  {key}{optional}: {self.type(spec, name + pascal(field), False)}")
        if schema.get(FRAME_EXTENSION):
            lines.append("  frame: FrameHandle")
        lines.append("}")
        return "\n".join(lines)


PYTHON_HEADER = '''"""SDK model types generated from the Rust type catalog. Do not edit; run scripts/sdk-types.py."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal, TypeAlias, TypedDict

if TYPE_CHECKING:
    from typing_extensions import Never, NotRequired

    from termtrace._termtrace import ScreenFrame as FrameHandle

JsonValue: TypeAlias = "None | bool | int | float | str | list[JsonValue] | dict[str, JsonValue]"
'''

TYPESCRIPT_HEADER = """/** SDK model types generated from the Rust type catalog. Do not edit; run scripts/sdk-types.py. */
import type { ScreenFrame as FrameHandle } from './index'
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
/** A native method result: the inner model type. */
export type Model<T> = T
/** A native method argument: the inner model type. */
export type Arg<T> = T
"""


def native_classes() -> set[str]:
    index = (ROOT / "crates/bindings-node/index.d.ts").read_text()
    return set(re.findall(r"^export declare class (\w+)", index, re.M))


def write_header(names: list[str]) -> None:
    classes = native_classes()
    names = [name for name in names if name not in classes]
    text = HEADER.read_text()
    start = text.index(HEADER_START)
    end = text.index(HEADER_END) + len(HEADER_END)
    block = "\n".join(
        [
            HEADER_START,
            "import type { Arg, JsonValue, Model, " + ", ".join(names) + " } from './types'",
            "export type * from './types'",
            HEADER_END,
        ]
    )
    HEADER.write_text(text[:start] + block + text[end:])


def references(schema: object) -> set[str]:
    if isinstance(schema, dict):
        found = {str(schema["$ref"]).rsplit("/", 1)[-1]} if "$ref" in schema else set()
        return found.union(*(references(value) for value in schema.values()))
    if isinstance(schema, list):
        return set().union(*(references(value) for value in schema))
    return set()


def input_variants(inputs: dict[str, dict], outputs: dict[str, dict]) -> set[str]:
    """Input types the native stub names, and the types they reference, whose accepted shape differs
    from the output shape.

    The difference test mirrors `termtrace_service::sdk_contract::types::input_name`, which names
    the stub argument types.
    """

    def closure(name: str) -> set[str]:
        seen: set[str] = set()
        pending = [name]
        while pending:
            current = pending.pop()
            if current not in seen:
                seen.add(current)
                pending.extend(references(inputs[current]))
        return seen

    stub = PYTHON_STUB.read_text()
    roots = set(re.findall(r"\btypes\.(\w+)Input\b", stub))
    accepted = set().union(*(closure(root) for root in roots)) if roots else set()
    return {
        name
        for name in accepted
        if any(inputs[member] != outputs.get(member) for member in closure(name))
    }


def main() -> int:
    catalog = load_catalog()
    inputs = catalog["$defs"]
    outputs = catalog["serialized"]["$defs"]
    typescript = Emitter(inputs)
    typescript.run()
    python = Emitter(outputs)
    python.run()
    variants = input_variants(inputs, outputs)
    accepted = Emitter({f"{name}Input": inputs[name] for name in sorted(variants)})
    accepted.refs = {name: f"{name}Input" for name in variants}
    accepted.run()
    python.python.update(accepted.python)
    names = sorted(typescript.typescript)
    body = "\n\n\n".join(python.python[n] for n in sorted(python.python))
    header = PYTHON_HEADER if "Never" in body else PYTHON_HEADER.replace("Never, NotRequired", "NotRequired")
    PYTHON_OUT.write_text(header + "\n\n" + body + "\n")
    TYPESCRIPT_OUT.write_text(TYPESCRIPT_HEADER + "\n" + "\n\n".join(typescript.typescript[n] for n in names) + "\n")
    write_header(names)
    return 0


if __name__ == "__main__":
    sys.exit(main())
