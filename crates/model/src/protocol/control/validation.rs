//! Request-shape rules every control client and the recording actor apply identically.

use super::{
    ActionPostcondition, ControlCommand, ReactionRequirement, ScreenMode, ScreenPredicate,
    StateHashComparison,
};
use crate::terminal::{
    screen::{screen_match_text, ScreenRegion},
    state::SemanticState,
};
use regex::{Regex, RegexBuilder};
use std::{collections::BTreeMap, error::Error as StdError, fmt, ops::Range, time::Duration};

/// Longest server-side wait or postcondition timeout a control command may request.
pub const MAX_CONTROL_WAIT: Duration = Duration::from_secs(3_600);
const MAX_MARKER_LABEL_BYTES: usize = 256;
const MAX_PREDICATE_TEXT_BYTES: usize = 16 * 1024;
const MAX_PREDICATE_PATTERN_BYTES: usize = 1024;
const MAX_PREDICATE_DEPTH: usize = 4;
const MAX_PREDICATE_NODES: usize = 32;
const MAX_PREDICATE_CHILDREN: usize = 16;
const PREDICATE_REGEX_SIZE_LIMIT: usize = 1024 * 1024;
const REACTION_RULE: &str =
    "reaction requires a lowercase BLAKE3 starting state and 0 < timeout <= 1h";

fn valid_reaction(reaction: &ReactionRequirement) -> bool {
    return within_wait(reaction.timeout_ns)
        && reaction
            .changed_from
            .as_deref()
            .is_none_or(is_lowercase_blake3_hex);
}

/// Whether a value is a 64-character lowercase hexadecimal BLAKE3 digest.
pub fn is_lowercase_blake3_hex(hash: &str) -> bool {
    return hash.len() == 64
        && hash
            .as_bytes()
            .iter()
            .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
}

/// A screen predicate that violates its size or shape limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredicateError {
    /// A text-count predicate has a non-text operand or a zero minimum.
    TextCount,
    /// Composition is deeper than 4 levels or has more than 32 nodes.
    TooLarge,
    /// Text operand is empty or longer than 16 KiB.
    TextLength,
    /// Regular expression is empty or longer than 1 KiB.
    PatternLength,
    /// Regular expression does not compile within its size limit.
    InvalidPattern {
        /// Compiler diagnostic.
        reason: String,
    },
    /// Region is empty or inverted.
    EmptyRegion,
    /// Cursor predicate constrains no property.
    UnconstrainedCursor,
    /// State hash is not a lowercase BLAKE3 digest.
    InvalidStateHash,
    /// `all` or `any` has other than 1 through 16 children.
    ChildCount,
}

impl fmt::Display for PredicateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return match self {
            Self::TextCount => write!(
                f,
                "text count requires a contains or matches-regex predicate and a positive minimum"
            ),
            Self::TooLarge => write!(f, "screen predicate composition is too deep or large"),
            Self::TextLength => write!(f, "screen predicate text must contain 1-16384 bytes"),
            Self::PatternLength => {
                write!(f, "screen regular expression must contain 1-1024 bytes")
            }
            Self::InvalidPattern { reason } => write!(f, "invalid bounded screen regex: {reason}"),
            Self::EmptyRegion => write!(f, "screen region must be non-empty and half-open"),
            Self::UnconstrainedCursor => {
                write!(f, "cursor predicate must constrain at least one property")
            }
            Self::InvalidStateHash => {
                write!(f, "state predicate requires a lowercase BLAKE3 hash")
            }
            Self::ChildCount => write!(f, "predicate composition must contain 1-16 children"),
        };
    }
}

impl StdError for PredicateError {}

/// A control command that violates a request-shape rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandError {
    /// Screen-wait predicate is invalid.
    ScreenWaitPredicate(PredicateError),
    /// Screen-wait durations do not satisfy `stable <= timeout <= 1h` with a nonzero timeout.
    ScreenWaitDurations,
    /// Stable-capture durations do not satisfy `0 < stable <= timeout <= 1h`.
    StableCaptureDurations,
    /// Stable-capture reaction has an invalid starting state or a timeout outside `0 < timeout <= 1h`.
    StableCaptureReaction,
    /// Action screen postcondition predicate is invalid.
    PostconditionPredicate(PredicateError),
    /// Action screen postcondition timeout does not satisfy `0 < timeout <= 1h`.
    PostconditionTimeout,
    /// Action idle postcondition durations do not satisfy `0 < stable <= timeout <= 1h`.
    PostconditionIdleDurations,
    /// Action idle reaction has an invalid starting state or a timeout outside `0 < timeout <= 1h`.
    PostconditionReaction,
    /// Marker label is empty after trimming or longer than 256 bytes.
    MarkerLabel,
    /// Application event violates its own limits.
    ApplicationEvent {
        /// Stable description of the violated limit.
        reason: &'static str,
    },
}

impl CommandError {
    /// Stable rejection code reported on the control wire.
    pub const fn code(&self) -> &'static str {
        return match self {
            Self::ScreenWaitPredicate(_) | Self::ScreenWaitDurations => "invalid_screen_wait",
            Self::StableCaptureDurations | Self::StableCaptureReaction => "invalid_stable_capture",
            Self::PostconditionPredicate(_)
            | Self::PostconditionTimeout
            | Self::PostconditionIdleDurations
            | Self::PostconditionReaction => "invalid_action_postcondition",
            Self::MarkerLabel => "invalid_marker",
            Self::ApplicationEvent { .. } => "invalid_application_event",
        };
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return match self {
            Self::ScreenWaitPredicate(error) | Self::PostconditionPredicate(error) => {
                write!(f, "{error}")
            }
            Self::ScreenWaitDurations => {
                write!(f, "durations do not satisfy stable <= timeout <= 1h")
            }
            Self::StableCaptureDurations => write!(
                f,
                "durations must satisfy stable <= timeout <= 1h and both must be nonzero"
            ),
            Self::StableCaptureReaction => write!(f, "stable capture {REACTION_RULE}"),
            Self::PostconditionTimeout => write!(
                f,
                "action postcondition timeout must satisfy 0 < timeout <= 1h"
            ),
            Self::PostconditionIdleDurations => write!(
                f,
                "idle postcondition durations must satisfy 0 < stable <= timeout <= 1h"
            ),
            Self::PostconditionReaction => write!(f, "idle postcondition {REACTION_RULE}"),
            Self::MarkerLabel => write!(f, "marker must contain 1 to 256 UTF-8 bytes"),
            Self::ApplicationEvent { reason } => write!(f, "{reason}"),
        };
    }
}

impl StdError for CommandError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        return match self {
            Self::ScreenWaitPredicate(error) | Self::PostconditionPredicate(error) => Some(error),
            _ => None,
        };
    }
}

/// A screen predicate within its limits, with its regular expressions compiled once.
#[derive(Clone, Debug)]
pub struct CompiledScreenPredicate {
    predicate: ScreenPredicate,
    regexes: BTreeMap<String, Regex>,
}

impl CompiledScreenPredicate {
    /// Byte ranges of literal or regex matches; `None` for non-text predicates.
    pub fn text_matches(&self, text: &str) -> Option<Vec<Range<usize>>> {
        return text_matches(&self.predicate, &self.regexes, text);
    }

    /// Minimum count explicitly requested for a text-count predicate.
    pub fn minimum_text_matches(&self) -> Option<usize> {
        return match self.predicate {
            ScreenPredicate::TextCount { minimum, .. } => Some(minimum),
            _ => None,
        };
    }

    /// The predicate as requested.
    pub const fn predicate(&self) -> &ScreenPredicate {
        return &self.predicate;
    }

    /// Release the predicate as requested.
    pub fn into_predicate(self) -> ScreenPredicate {
        return self.predicate;
    }

    /// Whether evaluation reads the viewport text, so callers can skip rendering it.
    pub fn needs_viewport_text(&self) -> bool {
        return needs_viewport_text(&self.predicate);
    }

    /// Evaluate against one semantic state, its viewport text, and its presented-state hash.
    pub fn matches(&self, state: &SemanticState, viewport_text: &str, state_hash: &str) -> bool {
        return evaluate(
            &self.predicate,
            &self.regexes,
            state,
            viewport_text,
            state_hash,
        );
    }
}

impl ScreenPredicate {
    /// Check the predicate's limits and compile its regular expressions.
    ///
    /// # Errors
    ///
    /// Returns the first violated limit: composition deeper than 4 levels or larger than 32
    /// nodes, other than 1-16 children, empty or oversized text or patterns, an invalid regular
    /// expression, an empty or inverted region, an unconstrained cursor predicate, or a state hash
    /// that is not lowercase BLAKE3 hex.
    pub fn compile(&self) -> Result<CompiledScreenPredicate, PredicateError> {
        let mut nodes = 0;
        let mut regexes = BTreeMap::new();
        check_predicate(self, 1, &mut nodes, &mut regexes)?;

        return Ok(CompiledScreenPredicate {
            predicate: self.clone(),
            regexes,
        });
    }
}

/// A control command that satisfies every request-shape rule.
#[derive(Debug)]
pub struct ValidatedCommand<'command> {
    command: &'command ControlCommand,
    predicate: Option<CompiledScreenPredicate>,
}

impl<'command> ValidatedCommand<'command> {
    /// Compiled screen predicate, when this command evaluates one.
    pub const fn predicate(&self) -> Option<&CompiledScreenPredicate> {
        return self.predicate.as_ref();
    }

    /// The validated command.
    pub const fn command(&self) -> &'command ControlCommand {
        return self.command;
    }

    /// Compiled predicate of a screen wait or a screen action postcondition.
    pub fn into_predicate(self) -> Option<CompiledScreenPredicate> {
        return self.predicate;
    }
}

impl ControlCommand {
    /// Check every request-shape rule that does not depend on live session state.
    ///
    /// # Errors
    ///
    /// Returns the violated rule; [`CommandError::code`] gives its wire rejection code.
    pub fn validate(&self) -> Result<ValidatedCommand<'_>, CommandError> {
        let predicate = match self {
            Self::MatchBaseline { predicate } => Some(
                predicate
                    .compile()
                    .map_err(CommandError::PostconditionPredicate)?,
            ),
            Self::WaitScreen {
                predicate,
                stable_for_ns,
                timeout_ns,
                ..
            } => {
                let predicate = predicate
                    .compile()
                    .map_err(CommandError::ScreenWaitPredicate)?;

                if !within_wait(*timeout_ns) || *stable_for_ns > *timeout_ns {
                    return Err(CommandError::ScreenWaitDurations);
                }

                Some(predicate)
            }
            Self::CaptureStable {
                stable_for_ns,
                timeout_ns,
                reaction,
                ..
            } => {
                if *stable_for_ns == 0 || !within_wait(*timeout_ns) || *stable_for_ns > *timeout_ns
                {
                    return Err(CommandError::StableCaptureDurations);
                }

                if reaction
                    .as_ref()
                    .is_some_and(|reaction| return !valid_reaction(reaction))
                {
                    return Err(CommandError::StableCaptureReaction);
                }

                None
            }
            Self::Act { postcondition, .. } => match postcondition {
                ActionPostcondition::None => None,
                ActionPostcondition::Idle {
                    stable_for_ns,
                    timeout_ns,
                    reaction,
                } => {
                    if *stable_for_ns == 0
                        || !within_wait(*timeout_ns)
                        || *stable_for_ns > *timeout_ns
                    {
                        return Err(CommandError::PostconditionIdleDurations);
                    }

                    if !valid_reaction(reaction) {
                        return Err(CommandError::PostconditionReaction);
                    }

                    None
                }
                ActionPostcondition::Screen {
                    predicate,
                    timeout_ns,
                    stable_for_ns,
                } => {
                    if !within_wait(*timeout_ns) || *stable_for_ns > *timeout_ns {
                        return Err(CommandError::PostconditionTimeout);
                    }

                    Some(
                        predicate
                            .compile()
                            .map_err(CommandError::PostconditionPredicate)?,
                    )
                }
            },
            Self::Mark { label } => {
                let label = label.trim();
                if label.is_empty() || label.len() > MAX_MARKER_LABEL_BYTES {
                    return Err(CommandError::MarkerLabel);
                }

                None
            }
            Self::Application { data } => {
                data.validate()
                    .map_err(|reason| return CommandError::ApplicationEvent { reason })?;

                None
            }
            Self::Ping
            | Self::Status
            | Self::EventCursor { .. }
            | Self::Snapshot
            | Self::ScreenFrame { .. }
            | Self::Input { .. }
            | Self::InputSequence { .. }
            | Self::Key { .. }
            | Self::Paste { .. }
            | Self::Focus { .. }
            | Self::Pointer { .. }
            | Self::Activate { .. }
            | Self::Resize { .. }
            | Self::Signal { .. }
            | Self::Stop => None,
        };

        return Ok(ValidatedCommand {
            command: self,
            predicate,
        });
    }
}

fn within_wait(timeout_ns: u64) -> bool {
    return timeout_ns > 0 && Duration::from_nanos(timeout_ns) <= MAX_CONTROL_WAIT;
}

fn check_predicate(
    predicate: &ScreenPredicate,
    depth: usize,
    nodes: &mut usize,
    regexes: &mut BTreeMap<String, Regex>,
) -> Result<(), PredicateError> {
    *nodes = nodes.saturating_add(1);

    if depth > MAX_PREDICATE_DEPTH || *nodes > MAX_PREDICATE_NODES {
        return Err(PredicateError::TooLarge);
    }

    match predicate {
        ScreenPredicate::TextCount {
            matching: predicate,
            minimum,
        } => {
            if *minimum == 0
                || !matches!(
                    predicate.as_ref(),
                    ScreenPredicate::Contains { .. } | ScreenPredicate::MatchesRegex { .. }
                )
            {
                return Err(PredicateError::TextCount);
            }

            check_predicate(predicate, depth + 1, nodes, regexes)?;
        }
        ScreenPredicate::Contains { text }
        | ScreenPredicate::Absent { text }
        | ScreenPredicate::RowContains { text, .. }
        | ScreenPredicate::RowAbsent { text, .. } => check_text(text)?,
        ScreenPredicate::MatchesRegex { pattern } | ScreenPredicate::ExcludesRegex { pattern } => {
            if pattern.is_empty() || pattern.len() > MAX_PREDICATE_PATTERN_BYTES {
                return Err(PredicateError::PatternLength);
            }

            if !regexes.contains_key(pattern) {
                let regex = RegexBuilder::new(pattern)
                    .size_limit(PREDICATE_REGEX_SIZE_LIMIT)
                    .build()
                    .map_err(|error| {
                        return PredicateError::InvalidPattern {
                            reason: error.to_string(),
                        };
                    })?;
                regexes.insert(pattern.clone(), regex);
            }
        }
        ScreenPredicate::RegionContains { region, text }
        | ScreenPredicate::RegionAbsent { region, text } => {
            check_text(text)?;

            if region.left >= region.right || region.top >= region.bottom {
                return Err(PredicateError::EmptyRegion);
            }
        }
        ScreenPredicate::Cursor {
            column,
            row,
            visible,
            shape,
        } => {
            if column.is_none() && row.is_none() && visible.is_none() && shape.is_none() {
                return Err(PredicateError::UnconstrainedCursor);
            }
        }
        ScreenPredicate::StateHash { hash, .. } => {
            if !is_lowercase_blake3_hex(hash) {
                return Err(PredicateError::InvalidStateHash);
            }
        }
        ScreenPredicate::All { predicates } | ScreenPredicate::Any { predicates } => {
            if predicates.is_empty() || predicates.len() > MAX_PREDICATE_CHILDREN {
                return Err(PredicateError::ChildCount);
            }

            for child in predicates {
                check_predicate(child, depth + 1, nodes, regexes)?;
            }
        }
        ScreenPredicate::Mode { .. } => {}
    }

    return Ok(());
}

fn check_text(text: &str) -> Result<(), PredicateError> {
    if text.is_empty() || text.len() > MAX_PREDICATE_TEXT_BYTES {
        return Err(PredicateError::TextLength);
    }
    return Ok(());
}

fn text_matches(
    predicate: &ScreenPredicate,
    regexes: &BTreeMap<String, Regex>,
    text: &str,
) -> Option<Vec<Range<usize>>> {
    return match predicate {
        ScreenPredicate::TextCount {
            matching: predicate,
            ..
        } => text_matches(predicate, regexes, text),
        ScreenPredicate::Contains { text: needle } => Some(
            text.match_indices(needle)
                .map(|(start, matched)| start..start + matched.len())
                .collect(),
        ),
        ScreenPredicate::MatchesRegex { pattern } => regexes.get(pattern).map(|regex| {
            return regex
                .find_iter(text)
                .map(|matched| return matched.range())
                .collect();
        }),
        _ => None,
    };
}

fn needs_viewport_text(predicate: &ScreenPredicate) -> bool {
    return match predicate {
        ScreenPredicate::TextCount { .. } => true,
        ScreenPredicate::Contains { .. }
        | ScreenPredicate::Absent { .. }
        | ScreenPredicate::MatchesRegex { .. }
        | ScreenPredicate::ExcludesRegex { .. } => true,
        ScreenPredicate::All { predicates } | ScreenPredicate::Any { predicates } => {
            predicates.iter().any(needs_viewport_text)
        }
        ScreenPredicate::RowContains { .. }
        | ScreenPredicate::RowAbsent { .. }
        | ScreenPredicate::RegionContains { .. }
        | ScreenPredicate::RegionAbsent { .. }
        | ScreenPredicate::Cursor { .. }
        | ScreenPredicate::Mode { .. }
        | ScreenPredicate::StateHash { .. } => false,
    };
}

fn evaluate(
    predicate: &ScreenPredicate,
    regexes: &BTreeMap<String, Regex>,
    state: &SemanticState,
    viewport_text: &str,
    state_hash: &str,
) -> bool {
    return match predicate {
        ScreenPredicate::TextCount {
            matching: predicate,
            minimum,
        } => text_matches(predicate, regexes, &screen_match_text(state).0)
            .is_some_and(|matches| return matches.len() >= *minimum),
        ScreenPredicate::Contains { text } => viewport_text.contains(text),
        ScreenPredicate::Absent { text } => !viewport_text.contains(text),
        ScreenPredicate::MatchesRegex { pattern } => regexes
            .get(pattern)
            .is_some_and(|regex| return regex.is_match(viewport_text)),
        ScreenPredicate::ExcludesRegex { pattern } => regexes
            .get(pattern)
            .is_some_and(|regex| return !regex.is_match(viewport_text)),
        ScreenPredicate::RowContains { row, text } => {
            screen_row_text(state, *row).is_some_and(|value| return value.contains(text))
        }
        ScreenPredicate::RowAbsent { row, text } => {
            screen_row_text(state, *row).is_some_and(|value| return !value.contains(text))
        }
        ScreenPredicate::RegionContains { region, text } => {
            screen_region_text(state, *region).is_some_and(|value| return value.contains(text))
        }
        ScreenPredicate::RegionAbsent { region, text } => {
            screen_region_text(state, *region).is_some_and(|value| return !value.contains(text))
        }
        ScreenPredicate::Cursor {
            column,
            row,
            visible,
            shape,
        } => {
            column.is_none_or(|value| return state.cursor.column == Some(value))
                && row.is_none_or(|value| return state.cursor.row == Some(value))
                && visible.is_none_or(|value| return state.cursor.visible == value)
                && shape.is_none_or(|value| return state.cursor.style == value)
        }
        ScreenPredicate::Mode { mode, enabled } => screen_mode(state, *mode) == *enabled,
        ScreenPredicate::StateHash { hash, comparison } => match comparison {
            StateHashComparison::Unchanged => state_hash == hash,
            StateHashComparison::Changed => state_hash != hash,
        },
        ScreenPredicate::All { predicates } => predicates.iter().all(|child| {
            return evaluate(child, regexes, state, viewport_text, state_hash);
        }),
        ScreenPredicate::Any { predicates } => predicates.iter().any(|child| {
            return evaluate(child, regexes, state, viewport_text, state_hash);
        }),
    };
}

fn screen_row_text(state: &SemanticState, row: u16) -> Option<String> {
    let row = state.grid.get(usize::from(row))?;
    let mut text = String::new();

    for cell in row.cells.iter() {
        text.push_str(if cell.grapheme.is_empty() { " " } else { &cell.grapheme });
    }

    return Some(text.trim_end().to_owned());
}

fn screen_region_text(state: &SemanticState, region: ScreenRegion) -> Option<String> {
    if region.right > state.cols || region.bottom > state.rows {
        return None;
    }

    let mut text = String::new();
    for row_index in region.top..region.bottom {
        let row = state.grid.get(usize::from(row_index))?;
        for column in region.left..region.right {
            let cell = row.cells.get(usize::from(column))?;
            text.push_str(if cell.grapheme.is_empty() { " " } else { &cell.grapheme });
        }

        while text.ends_with(' ') {
            text.pop();
        }

        if row_index + 1 < region.bottom {
            text.push('\n');
        }
    }

    return Some(text);
}

fn screen_mode(state: &SemanticState, mode: ScreenMode) -> bool {
    return match mode {
        ScreenMode::AlternateScreen => state.modes.alternate_screen,
        ScreenMode::BracketedPaste => state.modes.bracketed_paste,
        ScreenMode::Wraparound => state.modes.wraparound,
        ScreenMode::SynchronizedOutput => state.modes.synchronized_output,
        ScreenMode::Ansi { number } => state
            .modes
            .values
            .iter()
            .find(|mode| return mode.ansi && mode.number == number)
            .is_some_and(|mode| return mode.enabled),
        ScreenMode::Dec { number } => state
            .modes
            .values
            .iter()
            .find(|mode| return !mode.ansi && mode.number == number)
            .is_some_and(|mode| return mode.enabled),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{protocol::control::InputSequenceAtom, terminal::screen::ScreenProjectionRequest};

    const HOUR_NS: u64 = 3_600_000_000_000;

    fn contains(text: &str) -> ScreenPredicate {
        return ScreenPredicate::Contains {
            text: text.to_owned(),
        };
    }

    fn wait(predicate: ScreenPredicate, stable_for_ns: u64, timeout_ns: u64) -> ControlCommand {
        return ControlCommand::WaitScreen {
            predicate,
            stable_for_ns,
            timeout_ns,
            projection: ScreenProjectionRequest::default(),
        };
    }

    fn reaction(timeout_ns: u64) -> ReactionRequirement {
        return ReactionRequirement {
            changed_from: None,
            timeout_ns,
        };
    }

    fn act(postcondition: ActionPostcondition) -> ControlCommand {
        return ControlCommand::Act {
            atoms: Vec::<InputSequenceAtom>::new(),
            pace_ns: 0,
            include_atom_outcomes: false,
            postcondition,
            projection: ScreenProjectionRequest::default(),
        };
    }

    fn rejection(command: &ControlCommand) -> CommandError {
        return command
            .validate()
            .expect_err("command breaks a request-shape rule");
    }

    #[test]
    fn screen_wait_accepts_zero_stable_interval_and_the_full_hour() {
        let predicate = wait(contains("ready"), 0, HOUR_NS)
            .validate()
            .expect("hour-long wait is allowed")
            .into_predicate()
            .expect("screen wait carries its compiled predicate");
        assert_eq!(predicate.predicate(), &contains("ready"));
    }

    #[test]
    fn screen_wait_rejects_durations_outside_stable_le_timeout_le_hour() {
        for (stable_for_ns, timeout_ns) in [(0, 0), (0, HOUR_NS + 1), (2, 1)] {
            let error = rejection(&wait(contains("ready"), stable_for_ns, timeout_ns));
            assert_eq!(error, CommandError::ScreenWaitDurations);
            assert_eq!(error.code(), "invalid_screen_wait");
        }
    }

    #[test]
    fn screen_wait_reports_an_invalid_predicate_before_its_durations() {
        let error = rejection(&wait(contains(""), 0, 0));
        assert_eq!(
            error,
            CommandError::ScreenWaitPredicate(PredicateError::TextLength)
        );
    }

    #[test]
    fn stable_capture_requires_a_nonzero_stable_interval() {
        let capture = |stable_for_ns| {
            return ControlCommand::CaptureStable {
                stable_for_ns,
                timeout_ns: HOUR_NS,
                projection: ScreenProjectionRequest::default(),
                reaction: None,
            };
        };
        assert!(capture(1).validate().is_ok());
        assert_eq!(rejection(&capture(0)).code(), "invalid_stable_capture");
    }

    #[test]
    fn stable_capture_reaction_requires_a_blake3_state_and_a_bounded_timeout() {
        let capture = |changed_from: Option<String>, timeout_ns| {
            return ControlCommand::CaptureStable {
                stable_for_ns: 1,
                timeout_ns: HOUR_NS,
                projection: ScreenProjectionRequest::default(),
                reaction: Some(ReactionRequirement {
                    changed_from,
                    timeout_ns,
                }),
            };
        };
        assert!(capture(Some("a".repeat(64)), 1).validate().is_ok());
        assert!(capture(None, HOUR_NS).validate().is_ok());

        for (changed_from, timeout_ns) in [
            (Some(String::new()), 1),
            (Some("ABC".to_owned()), 1),
            (Some("A".repeat(64)), 1),
            (None, 0),
            (None, HOUR_NS + 1),
        ] {
            assert_eq!(
                rejection(&capture(changed_from, timeout_ns)),
                CommandError::StableCaptureReaction
            );
        }
    }

    #[test]
    fn action_postconditions_follow_the_same_wait_limit() {
        let rejected = [
            ActionPostcondition::Idle {
                stable_for_ns: 0,
                timeout_ns: 1,
                reaction: reaction(1),
            },
            ActionPostcondition::Idle {
                stable_for_ns: 1,
                timeout_ns: HOUR_NS + 1,
                reaction: reaction(1),
            },
            ActionPostcondition::Idle {
                stable_for_ns: 1,
                timeout_ns: 1,
                reaction: ReactionRequirement {
                    changed_from: Some("not-a-hash".to_owned()),
                    timeout_ns: 1,
                },
            },
            ActionPostcondition::Idle {
                stable_for_ns: 1,
                timeout_ns: 1,
                reaction: reaction(0),
            },
            ActionPostcondition::Screen {
                predicate: contains("done"),
                timeout_ns: 0,
                stable_for_ns: 0,
            },
            ActionPostcondition::Screen {
                predicate: ScreenPredicate::All { predicates: vec![] },
                timeout_ns: 1,
                stable_for_ns: 0,
            },
        ];

        for postcondition in rejected {
            assert_eq!(
                rejection(&act(postcondition)).code(),
                "invalid_action_postcondition"
            );
        }

        let screen = act(ActionPostcondition::Screen {
            predicate: contains("done"),
            timeout_ns: HOUR_NS,
            stable_for_ns: 0,
        });
        assert!(screen
            .validate()
            .expect("screen postcondition within limits")
            .into_predicate()
            .is_some());
    }

    #[test]
    fn predicate_composition_is_bounded_in_depth_and_size() {
        let mut deep = contains("x");
        for _ in 0..4 {
            deep = ScreenPredicate::All {
                predicates: vec![deep],
            };
        }

        assert_eq!(deep.compile().map(|_| ()), Err(PredicateError::TooLarge));

        let wide = ScreenPredicate::Any {
            predicates: (0..16)
                .map(|_| {
                    return ScreenPredicate::All {
                        predicates: vec![contains("a"), contains("b")],
                    };
                })
                .collect(),
        };

        assert_eq!(wide.compile().map(|_| ()), Err(PredicateError::TooLarge));

        let siblings = ScreenPredicate::Any {
            predicates: (0..17).map(|_| return contains("a")).collect(),
        };

        assert_eq!(
            siblings.compile().map(|_| ()),
            Err(PredicateError::ChildCount)
        );
    }

    #[test]
    fn predicate_operands_are_checked() {
        let invalid = [
            (
                ScreenPredicate::MatchesRegex {
                    pattern: "[".to_owned(),
                },
                "invalid bounded screen regex",
            ),
            (
                ScreenPredicate::ExcludesRegex {
                    pattern: "a".repeat(1025),
                },
                "1-1024 bytes",
            ),
            (contains(&"a".repeat(16 * 1024 + 1)), "1-16384 bytes"),
            (
                ScreenPredicate::StateHash {
                    hash: "A".repeat(64),
                    comparison: StateHashComparison::Changed,
                },
                "lowercase BLAKE3",
            ),
            (
                ScreenPredicate::Cursor {
                    column: None,
                    row: None,
                    visible: None,
                    shape: None,
                },
                "at least one property",
            ),
            (
                ScreenPredicate::RegionContains {
                    region: ScreenRegion {
                        left: 2,
                        top: 0,
                        right: 2,
                        bottom: 1,
                    },
                    text: "a".to_owned(),
                },
                "non-empty",
            ),
        ];

        for (predicate, message) in invalid {
            let error = predicate.compile().expect_err("operand is out of bounds");
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    #[test]
    fn marker_label_is_measured_after_trimming() {
        let mark = |label: String| return ControlCommand::Mark { label };
        assert!(mark(format!("  {}  ", "m".repeat(256))).validate().is_ok());
        assert_eq!(
            rejection(&mark(" \t ".to_owned())),
            CommandError::MarkerLabel
        );
        assert_eq!(rejection(&mark("m".repeat(257))).code(), "invalid_marker");
    }
}
