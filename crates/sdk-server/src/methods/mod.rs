pub(crate) mod action;
pub(crate) mod evidence;
pub(crate) mod instrumentation;
pub(crate) mod locator;
pub(crate) mod media;
pub(crate) mod original;
pub(crate) mod recording;
pub(crate) mod registry;
pub(crate) mod screen;
pub(crate) mod session;
pub(crate) mod system;
pub(crate) mod termtrace;
pub(crate) mod testing;
pub(crate) mod tmux;

use crate::failure::Failure;
use crate::server::Call;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::value::{to_raw_value, RawValue};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

pub(crate) const INITIALIZE: &str = "initialize";

type Invoke = dyn Fn(&Call<'_>, &RawValue) -> Result<Box<RawValue>, Failure> + Send + Sync;

/// One callable method and the schemas of its params and result.
pub(crate) struct Entry {
    pub(crate) description: &'static str,
    pub(crate) invoke: Box<Invoke>,
    pub(crate) params: fn(&mut SchemaGenerator) -> Schema,
    pub(crate) result: fn(&mut SchemaGenerator) -> Schema,
}

pub(crate) type Registry = BTreeMap<&'static str, Arc<Entry>>;

/// Register `call` under `name`; params decode from the request and the result encodes back.
fn method<P, R>(
    registry: &mut Registry,
    name: &'static str,
    description: &'static str,
    call: fn(&Call<'_>, P) -> Result<R, Failure>,
) where
    P: DeserializeOwned + JsonSchema + 'static,
    R: Serialize + JsonSchema + 'static,
{
    let invoke = move |context: &Call<'_>, params: &RawValue| {
        let params: P = serde_json::from_str(params.get())
            .map_err(|error| return Failure::invalid_params(format!("{name}: {error}")))?;
        let result = call(context, params)?;

        return to_raw_value(&result)
            .map_err(|error| return Failure::internal(format!("{name}: {error}")));
    };
    let entry = Entry {
        description,
        invoke: Box::new(invoke),
        params: |generator| return generator.subschema_for::<P>(),
        result: |generator| return generator.subschema_for::<R>(),
    };
    let previous = registry.insert(name, Arc::new(entry));
    debug_assert!(previous.is_none(), "method {name} is registered twice");
}

pub(crate) fn registry() -> Registry {
    let mut registry = Registry::new();
    termtrace::register(&mut registry);
    recording::register(&mut registry);
    session::register(&mut registry);
    screen::register(&mut registry);
    locator::register(&mut registry);
    action::register(&mut registry);
    system::register(&mut registry);
    evidence::register(&mut registry);
    self::registry::register(&mut registry);
    media::register(&mut registry);
    original::register(&mut registry);
    instrumentation::register(&mut registry);
    tmux::register(&mut registry);
    testing::register(&mut registry);
    return registry;
}

fn millis(milliseconds: Option<u64>, default: Duration) -> Duration {
    return milliseconds.map_or(default, Duration::from_millis);
}
