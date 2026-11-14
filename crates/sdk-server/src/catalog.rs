//! Schemas of every server method and the model types they use, for client generators.

use crate::failure::ErrorData;
use crate::methods::{self, termtrace::InitializeParams, termtrace::InitializeResult};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use termtrace_sdk::contracts::types::input_generator;

/// One method and the schemas of its params and result.
#[derive(Debug, Clone)]
pub struct MethodSchema {
    /// Method name, such as `screen.frame`.
    pub name: String,
    /// What the method does.
    pub description: String,
    /// Params schema, usually a `$ref` into the definitions.
    pub params: Value,
    /// Result schema.
    pub result: Value,
}

/// Every method plus the definitions their schemas reference.
#[derive(Debug, Clone)]
pub struct Catalog {
    /// Methods sorted by name, `initialize` included.
    pub methods: Vec<MethodSchema>,
    /// Definitions by schema name: the SDK type catalog plus the server's own types.
    pub definitions: Map<String, Value>,
    /// Schema of the `data` of a Termtrace error response.
    pub error_data: Value,
}

/// Why the catalog could not be built.
#[derive(Debug)]
pub struct CatalogError(String);

impl Display for CatalogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        return f.write_str(&self.0);
    }
}

impl Error for CatalogError {}

/// Build the catalog.
///
/// # Errors
///
/// Fails when a server type shares its schema name with another type.
pub fn catalog() -> Result<Catalog, CatalogError> {
    let mut generator = input_generator();
    let sdk_names = generator
        .definitions()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut methods = vec![MethodSchema {
        name: methods::INITIALIZE.to_owned(),
        description: "Bind the server to a Termtrace binary and evidence root and report its \
                      version."
            .to_owned(),
        params: generator.subschema_for::<InitializeParams>().to_value(),
        result: generator.subschema_for::<InitializeResult>().to_value(),
    }];
    for (name, entry) in methods::registry() {
        methods.push(MethodSchema {
            name: name.to_owned(),
            description: entry.description.to_owned(),
            params: (entry.params)(&mut generator).to_value(),
            result: (entry.result)(&mut generator).to_value(),
        });
    }

    methods.sort_by(|left, right| return left.name.cmp(&right.name));

    let error_data = generator.subschema_for::<ErrorData>().to_value();
    let definitions = generator.take_definitions(true);

    for name in definitions
        .keys()
        .filter(|name| return !sdk_names.contains(*name))
    {
        let stem = name.trim_end_matches(|character: char| return character.is_ascii_digit());
        if stem.len() < name.len() && definitions.contains_key(stem) {
            return Err(CatalogError(format!(
                "server type `{stem}` shares its name with another type and was renamed `{name}`"
            )));
        }
    }

    return Ok(Catalog {
        methods,
        definitions,
        error_data,
    });
}
