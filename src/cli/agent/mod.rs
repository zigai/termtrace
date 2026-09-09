//! Versioned newline-framed process adapter for language-neutral clients.

mod framing;
mod native;
mod operations;
mod runtime;
#[cfg(test)]
mod tests;

pub(in crate::cli) use runtime::serve;
