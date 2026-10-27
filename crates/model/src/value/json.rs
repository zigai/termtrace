//! Serde support for fields holding arbitrary JSON.
//!
//! Text formats see the value unchanged. Native host converters recognize the marker name and
//! convert numbers inside it to their language's JSON number type, so an integer that a model
//! field would carry as a host big integer stays a plain number here.

use super::JSON_MARKER;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Serialize one JSON-valued field.
///
/// # Errors
///
/// Propagates serializer failures.
pub fn serialize<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    return serializer.serialize_newtype_struct(JSON_MARKER, value);
}

/// Deserialize one JSON-valued field.
///
/// # Errors
///
/// Propagates deserializer failures.
pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    return T::deserialize(deserializer);
}
