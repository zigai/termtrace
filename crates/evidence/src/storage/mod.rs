pub(crate) mod artifact;
pub(crate) mod event_log;
pub(crate) mod session;

mod error;
pub(crate) mod identifier_index;

pub(crate) use error::{CompletionIntegrityError, CompletionIntegrityKind};
pub use error::{Error, Result};

pub(crate) fn is_lowercase_blake3(value: &str) -> bool {
    return value.len() == 64
        && value
            .bytes()
            .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
}
