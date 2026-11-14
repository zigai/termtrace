//! Language-neutral declarations resolved from catalog schemas.
//!
//! Each named schema becomes one declaration. Inline object schemas, including the object
//! branches of a union, get their own declarations named after the path that reaches them, so
//! the binding generators only render names and type expressions.

use serde_json::{Map, Value};
use std::{collections::BTreeMap, fmt::Write as _};

const FRAME_EXTENSION: &str = "x-termtrace-frame";
const EXPLICIT_NULL_EXTENSION: &str = "x-termtrace-explicit-null";
const WIDE_FORMATS: [&str; 6] = ["int64", "uint64", "int128", "uint128", "int", "uint"];

/// An integer that may exceed the exact range of a double.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WideInteger {
    /// Whether negative values are allowed.
    pub signed: bool,
    /// Bit width: 64 or 128.
    pub bits: u16,
    /// Whether the value is carried as decimal text instead of a JSON number.
    pub decimal_text: bool,
}

impl WideInteger {
    fn from_format(format: &str, decimal_text: bool) -> Self {
        return Self {
            signed: !format.starts_with('u'),
            bits: if format.ends_with("128") { 128 } else { 64 },
            decimal_text,
        };
    }
}

/// A schema the declaration resolver cannot express.
#[derive(Debug, thiserror::Error)]
#[error("{name}: {reason}")]
pub struct DeclarationError {
    /// Declaration or type path that holds the schema.
    pub name: String,
    /// What the schema uses that has no declaration form.
    pub reason: String,
}

/// A type as referenced from a declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// Any JSON value.
    Json,
    /// A type with no values.
    Never,
    /// A declared type.
    Named(String),
    /// A filesystem path.
    Path,
    /// An integer that may exceed the exact range of a double.
    WideInteger(WideInteger),
    /// One of these JSON values.
    Literal(Vec<Value>),
    /// Any of these types, in schema order and possibly repeated.
    Union(Vec<TypeExpr>),
    /// JSON `null`.
    Null,
    /// A boolean.
    Boolean,
    /// An integer within the exact range of a double.
    Integer,
    /// A floating-point number.
    Number,
    /// A string.
    String,
    /// Bytes carried as base64 text.
    Bytes,
    /// A fixed-length array with one type per position.
    Tuple(Vec<TypeExpr>),
    /// An array of one item type.
    Array(Box<TypeExpr>),
    /// An object keyed by strings with one value type.
    Map(Box<TypeExpr>),
}

/// One field of an object declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// JSON property name.
    pub name: String,
    /// Schema description of the property.
    pub description: Option<String>,
    /// Whether the property is always present.
    pub required: bool,
    /// Value the reader assumes when the property is absent.
    pub default: Option<Value>,
    /// Whether an explicit null means something different from an absent property.
    pub explicit_null: bool,
    /// Property type.
    pub value: TypeExpr,
}

/// What a declaration defines.
#[derive(Debug, Clone, PartialEq)]
pub enum DeclarationBody {
    /// An object with named fields, in schema order.
    Object {
        /// Declared fields.
        fields: Vec<Field>,
        /// Whether the native binding attaches its screen frame handle as `frame`.
        frame: bool,
    },
    /// Another name for a type expression.
    Alias(TypeExpr),
}

/// One named type to generate.
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    /// Schema description of the type.
    pub description: Option<String>,
    /// Declared shape.
    pub body: DeclarationBody,
}

/// Resolve schemas into declarations sorted by name.
///
/// Schemas are resolved in the given order and the first schema to claim a name keeps it.
/// `refs` renames `$ref` targets; a target without an entry keeps its definition name.
///
/// # Errors
///
/// Fails when a schema is neither an object nor a boolean, or combines `allOf` parts.
pub fn declarations<'schema>(
    schemas: impl IntoIterator<Item = (String, &'schema Value)>,
    refs: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Declaration>, DeclarationError> {
    let mut resolver = Resolver {
        refs,
        declared: BTreeMap::new(),
    };
    for (name, schema) in schemas {
        resolver.define(&name, &Json::from_value(schema))?;
    }

    return Ok(resolver
        .declared
        .into_iter()
        .filter_map(|(name, declaration)| {
            return declaration.map(|declaration| return (name, declaration));
        })
        .collect());
}

/// JSON text with every character outside printable ASCII escaped, which Python and
/// JavaScript sources both accept as a string literal.
#[must_use]
pub fn ascii_json(value: &Value) -> String {
    return match value {
        Value::String(text) => ascii_json_string(text),
        Value::Array(items) => {
            let items = items.iter().map(ascii_json).collect::<Vec<_>>();

            format!("[{}]", items.join(", "))
        }
        Value::Object(map) => {
            let entries = map
                .iter()
                .map(|(key, value)| {
                    return format!("{}: {}", ascii_json_string(key), ascii_json(value));
                })
                .collect::<Vec<_>>();

            format!("{{{}}}", entries.join(", "))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => value.to_string(),
    };
}

fn ascii_json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');

    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            _ => {
                let mut units = [0_u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    let _infallible = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }

    out.push('"');
    return out;
}

/// A schema node whose objects keep key order, so merged union branches list the shared
/// properties first, as the generated declarations always have.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Scalar(Value),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn from_value(value: &Value) -> Self {
        return match value {
            Value::Array(items) => Self::Array(items.iter().map(Self::from_value).collect()),
            Value::Object(map) => Self::Object(
                map.iter()
                    .map(|(key, value)| return (key.clone(), Self::from_value(value)))
                    .collect(),
            ),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                Self::Scalar(value.clone())
            }
        };
    }

    fn to_value(&self) -> Value {
        return match self {
            Self::Scalar(value) => value.clone(),
            Self::Array(items) => Value::Array(items.iter().map(Self::to_value).collect()),
            Self::Object(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(key, value)| return (key.clone(), value.to_value()))
                    .collect::<Map<_, _>>(),
            ),
        };
    }

    fn get(&self, key: &str) -> Option<&Self> {
        let Self::Object(entries) = self else {
            return None;
        };
        return entries
            .iter()
            .find(|(name, _)| return name == key)
            .map(|(_, value)| return value);
    }

    fn has(&self, key: &str) -> bool {
        return self.get(key).is_some();
    }

    fn entries(&self) -> &[(String, Self)] {
        return match self {
            Self::Object(entries) => entries,
            Self::Scalar(_) | Self::Array(_) => &[],
        };
    }

    fn items(&self) -> &[Self] {
        return match self {
            Self::Array(items) => items,
            Self::Scalar(_) | Self::Object(_) => &[],
        };
    }

    fn as_str(&self) -> Option<&str> {
        return match self {
            Self::Scalar(Value::String(text)) => Some(text),
            Self::Scalar(_) | Self::Array(_) | Self::Object(_) => None,
        };
    }

    fn truthy(&self) -> bool {
        return match self {
            Self::Scalar(Value::Null) => false,
            Self::Scalar(Value::Bool(value)) => *value,
            Self::Scalar(Value::Number(number)) => {
                number.as_f64().is_some_and(|value| return value != 0.0)
            }
            Self::Scalar(Value::String(text)) => !text.is_empty(),
            Self::Scalar(Value::Array(_) | Value::Object(_)) => true,
            Self::Array(items) => !items.is_empty(),
            Self::Object(entries) => !entries.is_empty(),
        };
    }

    fn text(&self) -> String {
        return match self {
            Self::Scalar(Value::String(text)) => text.clone(),
            Self::Scalar(Value::Bool(true)) => "True".to_owned(),
            Self::Scalar(Value::Bool(false)) => "False".to_owned(),
            Self::Scalar(Value::Null) => "None".to_owned(),
            Self::Scalar(_) | Self::Array(_) | Self::Object(_) => self.to_value().to_string(),
        };
    }

    fn with(&self, key: &str, value: Self) -> Self {
        let mut entries = self.entries().to_vec();
        insert(&mut entries, key, value);
        return Self::Object(entries);
    }
}

fn insert(entries: &mut Vec<(String, Json)>, key: &str, value: Json) {
    if let Some(slot) = entries.iter_mut().find(|(name, _)| return name == key) {
        slot.1 = value;
    } else {
        entries.push((key.to_owned(), value));
    }
}

fn is_object(schema: &Json) -> bool {
    return matches!(schema, Json::Object(_))
        && schema.has("properties")
        && !["oneOf", "anyOf", "allOf"]
            .iter()
            .any(|key| return schema.has(key));
}

fn pascal(text: &str) -> String {
    return text
        .split(|character: char| return !character.is_ascii_alphanumeric())
        .filter(|part| return !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            let first = characters
                .next()
                .map(|first| return first.to_ascii_uppercase());
            return first.into_iter().chain(characters).collect::<String>();
        })
        .collect();
}

fn variant_name(parent: &str, branch: &Json, index: usize) -> String {
    let properties = branch.get("properties").map_or(&[][..], Json::entries);
    for (_, spec) in properties {
        if let Some(constant) = spec.get("const") {
            return format!("{parent}{}", pascal(&constant.text()));
        }
    }

    let required = branch.get("required").map_or(&[][..], Json::items);
    if let [only] = required {
        let only = only.text();
        if !properties.is_empty() && properties.iter().all(|(name, _)| return *name == only) {
            return format!("{parent}{}", pascal(&only));
        }
    }

    return format!("{parent}Variant{index}");
}

fn description(schema: &Json) -> Option<String> {
    return schema
        .get("description")
        .and_then(Json::as_str)
        .map(str::to_owned);
}

struct Resolver<'names> {
    refs: &'names BTreeMap<String, String>,
    declared: BTreeMap<String, Option<Declaration>>,
}

impl Resolver<'_> {
    fn define(&mut self, name: &str, schema: &Json) -> Result<(), DeclarationError> {
        if self.declared.contains_key(name) {
            return Ok(());
        }

        self.declared.insert(name.to_owned(), None);
        let body = if is_object(schema) {
            self.object(name, schema)?
        } else {
            DeclarationBody::Alias(self.expr(schema, name)?)
        };

        let declaration = Declaration {
            description: description(schema),
            body,
        };
        self.declared.insert(name.to_owned(), Some(declaration));
        return Ok(());
    }

    fn object(&mut self, name: &str, schema: &Json) -> Result<DeclarationBody, DeclarationError> {
        let required = schema.get("required").map_or(&[][..], Json::items);
        let mut fields = Vec::new();
        for (field, spec) in schema.get("properties").map_or(&[][..], Json::entries) {
            let value = self.expr(spec, &format!("{name}{}", pascal(field)))?;
            fields.push(Field {
                name: field.clone(),
                description: description(spec),
                required: required
                    .iter()
                    .any(|item| return item.as_str() == Some(field)),
                default: spec.get("default").map(Json::to_value),
                explicit_null: spec.get(EXPLICIT_NULL_EXTENSION).is_some_and(Json::truthy),
                value,
            });
        }

        return Ok(DeclarationBody::Object {
            fields,
            frame: schema.get(FRAME_EXTENSION).is_some_and(Json::truthy),
        });
    }

    fn expr(&mut self, schema: &Json, name: &str) -> Result<TypeExpr, DeclarationError> {
        match schema {
            Json::Scalar(Value::Bool(true)) => return Ok(TypeExpr::Json),
            Json::Object(entries) if entries.is_empty() => return Ok(TypeExpr::Json),
            Json::Scalar(Value::Bool(false)) => return Ok(TypeExpr::Never),
            Json::Object(_) => {}
            Json::Scalar(_) | Json::Array(_) => {
                return Err(DeclarationError {
                    name: name.to_owned(),
                    reason: format!("unsupported schema {}", schema.to_value()),
                });
            }
        }

        if let Some(reference) = schema.get("$ref") {
            let target = reference.text();
            let target = target.rsplit('/').next().unwrap_or_default();
            let target = self.refs.get(target).map_or(target, String::as_str);

            return Ok(TypeExpr::Named(target.to_owned()));
        }

        if schema.get("x-termtrace-path").is_some_and(Json::truthy) {
            return Ok(TypeExpr::Path);
        }

        if let Some(format) = schema
            .get("x-termtrace-native-integer")
            .filter(|native| return native.truthy())
        {
            let format = format.as_str().unwrap_or("uint64");
            let decimal_text = schema.get("type").and_then(Json::as_str) == Some("string");

            return Ok(TypeExpr::WideInteger(WideInteger::from_format(
                format,
                decimal_text,
            )));
        }

        if let Some(constant) = schema.get("const") {
            return Ok(TypeExpr::Literal(vec![constant.to_value()]));
        }

        if let Some(values) = schema.get("enum") {
            return Ok(TypeExpr::Literal(
                values.items().iter().map(Json::to_value).collect(),
            ));
        }

        for union in ["oneOf", "anyOf"] {
            if let Some(branches) = schema.get(union) {
                let mut types = Vec::new();
                for (index, branch) in branches.items().iter().enumerate() {
                    types.push(self.branch(name, schema, branch, index)?);
                }

                return Ok(TypeExpr::Union(types));
            }
        }

        if let Some(parts) = schema.get("allOf") {
            let [part] = parts.items() else {
                return Err(DeclarationError {
                    name: name.to_owned(),
                    reason: format!("allOf with {} parts is not supported", parts.items().len()),
                });
            };

            return self.expr(part, name);
        }

        let kind = schema.get("type");
        if let Some(Json::Array(kinds)) = kind {
            let mut types = Vec::new();
            for kind in kinds {
                types.push(self.expr(&schema.with("type", kind.clone()), name)?);
            }

            return Ok(TypeExpr::Union(types));
        }

        let format = schema.get("format").and_then(Json::as_str);
        let expr = match kind.and_then(Json::as_str) {
            Some("null") => TypeExpr::Null,
            Some("boolean") => TypeExpr::Boolean,
            Some("integer")
                if let Some(format) =
                    format.filter(|format| return WIDE_FORMATS.contains(format)) =>
            {
                TypeExpr::WideInteger(WideInteger::from_format(format, false))
            }
            Some("integer") => TypeExpr::Integer,
            Some("number") => TypeExpr::Number,
            Some("string")
                if schema.get("contentEncoding").and_then(Json::as_str) == Some("base64") =>
            {
                TypeExpr::Bytes
            }
            Some("string") => TypeExpr::String,
            Some("array") => self.array(schema, name)?,
            kind if kind == Some("object") || schema.has("properties") => {
                if !schema.has("properties") {
                    let value = schema
                        .get("additionalProperties")
                        .cloned()
                        .unwrap_or(Json::Object(Vec::new()));
                    let value = self.expr(&value, &format!("{name}Value"))?;

                    return Ok(TypeExpr::Map(Box::new(value)));
                }

                self.define(name, schema)?;

                TypeExpr::Named(name.to_owned())
            }
            _ => TypeExpr::Json,
        };
        return Ok(expr);
    }

    fn array(&mut self, schema: &Json, name: &str) -> Result<TypeExpr, DeclarationError> {
        if let Some(items) = schema.get("prefixItems") {
            let mut types = Vec::new();
            for (index, item) in items.items().iter().enumerate() {
                types.push(self.expr(item, &format!("{name}Item{index}"))?);
            }

            return Ok(TypeExpr::Tuple(types));
        }

        let item = schema
            .get("items")
            .cloned()
            .unwrap_or(Json::Object(Vec::new()));
        let item = self.expr(&item, &format!("{name}Item"))?;

        return Ok(TypeExpr::Array(Box::new(item)));
    }

    fn branch(
        &mut self,
        parent: &str,
        schema: &Json,
        branch: &Json,
        index: usize,
    ) -> Result<TypeExpr, DeclarationError> {
        let branch = match (schema.get("properties"), branch) {
            (Some(shared), Json::Object(_)) => merge(schema, shared, branch),
            _ => branch.clone(),
        };
        if is_object(&branch) {
            let name = variant_name(parent, &branch, index);
            self.define(&name, &branch)?;

            return Ok(TypeExpr::Named(name));
        }

        return self.expr(&branch, &format!("{parent}Variant{index}"));
    }
}

fn merge(schema: &Json, shared: &Json, branch: &Json) -> Json {
    let mut properties = shared.entries().to_vec();
    for (key, value) in branch.get("properties").map_or(&[][..], Json::entries) {
        insert(&mut properties, key, value.clone());
    }

    let mut required = Vec::<Json>::new();
    for item in schema
        .get("required")
        .map_or(&[][..], Json::items)
        .iter()
        .chain(branch.get("required").map_or(&[][..], Json::items))
    {
        if !required.contains(item) {
            required.push(item.clone());
        }
    }

    return branch
        .with("properties", Json::Object(properties))
        .with("required", Json::Array(required));
}
