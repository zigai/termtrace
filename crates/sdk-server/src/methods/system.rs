use super::{method, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use termtrace_model::protocol::capability::CapabilityReport;
use termtrace_sdk::locator::WaitCancellation;
use termtrace_sdk::system::SystemService;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SystemParams {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SchemaParams {
    /// Schema file name, as listed by `system.schemas`.
    name: String,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "system.capabilities",
        "Probe the installation, dependencies, and media capabilities.",
        |call, _params: SystemParams| return Ok(capabilities(call)),
    );
    method(
        registry,
        "system.versions",
        "Report build and protocol versions.",
        |_call, _params: SystemParams| return Ok(SystemService::versions()),
    );
    method(
        registry,
        "system.schemas",
        "List the public JSON Schema names.",
        schemas,
    );
    method(
        registry,
        "system.schema",
        "Return one public JSON Schema.",
        schema,
    );
}

fn capabilities(call: &Call<'_>) -> CapabilityReport {
    return WaitCancellation::forward(Some(&call.cancellation), SystemService::probe_capabilities);
}

fn schemas(_call: &Call<'_>, _params: SystemParams) -> Result<Vec<String>, Failure> {
    return SystemService::schemas().map_err(|error| return Failure::sdk(&error));
}

fn schema(_call: &Call<'_>, params: SchemaParams) -> Result<Value, Failure> {
    return SystemService::schema(&params.name).map_err(|error| return Failure::sdk(&error));
}
