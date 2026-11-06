use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    path::Path,
};
use termtrace_sdk::contracts::types::{
    catalog,
    declarations::{ascii_json, declarations, Declaration, DeclarationBody, TypeExpr},
};

const HEADER: &str = r#""""SDK model types generated from the Rust type catalog. Do not edit; run `just generate`."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal, TypeAlias, TypedDict

if TYPE_CHECKING:
    from typing_extensions import Never, NotRequired

    from termtrace._termtrace import ScreenFrame as FrameHandle

JsonValue: TypeAlias = "None | bool | int | float | str | list[JsonValue] | dict[str, JsonValue]"
"#;

const KEYWORDS: [&str; 35] = [
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

pub(crate) fn write(package: &Path, stub: &str) -> Result<(), Box<dyn Error>> {
    let catalog = catalog()?;
    let inputs = definitions(catalog.get("$defs"))?;
    let outputs = definitions(
        catalog
            .get("serialized")
            .and_then(|serialized| return serialized.get("$defs")),
    )?;

    let mut declared = declarations(
        outputs
            .iter()
            .map(|(name, schema)| return (name.clone(), schema)),
        &BTreeMap::new(),
    )?;
    let variants = input_variants(inputs, outputs, stub)?;
    let refs = variants
        .iter()
        .map(|name| return (name.clone(), format!("{name}Input")))
        .collect::<BTreeMap<_, _>>();
    let mut accepted = Vec::new();
    for name in &variants {
        let schema = inputs
            .get(name)
            .ok_or_else(|| return format!("missing input `{name}`"))?;
        accepted.push((format!("{name}Input"), schema));
    }
    declared.extend(declarations(accepted, &refs)?);

    let body = declared
        .iter()
        .map(|(name, declaration)| return render_declaration(name, declaration))
        .collect::<Vec<_>>()
        .join("\n\n\n");
    let header = if body.contains("Never") {
        HEADER.to_owned()
    } else {
        HEADER.replace("Never, NotRequired", "NotRequired")
    };
    fs_err::write(package.join("types.py"), format!("{header}\n\n{body}\n"))?;

    return Ok(());
}

fn definitions(value: Option<&Value>) -> Result<&Map<String, Value>, String> {
    return value
        .and_then(Value::as_object)
        .ok_or_else(|| return "catalog definitions are not an object".to_owned());
}

fn references(schema: &Value, found: &mut BTreeSet<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref") {
                let reference = reference
                    .as_str()
                    .map_or_else(|| return reference.to_string(), str::to_owned);
                found.insert(reference.rsplit('/').next().unwrap_or_default().to_owned());
            }
            for value in map.values() {
                references(value, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                references(item, found);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn closure(inputs: &Map<String, Value>, name: &str) -> Result<BTreeSet<String>, String> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![name.to_owned()];
    while let Some(current) = pending.pop() {
        if seen.contains(&current) {
            continue;
        }
        let schema = inputs
            .get(&current)
            .ok_or_else(|| return format!("missing input `{current}`"))?;
        let mut found = BTreeSet::new();
        references(schema, &mut found);
        pending.extend(found);
        seen.insert(current);
    }

    return Ok(seen);
}

// The native stub names an input type `types.<Name>Input`. Such types, and the types they
// reference, get their own declaration when their accepted shape differs from the output shape,
// mirroring `termtrace_service::sdk_contract::types::input_name`.
fn input_variants(
    inputs: &Map<String, Value>,
    outputs: &Map<String, Value>,
    stub: &str,
) -> Result<BTreeSet<String>, String> {
    let mut accepted = BTreeSet::new();
    for root in input_roots(stub) {
        accepted.extend(closure(inputs, &root)?);
    }

    let mut variants = BTreeSet::new();
    for name in accepted {
        let mut differs = false;
        for member in closure(inputs, &name)? {
            differs |= inputs.get(&member) != outputs.get(&member);
        }
        if differs {
            variants.insert(name);
        }
    }
    return Ok(variants);
}

fn is_word(character: char) -> bool {
    return character.is_alphanumeric() || character == '_';
}

fn input_roots(stub: &str) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    for (start, _) in stub.match_indices("types.") {
        let before = stub
            .get(..start)
            .and_then(|text| return text.chars().next_back());
        if before.is_some_and(is_word) {
            continue;
        }
        let rest = stub.get(start + "types.".len()..).unwrap_or_default();
        let word = rest
            .split(|character| return !is_word(character))
            .next()
            .unwrap_or_default();
        if let Some(root) = word
            .strip_suffix("Input")
            .filter(|root| return !root.is_empty())
        {
            roots.insert(root.to_owned());
        }
    }

    return roots;
}

fn render_declaration(name: &str, declaration: &Declaration) -> String {
    return match &declaration.body {
        DeclarationBody::Object { fields, frame } => {
            let functional = fields.iter().any(|field| {
                return KEYWORDS.contains(&field.name.as_str()) || !is_identifier(&field.name);
            });
            let mut entries = fields
                .iter()
                .map(|field| {
                    let value = render(&field.value);
                    let value =
                        if field.required { value } else { format!("NotRequired[{value}]") };
                    return (field.name.clone(), value);
                })
                .collect::<Vec<_>>();
            if functional {
                if *frame {
                    entries.push(("frame".to_owned(), "FrameHandle".to_owned()));
                }
                let body = entries
                    .iter()
                    .map(|(field, value)| {
                        return format!(
                            "{}: {}",
                            repr(&Value::String(field.clone())),
                            ascii_json(&Value::String(value.clone()))
                        );
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return format!("{name} = TypedDict(\"{name}\", {{{body}}})");
            }

            let mut lines = vec![format!("class {name}(TypedDict):")];
            if let Some(description) = declaration
                .description
                .as_deref()
                .filter(|text| return !text.is_empty())
            {
                lines.push(format!(
                    "    {}",
                    ascii_json(&Value::String(description.trim().to_owned()))
                ));
                lines.push(String::new());
            }
            lines.extend(
                entries
                    .iter()
                    .map(|(field, value)| return format!("    {field}: {value}")),
            );
            if *frame {
                lines.push("    frame: FrameHandle".to_owned());
            }
            if entries.is_empty() {
                lines.push("    pass".to_owned());
            }
            lines.join("\n")
        }
        DeclarationBody::Alias(value) => format!("{name}: TypeAlias = \"{}\"", render(value)),
    };
}

fn render(expr: &TypeExpr) -> String {
    return match expr {
        TypeExpr::Json => "JsonValue".to_owned(),
        TypeExpr::Never => "Never".to_owned(),
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::Path | TypeExpr::String => "str".to_owned(),
        TypeExpr::WideInteger | TypeExpr::Integer => "int".to_owned(),
        TypeExpr::Literal(values) => {
            format!(
                "Literal[{}]",
                values.iter().map(repr).collect::<Vec<_>>().join(", ")
            )
        }
        TypeExpr::Union(types) => union(types.iter().map(render).collect()),
        TypeExpr::Null => "None".to_owned(),
        TypeExpr::Boolean => "bool".to_owned(),
        TypeExpr::Number => "float".to_owned(),
        TypeExpr::Bytes => "bytes".to_owned(),
        TypeExpr::Tuple(items) => {
            format!(
                "tuple[{}]",
                items.iter().map(render).collect::<Vec<_>>().join(", ")
            )
        }
        TypeExpr::Array(item) => format!("list[{}]", render(item)),
        TypeExpr::Map(value) => format!("dict[str, {}]", render(value)),
    };
}

fn union(rendered: Vec<String>) -> String {
    let mut types = Vec::<String>::new();
    for item in rendered {
        if !types.contains(&item) {
            types.push(item);
        }
    }

    let literals = types
        .iter()
        .filter_map(|item| return literal_values(item))
        .collect::<Vec<_>>();
    if literals.len() > 1 {
        let merged = format!("Literal[{}]", literals.join(", "));
        let first = types
            .iter()
            .position(|item| return item.starts_with("Literal["))
            .unwrap_or_default();
        let mut rest = types
            .iter()
            .filter(|item| return !item.starts_with("Literal["))
            .cloned()
            .collect::<Vec<_>>();
        rest.insert(first.min(rest.len()), merged);
        return rest.join(" | ");
    }
    return types.join(" | ");
}

fn literal_values(rendered: &str) -> Option<&str> {
    let rest = rendered.strip_prefix("Literal[")?;
    let mut characters = rest.chars();
    characters.next_back();

    return Some(characters.as_str());
}

fn is_identifier(text: &str) -> bool {
    let mut characters = text.chars();
    return characters
        .next()
        .is_some_and(|first| return first.is_alphabetic() || first == '_')
        && characters.all(is_word);
}

fn repr(value: &Value) -> String {
    return match value {
        Value::String(text) => {
            let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
            let mut out = String::from(quote);
            for character in text.chars() {
                match character {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    _ if character == quote => {
                        out.push('\\');
                        out.push(quote);
                    }
                    '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {
                        out.extend(format!("\\x{:02x}", u32::from(character)).chars());
                    }
                    _ => out.push(character),
                }
            }
            out.push(quote);
            out
        }
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Null => "None".to_owned(),
        Value::Number(_) | Value::Array(_) | Value::Object(_) => value.to_string(),
    };
}
