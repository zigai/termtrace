use schemars::JsonSchema;
use serde::Deserialize;
use std::{collections::BTreeMap, io::Write as _};
use tempfile::NamedTempFile;
use termtrace_model::{execution::environment::EnvironmentSpec, value::ByteString};

pub(super) struct Invocation {
    pub(super) arguments: Vec<String>,
    pub(super) environment: Option<NamedTempFile>,
}

impl From<Vec<String>> for Invocation {
    fn from(arguments: Vec<String>) -> Self {
        return Self {
            arguments,
            environment: None,
        };
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Until {
    /// Wait until the screen stops changing.
    Idle {
        /// Quiet interval in milliseconds; defaults to 500.
        ms: Option<u64>,
    },
    /// Wait until the screen contains this exact text.
    Screen { text: String },
    /// Wait until the screen matches this regular expression.
    ScreenRegex { pattern: String },
}

impl Until {
    fn push(&self, arguments: &mut Vec<String>) {
        match self {
            Self::Idle { ms: None } => arguments.push("--until-idle".to_owned()),
            Self::Idle { ms: Some(ms) } => arguments.push(format!("--until-idle={ms}ms")),
            Self::Screen { text } => push_value(arguments, "--until-screen", text),
            Self::ScreenRegex { pattern } => {
                push_value(arguments, "--until-screen-regex", pattern);
            }
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StartInput {
    /// Command and arguments to run in the recorded terminal.
    command: Vec<String>,
    /// Session name for later calls; defaults to the command name.
    name: Option<String>,
    /// Working directory for the command.
    cwd: Option<String>,
    /// Terminal columns (1-1000); defaults to 160.
    cols: Option<u16>,
    /// Terminal rows (1-1000; at most 250000 cells in total); defaults to 45.
    rows: Option<u16>,
    /// Readiness condition to wait for before returning.
    until: Option<Until>,
    /// Maximum wait for `until` in milliseconds; defaults to 30000.
    timeout_ms: Option<u64>,
    /// Environment variables set for the command, on top of the server's environment.
    #[serde(default)]
    env: BTreeMap<String, String>,
}

impl StartInput {
    pub(super) fn invocation(self) -> Result<Invocation, String> {
        if self.command.is_empty() {
            return Err("command must not be empty".to_owned());
        }

        let mut arguments = vec!["record".to_owned()];
        arguments.push(self.name.map_or_else(
            || return "--detach".to_owned(),
            |name| return format!("--detach={name}"),
        ));
        arguments.push("--json".to_owned());
        push_optional(&mut arguments, "--cwd", self.cwd);
        push_optional(&mut arguments, "--cols", self.cols);
        push_optional(&mut arguments, "--rows", self.rows);

        if let Some(until) = &self.until {
            until.push(&mut arguments);
        }

        push_millis(&mut arguments, "--timeout", self.timeout_ms);

        let environment = environment_spec(self.env)?;
        if let Some(file) = &environment {
            push_value(
                &mut arguments,
                "--environment-spec",
                &file.path().to_string_lossy(),
            );
        }

        arguments.push("--".to_owned());
        arguments.extend(self.command);

        return Ok(Invocation {
            arguments,
            environment,
        });
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SendScreen {
    /// Only the resulting state hash.
    Text,
    /// Only the rows that changed since just before the send.
    Diff,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SendInput {
    /// Session name or path.
    session: String,
    /// Inputs in order: key names such as `enter`, `ctrl+c`, or `up`; `text:VALUE`;
    /// `paste:VALUE`; `raw:VALUE`; `raw-file:PATH`; one printable character;
    /// `click:COL,ROW`; `activate:TEXT`; `resize:COLSxROWS`; `signal:INT`; or `mark:LABEL`.
    inputs: Vec<String>,
    /// Condition to wait for after the input.
    until: Option<Until>,
    /// Maximum wait for `until` in milliseconds; defaults to 30000.
    timeout_ms: Option<u64>,
    /// With idle `until`, how long to wait for the screen to react before reporting
    /// `unchanged`, in milliseconds; defaults to 2000.
    reaction_timeout_ms: Option<u64>,
    /// Apply the input only if the current screen state has this hash.
    if_state: Option<String>,
    /// Reason recorded with the input event.
    intent: Option<String>,
    /// Screen report after the send; defaults to `text`.
    screen: Option<SendScreen>,
}

impl SendInput {
    pub(super) fn arguments(self) -> Result<Vec<String>, String> {
        if self.inputs.is_empty() {
            return Err("inputs must not be empty".to_owned());
        }

        if self.inputs.iter().any(|input| return input == "-") {
            return Err("`-` reads standard input, which carries the MCP protocol".to_owned());
        }

        let mut arguments = vec!["send".to_owned(), self.session, "--json".to_owned()];
        if let Some(until) = &self.until {
            until.push(&mut arguments);
        }

        push_millis(&mut arguments, "--timeout", self.timeout_ms);
        push_millis(
            &mut arguments,
            "--reaction-timeout",
            self.reaction_timeout_ms,
        );
        push_optional(&mut arguments, "--if-state", self.if_state);
        push_optional(&mut arguments, "--intent", self.intent);

        if let Some(screen) = self.screen {
            let screen = match screen {
                SendScreen::Text => "text",
                SendScreen::Diff => "diff",
            };
            push_value(&mut arguments, "--screen", screen);
        }

        arguments.push("--".to_owned());
        arguments.extend(self.inputs);

        return Ok(arguments);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ScreenEncoding {
    /// Trimmed text lines.
    Text,
    /// Complete screen text including trailing blank cells.
    Exact,
    /// Non-empty rows indexed by row number.
    Sparse,
    /// Changes since the `since` state.
    Diff,
    /// No screen text, only state metadata.
    None,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ScreenInput {
    /// Session name or path.
    session: String,
    /// Earlier state hash; reports `unchanged` when the screen still has it.
    since: Option<String>,
    /// Show this earlier state instead of the latest one.
    state_hash: Option<String>,
    /// Screen representation; defaults to `text`.
    encoding: Option<ScreenEncoding>,
    /// Require the live screen to stay unchanged for this many milliseconds first.
    stable_for_ms: Option<u64>,
    /// Maximum wait for `stable_for_ms` in milliseconds; defaults to 30000.
    timeout_ms: Option<u64>,
}

impl ScreenInput {
    pub(super) fn arguments(self) -> Vec<String> {
        let mut arguments = vec!["screen".to_owned(), self.session, "--json".to_owned()];
        push_optional(&mut arguments, "--since", self.since);
        push_optional(&mut arguments, "--state-hash", self.state_hash);

        if let Some(encoding) = self.encoding {
            let encoding = match encoding {
                ScreenEncoding::Text => "text",
                ScreenEncoding::Exact => "exact",
                ScreenEncoding::Sparse => "sparse",
                ScreenEncoding::Diff => "diff",
                ScreenEncoding::None => "none",
            };
            push_value(&mut arguments, "--screen", encoding);
        }

        push_millis(&mut arguments, "--stable-for", self.stable_for_ms);
        push_millis(&mut arguments, "--timeout", self.timeout_ms);

        return arguments;
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum WaitFor {
    /// The screen contains this exact text.
    Screen { text: String },
    /// The screen does not contain this exact text.
    ScreenAbsent { text: String },
    /// The screen matches this regular expression.
    ScreenRegex { pattern: String },
    /// The program's output history contains this exact text.
    Output { text: String },
    /// The screen stops changing.
    Idle {
        /// Quiet interval in milliseconds; defaults to 500.
        ms: Option<u64>,
    },
    /// The screen state differs from this hash.
    StateChanged { state_hash: String },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WaitInput {
    /// Session name or path.
    session: String,
    /// Condition to wait for.
    r#for: WaitFor,
    /// Maximum wait in milliseconds; defaults to 30000.
    timeout_ms: Option<u64>,
}

impl WaitInput {
    pub(super) fn arguments(self) -> Vec<String> {
        let mut arguments = vec!["wait".to_owned(), self.session, "--json".to_owned()];
        match self.r#for {
            WaitFor::Screen { text } => push_value(&mut arguments, "--screen", &text),
            WaitFor::ScreenAbsent { text } => {
                push_value(&mut arguments, "--screen-absent", &text);
            }
            WaitFor::ScreenRegex { pattern } => {
                push_value(&mut arguments, "--screen-regex", &pattern);
            }
            WaitFor::Output { text } => push_value(&mut arguments, "--output", &text),
            WaitFor::Idle { ms: None } => arguments.push("--idle".to_owned()),
            WaitFor::Idle { ms: Some(ms) } => arguments.push(format!("--idle={ms}ms")),
            WaitFor::StateChanged { state_hash } => {
                push_value(&mut arguments, "--state-changed", &state_hash);
            }
        }

        push_millis(&mut arguments, "--timeout", self.timeout_ms);

        return arguments;
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusInput {
    /// Session name or path; omit to list all sessions.
    session: Option<String>,
}

impl StatusInput {
    pub(super) fn arguments(self) -> Vec<String> {
        let mut arguments = vec!["status".to_owned(), "--json".to_owned()];
        arguments.extend(self.session);

        return arguments;
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StopInput {
    /// Session name or path.
    session: String,
    /// Wait until the recording is complete and validated, so analysis tools can read it;
    /// defaults to true.
    wait: Option<bool>,
    /// Maximum wait in milliseconds; defaults to 30000.
    timeout_ms: Option<u64>,
}

impl StopInput {
    pub(super) fn arguments(self) -> Vec<String> {
        let mut arguments = vec!["stop".to_owned(), self.session, "--json".to_owned()];
        if self.wait.unwrap_or(true) {
            arguments.push("--wait".to_owned());
        }

        push_optional(&mut arguments, "--timeout-ms", self.timeout_ms);

        return arguments;
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum AnalyzeKind {
    /// Summary and findings for the last input action.
    #[default]
    Triage,
    /// Operations and state transitions in order.
    Timeline,
    /// Differences between two states.
    Diff,
    /// Terminal control sequences and state around one state.
    Explain,
    /// Rendering diagnostics.
    Diagnose,
    /// Integrity verification of the recording.
    Verify,
    /// The program's output with control sequences removed.
    Output,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AnalyzeInput {
    /// Completed session name or path.
    session: String,
    /// Report to produce; defaults to `triage`.
    #[serde(default)]
    kind: AnalyzeKind,
    /// Earlier state hash for `diff`.
    from_state_hash: Option<String>,
    /// Later state hash for `diff`.
    to_state_hash: Option<String>,
    /// Target state hash for `explain`.
    state_hash: Option<String>,
    /// Maximum records for `timeline`, `explain`, `diagnose`, and `verify`; defaults to 100.
    limit: Option<usize>,
    /// Records to skip for `timeline`, `explain`, `diagnose`, and `verify`.
    offset: Option<usize>,
}

impl AnalyzeInput {
    pub(super) fn arguments(self) -> Result<Vec<String>, String> {
        let kind = self.kind;
        let diff = matches!(kind, AnalyzeKind::Diff);

        let explain = matches!(kind, AnalyzeKind::Explain);

        let paged = matches!(
            kind,
            AnalyzeKind::Timeline
                | AnalyzeKind::Explain
                | AnalyzeKind::Diagnose
                | AnalyzeKind::Verify
        );

        if !diff && (self.from_state_hash.is_some() || self.to_state_hash.is_some()) {
            return Err("from_state_hash and to_state_hash apply only to diff".to_owned());
        }

        if !explain && self.state_hash.is_some() {
            return Err("state_hash applies only to explain".to_owned());
        }

        if !paged && (self.limit.is_some() || self.offset.is_some()) {
            return Err(
                "limit and offset apply only to timeline, explain, diagnose, and verify".to_owned(),
            );
        }

        let mut arguments = vec!["analyze".to_owned()];
        let format = match kind {
            AnalyzeKind::Triage => {
                arguments.extend([self.session, "--json".to_owned()]);

                return Ok(arguments);
            }
            AnalyzeKind::Timeline => ["timeline", "--jsonl"],
            AnalyzeKind::Diff => ["diff", "--format=json"],
            AnalyzeKind::Explain => ["explain", "--format=json"],
            AnalyzeKind::Diagnose => ["diagnose", "--json"],
            AnalyzeKind::Verify => ["verify", "--json"],
            AnalyzeKind::Output => ["output", "--strip-controls"],
        };

        let [subcommand, format] = format;
        arguments.extend([subcommand.to_owned(), self.session, format.to_owned()]);
        push_optional(&mut arguments, "--from-state-hash", self.from_state_hash);
        push_optional(&mut arguments, "--to-state-hash", self.to_state_hash);
        push_optional(&mut arguments, "--state-hash", self.state_hash);
        push_optional(&mut arguments, "--limit", self.limit);
        push_optional(&mut arguments, "--offset", self.offset);

        return Ok(arguments);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum Renderer {
    /// Rendered from the recorded terminal state; needs a completed session.
    Semantic,
    /// Actual Ghostty pixels; also works on a live session's latest screen.
    Native,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ScreenshotInput {
    /// Session name or path.
    session: String,
    /// New directory that receives the PNG and its manifest.
    out: String,
    /// State to capture; defaults to the latest presented screen.
    state_hash: Option<String>,
    /// Image source; defaults to `semantic`.
    renderer: Option<Renderer>,
}

impl ScreenshotInput {
    pub(super) fn arguments(self) -> Vec<String> {
        let mut arguments = vec![
            "screenshot".to_owned(),
            self.session,
            "--json".to_owned(),
            "--out".to_owned(),
            self.out,
        ];
        push_optional(&mut arguments, "--state-hash", self.state_hash);

        if let Some(renderer) = self.renderer {
            let renderer = match renderer {
                Renderer::Semantic => "semantic",
                Renderer::Native => "native",
            };
            push_value(&mut arguments, "--renderer", renderer);
        }

        return arguments;
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum BundleProfile {
    /// Screen presentation evidence only.
    Screen,
    /// State history and diagnostic summaries.
    Debug,
    /// Complete byte-exact output streams and all artifacts.
    Forensic,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BundleInput {
    /// Completed session name or path.
    session: String,
    /// New destination directory.
    out: String,
    /// Content policy; defaults to `screen`.
    profile: Option<BundleProfile>,
    /// Include a PNG of the last presented screen.
    #[serde(default)]
    screenshot: bool,
    /// Write one compressed archive instead of a directory.
    #[serde(default)]
    archive: bool,
}

impl BundleInput {
    pub(super) fn arguments(self) -> Vec<String> {
        let mut arguments = vec![
            "bundle".to_owned(),
            self.session,
            "--json".to_owned(),
            "--out".to_owned(),
            self.out,
        ];

        if let Some(profile) = self.profile {
            let profile = match profile {
                BundleProfile::Screen => "screen",
                BundleProfile::Debug => "debug",
                BundleProfile::Forensic => "forensic",
            };
            push_value(&mut arguments, "--profile", profile);
        }

        if self.screenshot {
            arguments.push("--screenshot".to_owned());
        }

        if self.archive {
            arguments.push("--archive".to_owned());
        }

        return arguments;
    }
}

fn push_value(arguments: &mut Vec<String>, flag: &str, value: &str) {
    arguments.push(format!("{flag}={value}"));
}

fn push_optional(arguments: &mut Vec<String>, flag: &str, value: Option<impl ToString>) {
    if let Some(value) = value {
        push_value(arguments, flag, &value.to_string());
    }
}

fn push_millis(arguments: &mut Vec<String>, flag: &str, value: Option<u64>) {
    if let Some(ms) = value {
        push_value(arguments, flag, &format!("{ms}ms"));
    }
}

// Values stay out of recorder arguments, so they travel in an owner-only specification file.
fn environment_spec(variables: BTreeMap<String, String>) -> Result<Option<NamedTempFile>, String> {
    if variables.is_empty() {
        return Ok(None);
    }

    let spec = EnvironmentSpec {
        set: variables
            .into_iter()
            .map(|(name, value)| return (name, ByteString::from(value.into_bytes())))
            .collect(),
        ..EnvironmentSpec::default()
    };

    let mut file = NamedTempFile::new().map_err(|error| return error.to_string())?;
    serde_json::to_writer(&mut file, &spec).map_err(|error| return error.to_string())?;
    file.flush().map_err(|error| return error.to_string())?;

    return Ok(Some(file));
}
