//! Readable UTF-8 paths with an explicit Base64 representation for other path bytes.

use super::ByteString;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::str;

/// One exact filesystem path in text output.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PathValue {
    /// A path that is valid UTF-8.
    Text(String),
    /// A path whose bytes are not valid UTF-8.
    Base64 {
        /// Exact path bytes encoded as Base64.
        base64: ByteString,
    },
}

impl PathValue {
    /// Represent exact bytes as a readable path.
    pub fn from_bytes(bytes: &ByteString) -> Self {
        return str::from_utf8(bytes.as_bytes()).map_or_else(
            |_| {
                return Self::Base64 {
                    base64: bytes.clone(),
                };
            },
            |text| return Self::Text(text.to_owned()),
        );
    }

    /// Recover the exact filesystem path bytes.
    pub fn into_bytes(self) -> ByteString {
        return match self {
            Self::Text(text) => ByteString::new(text.into_bytes()),
            Self::Base64 { base64 } => base64,
        };
    }
}

/// Serialize an exact path as UTF-8 or explicitly tagged Base64.
///
/// # Errors
/// Returns the serializer's error if it cannot write the path value.
pub fn serialize<S: Serializer>(bytes: &ByteString, serializer: S) -> Result<S::Ok, S::Error> {
    return PathValue::from_bytes(bytes).serialize(serializer);
}

/// Deserialize an exact path from UTF-8 or explicitly tagged Base64.
///
/// # Errors
/// Rejects values other than path strings or objects carrying valid Base64 bytes.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ByteString, D::Error> {
    return PathValue::deserialize(deserializer).map(PathValue::into_bytes);
}

/// Optional exact paths.
pub mod option {
    use super::{ByteString, Deserialize, Deserializer, PathValue, Serialize, Serializer};

    /// Serialize an optional exact path.
    ///
    /// # Errors
    /// Returns the serializer's error if it cannot write the path or null.
    pub fn serialize<S: Serializer>(
        bytes: &Option<ByteString>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        return bytes
            .as_ref()
            .map(PathValue::from_bytes)
            .serialize(serializer);
    }

    /// Deserialize an optional exact path.
    ///
    /// # Errors
    /// Rejects non-null values other than path strings or objects carrying valid Base64 bytes.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<ByteString>, D::Error> {
        return Option::<PathValue>::deserialize(deserializer)
            .map(|path| return path.map(PathValue::into_bytes));
    }
}
