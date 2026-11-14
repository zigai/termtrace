use super::session::SessionRef;
use super::{method, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;
use termtrace_sdk::contracts::control::{
    guard, parse_request_id, AtomSpec, ConditionSpec, ProjectionSpec,
};
use termtrace_sdk::input::{ActOutcome, ActRequest, ActionCondition};

/// One input action: atoms, pacing, guard, and an optional postcondition.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActionSpec {
    /// Identity of this action; resending it returns the first outcome instead of typing twice.
    request_id: String,
    /// Apply only while the presented state has this hash.
    #[serde(default)]
    if_state: Option<String>,
    /// Input in order.
    atoms: Vec<AtomSpec>,
    /// Delay between atoms; none by default.
    #[serde(default)]
    pace_ms: Option<u64>,
    /// Free-form intent recorded with the input.
    #[serde(default)]
    intent: Option<String>,
    /// Report the outcome of every atom.
    #[serde(default)]
    verbose: bool,
    /// Return an unsettled postcondition instead of failing.
    #[serde(default)]
    allow_incomplete: bool,
    /// Detail and region of the returned frame.
    #[serde(default)]
    projection: ProjectionSpec,
    /// Postcondition; none by default.
    #[serde(default)]
    until: Option<ConditionSpec>,
}

impl ActionSpec {
    fn request(&self) -> termtrace_sdk::Result<ActRequest> {
        let request_id = parse_request_id(&self.request_id)?;

        return Ok(ActRequest {
            atoms: self
                .atoms
                .iter()
                .map(AtomSpec::to_atom)
                .collect::<termtrace_sdk::Result<Vec<_>>>()?,
            guard: guard(Some(request_id), self.if_state.clone()),
            intent: self.intent.clone(),
            pace: self.pace_ms.map_or(Duration::ZERO, Duration::from_millis),
            verbose_atom_outcomes: self.verbose,
            postcondition: self
                .until
                .as_ref()
                .map_or(Ok(ActionCondition::None), ConditionSpec::to_condition)?,
            projection: self.projection.into(),
            allow_incomplete: self.allow_incomplete,
        });
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunParams {
    session: SessionRef,
    action: ActionSpec,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "action.run",
        "Apply one input action and check its postcondition.",
        run,
    );
}

fn run(call: &Call<'_>, params: RunParams) -> Result<ActOutcome, Failure> {
    let request = params
        .action
        .request()
        .map_err(|error| return Failure::sdk(&error))?;

    let request_id = request.guard.request_id.unwrap_or_default();
    let session = params
        .session
        .attach_as(call, |error| return Failure::mutation(error, request_id))?;

    return session
        .act(request)
        .map_err(|error| return Failure::mutation(&error, request_id));
}
