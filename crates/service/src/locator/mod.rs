//! Rich-frame queries and lazy session-bound locators.

mod duration;
mod lazy;
mod query;

pub use duration::{wait_duration_from_millis, wait_duration_from_secs};
pub use lazy::{
    Locator, LocatorCondition, LocatorError, LocatorFailure, LocatorWaitFailure,
    LocatorWaitOptions, LocatorWaitResult, WaitCancellation, CAPTURE_QUANTUM,
};
pub use query::{
    LocatorSpec, MatchRange, MatchedCell, QueryDescription, QueryError, QueryResult,
    ScreenFrameQuery, ScreenMatch, StyleFilter, TextMatchKind, TextMatcher,
};
