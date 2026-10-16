//! Environment diagnosis reported by `termtrace doctor`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Outcome of one diagnostic check.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    /// The check passed.
    Ok,
    /// A neutral fact, such as an optional component that is absent.
    Info,
    /// Works now but is degraded or likely to break.
    Warn,
    /// The core path or a required feature does not work.
    Fail,
    /// The check did not run.
    Skip,
}

/// Repair for a problem a check found.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorFix {
    /// Shell command that repairs the problem.
    pub command: Option<String>,
    /// Manual step when no single command repairs the problem.
    pub manual: Option<String>,
    /// Whether `termtrace doctor --fix` applies the repair.
    pub automatic: bool,
}

/// Result of one diagnostic check.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorCheck {
    /// Stable identifier, `<group>.<name>`.
    pub id: String,
    /// Heading of the group the check belongs to.
    pub group: String,
    /// Check outcome.
    pub status: DoctorStatus,
    /// Stable diagnostic code of the underlying probe, when it reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// One-line finding.
    pub message: String,
    /// One line per finding when the check found several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    /// Checks whose result caused this check to be skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// Repair for the finding, when one is known.
    pub fix: Option<DoctorFix>,
}

/// Build identity of the running executable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorBuild {
    /// Source revision the executable was built from.
    pub revision: String,
    /// Target operating system and architecture, `<os>/<arch>`.
    pub target: String,
}

/// Location of the running executable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorInstall {
    /// Running executable, with the home directory shown as `~`.
    pub executable: Option<String>,
    /// Installer that placed the executable, when recognizable.
    pub method: Option<String>,
}

/// A resolved directory and the setting that selected it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorPath {
    /// Directory, with the home directory shown as `~`.
    pub path: String,
    /// Environment variable that selected the directory; absent for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Directories resolved for this run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorPaths {
    /// Session registry and managed-session state root; absent when none can be selected.
    pub state: Option<DoctorPath>,
}

/// Count of checks per status.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorSummary {
    /// Checks that passed.
    pub ok: u32,
    /// Neutral findings.
    pub info: u32,
    /// Degraded findings.
    pub warn: u32,
    /// Failures.
    pub fail: u32,
    /// Checks that did not run.
    pub skip: u32,
}

/// Outcome of applying one repair.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DoctorFixResult {
    /// The repair completed.
    Applied,
    /// The repair failed.
    Failed,
}

/// One repair applied by `termtrace doctor --fix`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorFixOutcome {
    /// Check the repair belongs to.
    pub id: String,
    /// Action that was taken.
    pub action: String,
    /// Whether the action succeeded.
    pub result: DoctorFixResult,
    /// Failure message when the action failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Complete environment diagnosis.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoctorReport {
    /// Package version.
    pub version: String,
    /// Build identity.
    pub build: DoctorBuild,
    /// Running executable.
    pub install: DoctorInstall,
    /// Directories resolved for this run.
    pub paths: DoctorPaths,
    /// Termtrace environment variables that are set, by name.
    pub env: BTreeMap<String, String>,
    /// Checks in report order.
    pub checks: Vec<DoctorCheck>,
    /// Count of checks per status.
    pub summary: DoctorSummary,
    /// Repairs applied by `--fix`, in the order they ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<DoctorFixOutcome>,
}
