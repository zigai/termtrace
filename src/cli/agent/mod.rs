//! Versioned newline-framed process adapter for language-neutral clients.

mod framing;
mod operations;
mod protocol;
mod runtime;

#[cfg(test)]
mod tests;

pub(in crate::cli) use runtime::serve;
