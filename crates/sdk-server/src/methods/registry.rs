use super::{method, millis, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::Deserialize;
use termtrace_model::execution::environment::EnvironmentSpec;
use termtrace_model::registry::{
    GarbageCollectionReport, RegistryEntry, RegistryListEntry, RegistrySessionResolution,
    RestartConfiguration, RestartReport,
};
use termtrace_model::value::ExactPath;
use termtrace_sdk::contracts::diagnostics as core;
use termtrace_sdk::defaults;
use termtrace_sdk::registry::RegistryService;

/// Which session registry a call uses.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RegistryRoot {
    /// Registry directory; the default registry when absent.
    #[serde(default)]
    root: Option<ExactPath>,
}

impl RegistryRoot {
    fn open(&self) -> Result<RegistryService, Failure> {
        let registry = self
            .root
            .as_ref()
            .map_or_else(RegistryService::open_default, |root| {
                return RegistryService::open(root.0.clone());
            });
        return registry.map_err(|error| return Failure::sdk(&error));
    }
}

/// Params of `registry.list`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(flatten)]
    registry: RegistryRoot,
    /// Whether to include sessions that are no longer running.
    #[serde(default)]
    include_all: bool,
}

/// Params of `registry.register`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RegisterParams {
    #[serde(flatten)]
    registry: RegistryRoot,
    /// Alias to create.
    alias: String,
    /// Session directory.
    session: ExactPath,
}

/// Params of calls that address one alias.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AliasParams {
    #[serde(flatten)]
    registry: RegistryRoot,
    /// Session alias.
    alias: String,
}

/// Params of `registry.restart`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RestartParams {
    #[serde(flatten)]
    registry: RegistryRoot,
    /// Session alias.
    alias: String,
    /// What the successor may change.
    #[serde(default)]
    options: RestartConfiguration,
    /// Successor environment; the predecessor's when absent.
    #[serde(default)]
    environment: Option<EnvironmentSpec>,
    /// Bound on successor readiness; the default startup timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Params of `registry.gc`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GcParams {
    #[serde(flatten)]
    registry: RegistryRoot,
    /// Age in days past which completed sessions are removed; 30 when absent.
    #[serde(default)]
    older_than_days: Option<u64>,
    /// Number of latest sessions always kept; 20 when absent.
    #[serde(default)]
    keep_latest: Option<u64>,
    /// Byte quota for completed sessions.
    #[serde(default)]
    max_completed_bytes: Option<u64>,
    /// Whether to delete; the call only plans when false.
    #[serde(default)]
    apply: bool,
}

pub(super) fn register(registry: &mut Registry) {
    method(registry, "registry.list", "List registered sessions.", list);
    method(
        registry,
        "registry.register",
        "Register a session under an alias.",
        register_alias,
    );
    method(
        registry,
        "registry.resolve",
        "Resolve an alias; null when no alias record exists.",
        resolve,
    );
    method(
        registry,
        "registry.lifecycle",
        "Report the lifecycle of an alias.",
        lifecycle,
    );
    method(
        registry,
        "registry.remove",
        "Remove an alias; the session evidence stays.",
        remove,
    );
    method(
        registry,
        "registry.restart",
        "Restart the session behind an alias.",
        restart,
    );
    method(
        registry,
        "registry.gc",
        "Plan or apply registry retention.",
        gc,
    );
}

fn sdk<T>(result: termtrace_sdk::Result<T>) -> Result<T, Failure> {
    return result.map_err(|error| return Failure::sdk(&error));
}

fn list(_call: &Call<'_>, params: ListParams) -> Result<Vec<RegistryListEntry>, Failure> {
    return sdk(core::registry_list(
        &params.registry.open()?,
        params.include_all,
    ));
}

fn register_alias(_call: &Call<'_>, params: RegisterParams) -> Result<RegistryEntry, Failure> {
    return sdk(core::registry_register(
        &params.registry.open()?,
        &params.alias,
        &params.session.0,
    ));
}

fn resolve(_call: &Call<'_>, params: AliasParams) -> Result<Option<RegistryEntry>, Failure> {
    return sdk(core::registry_resolve(
        &params.registry.open()?,
        &params.alias,
    ));
}

fn lifecycle(_call: &Call<'_>, params: AliasParams) -> Result<RegistrySessionResolution, Failure> {
    return sdk(core::registry_lifecycle(
        &params.registry.open()?,
        &params.alias,
    ));
}

fn remove(_call: &Call<'_>, params: AliasParams) -> Result<RegistryEntry, Failure> {
    return sdk(core::registry_remove(
        &params.registry.open()?,
        &params.alias,
    ));
}

fn restart(call: &Call<'_>, params: RestartParams) -> Result<RestartReport, Failure> {
    let terminal = call.terminal()?;

    return sdk(core::restart(
        &params.registry.open()?,
        terminal.binary(),
        &params.alias,
        &params.options,
        params.environment.as_ref(),
        millis(params.timeout_ms, defaults::STARTUP_TIMEOUT),
    ));
}

fn gc(_call: &Call<'_>, params: GcParams) -> Result<GarbageCollectionReport, Failure> {
    return sdk(core::registry_gc(
        &params.registry.open()?,
        params.older_than_days.unwrap_or(30),
        params.keep_latest.unwrap_or(20),
        params.max_completed_bytes,
        params.apply,
    ));
}
