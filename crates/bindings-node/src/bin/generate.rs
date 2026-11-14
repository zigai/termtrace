//! Write the TypeScript model types and the declaration header that re-exports them.

use serde_json::Value;
use std::{collections::BTreeMap, error::Error, path::Path};
use termtrace_sdk::contracts::types::{
    catalog,
    declarations::{ascii_json, declarations, Declaration, DeclarationBody, TypeExpr},
};

const HEADER: &str = "/** SDK model types generated from the Rust type catalog. Do not edit; run `just generate`. */
import type { ScreenFrame as FrameHandle } from './index'
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
/** A native method result: the inner model type. */
export type Model<T> = T
/** A native method argument: the inner model type. */
export type Arg<T> = T
";
const BLOCK_START: &str = "// <sdk-types>";
const BLOCK_END: &str = "// </sdk-types>";

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = catalog()?;
    let inputs = catalog
        .get("$defs")
        .and_then(Value::as_object)
        .ok_or("catalog definitions are not an object")?;
    let declared = declarations(
        inputs
            .iter()
            .map(|(name, schema)| return (name.clone(), schema)),
        &BTreeMap::new(),
    )?;

    let body = declared
        .iter()
        .map(|(name, declaration)| return render_declaration(name, declaration))
        .collect::<Vec<_>>()
        .join("\n\n");
    fs_err::write(root.join("types.d.ts"), format!("{HEADER}\n{body}\n"))?;

    let index = fs_err::read_to_string(root.join("index.d.ts"))?;
    let classes = index
        .lines()
        .filter_map(|line| return line.strip_prefix("export declare class "))
        .filter_map(|rest| {
            return rest
                .split(|character: char| return !is_word(character))
                .next();
        })
        .collect::<Vec<_>>();
    let names = declared
        .keys()
        .map(String::as_str)
        .filter(|name| return !classes.contains(name))
        .collect::<Vec<_>>();
    let block = format!(
        "{BLOCK_START}\nimport type {{ Arg, JsonValue, Model, {} }} from './types'\nexport type * from './types'\n{BLOCK_END}",
        names.join(", ")
    );

    let path = root.join("dts-header.d.ts");
    let header = fs_err::read_to_string(&path)?;
    let start = header
        .find(BLOCK_START)
        .ok_or("dts-header.d.ts has no type block start")?;
    let end = header
        .find(BLOCK_END)
        .ok_or("dts-header.d.ts has no type block end")?
        + BLOCK_END.len();
    let before = header
        .get(..start)
        .ok_or("type block start is not a character boundary")?;
    let after = header
        .get(end..)
        .ok_or("type block end is not a character boundary")?;
    fs_err::write(path, format!("{before}{block}{after}"))?;

    return Ok(());
}

fn is_word(character: char) -> bool {
    return character.is_alphanumeric() || character == '_';
}

fn is_identifier(text: &str) -> bool {
    let mut characters = text.chars();
    return characters
        .next()
        .is_some_and(|first| return first.is_alphabetic() || first == '_')
        && characters.all(is_word);
}

fn jsdoc(description: Option<&str>, indent: &str) -> String {
    let Some(description) = description.filter(|text| return !text.is_empty()) else {
        return String::new();
    };

    let text = description.trim().replace("*/", "*\\/");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    return format!("{indent}/** {text} */\n");
}

fn render_declaration(name: &str, declaration: &Declaration) -> String {
    let doc = jsdoc(declaration.description.as_deref(), "");
    return match &declaration.body {
        DeclarationBody::Object { fields, frame } => {
            let mut lines = vec![format!("{doc}export interface {name} {{")];
            for field in fields {
                let field_doc = jsdoc(field.description.as_deref(), "  ");
                let optional = if field.required { "" } else { "?" };
                let key = if is_identifier(&field.name) {
                    field.name.clone()
                } else {
                    ascii_json(&Value::String(field.name.clone()))
                };
                lines.push(format!(
                    "{field_doc}  {key}{optional}: {}",
                    render(&field.value)
                ));
            }
            if *frame {
                lines.push("  frame: FrameHandle".to_owned());
            }
            lines.push("}".to_owned());
            lines.join("\n")
        }
        DeclarationBody::Alias(value) => format!("{doc}export type {name} = {}", render(value)),
    };
}

fn render(expr: &TypeExpr) -> String {
    return match expr {
        TypeExpr::Json => "JsonValue".to_owned(),
        TypeExpr::Never => "never".to_owned(),
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::Path => "string | Uint8Array".to_owned(),
        TypeExpr::WideInteger(_) => "bigint".to_owned(),
        TypeExpr::Literal(values) => values
            .iter()
            .map(ascii_json)
            .collect::<Vec<_>>()
            .join(" | "),
        TypeExpr::Union(types) => {
            let mut unique = Vec::<String>::new();
            for rendered in types.iter().map(render) {
                if !unique.contains(&rendered) {
                    unique.push(rendered);
                }
            }
            unique.join(" | ")
        }
        TypeExpr::Null => "null".to_owned(),
        TypeExpr::Boolean => "boolean".to_owned(),
        TypeExpr::Integer | TypeExpr::Number => "number".to_owned(),
        TypeExpr::String => "string".to_owned(),
        TypeExpr::Bytes => "Uint8Array".to_owned(),
        TypeExpr::Tuple(items) => {
            format!(
                "[{}]",
                items.iter().map(render).collect::<Vec<_>>().join(", ")
            )
        }
        TypeExpr::Array(item) => format!("Array<{}>", render(item)),
        TypeExpr::Map(value) => format!("Record<string, {}>", render(value)),
    };
}
