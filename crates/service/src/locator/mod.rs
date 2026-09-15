//! Rich-frame queries and lazy session-bound locators.

mod lazy;
mod query;

pub use lazy::{Locator, LocatorError, LocatorWaitResult};
pub use query::{
    LocatorSpec, MatchRange, MatchedCell, QueryDescription, QueryError, QueryResult,
    ScreenFrameQuery, ScreenMatch, StyleFilter, TextMatchKind, TextMatcher,
};
