//! Shared scalar and compact value contracts.

mod byte_string;
pub mod exact_path;
pub mod geometry;
pub mod json;
pub mod pagination;

pub use byte_string::ByteString;
pub use exact_path::ExactPath;

/// Serde newtype name native host converters read as raw bytes carried in Base64 text.
pub const BYTES_MARKER: &str = "$termtrace::bytes";
/// Serde newtype name native host converters read as an exact path.
pub const PATH_MARKER: &str = "$termtrace::path";
/// Serde newtype name native host converters read as arbitrary JSON.
pub const JSON_MARKER: &str = "$termtrace::json";
/// Serde newtype name native host converters read as an unsigned integer carried in decimal
/// text.
pub const DECIMAL_MARKER: &str = "$termtrace::decimal";
