//! Serde support for paths that keeps non-UTF-8 Linux path bytes exact.
//!
//! Text formats see a UTF-8 path as a plain string and other path bytes as tagged Base64.
//! Native host converters recognize the marker name and produce their language's exact
//! path value instead.

use super::{path_bytes::PathValue, ByteString, PATH_MARKER};
use serde::{de, Deserialize as _, Deserializer, Serialize, Serializer};
use std::{
    borrow::Cow,
    ffi::OsString,
    fmt,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    result::Result as StdResult,
};

struct Repr<'path>(&'path Path);

impl Serialize for Repr<'_> {
    fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: Serializer,
    {
        return PathValue::from_bytes(&ByteString::from(self.0.as_os_str().as_bytes()))
            .serialize(serializer);
    }
}

/// Serialize one exact path.
///
/// # Errors
///
/// Propagates serializer failures.
pub fn serialize<S>(path: &Path, serializer: S) -> StdResult<S::Ok, S::Error>
where
    S: Serializer,
{
    return serializer.serialize_newtype_struct(PATH_MARKER, &Repr(path));
}

struct PathVisitor;

impl<'de> de::Visitor<'de> for PathVisitor {
    type Value = PathBuf;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        return formatter.write_str("a path string, tagged Base64, or native bytes");
    }

    fn visit_str<E: de::Error>(self, v: &str) -> StdResult<PathBuf, E> {
        return Ok(PathBuf::from(v));
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> StdResult<PathBuf, E> {
        return Ok(PathBuf::from(OsString::from_vec(v.to_vec())));
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> StdResult<PathBuf, E> {
        return Ok(PathBuf::from(OsString::from_vec(v)));
    }

    fn visit_seq<A>(self, mut seq: A) -> StdResult<PathBuf, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(byte) = seq.next_element::<u8>()? {
            bytes.push(byte);
        }

        return Ok(PathBuf::from(OsString::from_vec(bytes)));
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> StdResult<PathBuf, A::Error> {
        let value = PathValue::deserialize(de::value::MapAccessDeserializer::new(map))?;

        return Ok(PathBuf::from(OsString::from_vec(
            value.into_bytes().into_bytes(),
        )));
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> StdResult<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        return deserializer.deserialize_any(self);
    }
}

/// Deserialize an exact path from text, tagged Base64, or native bytes.
///
/// # Errors
///
/// Rejects values that are neither path strings nor encoded path bytes.
pub fn deserialize<'de, D>(deserializer: D) -> StdResult<PathBuf, D::Error>
where
    D: Deserializer<'de>,
{
    return deserializer.deserialize_any(PathVisitor);
}

/// Serde support for optional exact paths.
pub mod option {
    use super::{PathVisitor, Repr, PATH_MARKER};
    use serde::{de, Deserializer, Serializer};
    use std::{fmt, path::PathBuf, result::Result as StdResult};

    /// Serialize an optional exact path.
    ///
    /// # Errors
    ///
    /// Propagates serializer failures.
    pub fn serialize<S>(path: &Option<PathBuf>, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: Serializer,
    {
        return match path {
            Some(path) => serializer.serialize_some(&Marked(path)),
            None => serializer.serialize_none(),
        };
    }

    struct Marked<'path>(&'path PathBuf);

    impl serde::Serialize for Marked<'_> {
        fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
        where
            S: Serializer,
        {
            return serializer.serialize_newtype_struct(PATH_MARKER, &Repr(self.0));
        }
    }

    struct OptionVisitor;

    impl<'de> de::Visitor<'de> for OptionVisitor {
        type Value = Option<PathBuf>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            return formatter.write_str("null, a path string, tagged Base64, or native bytes");
        }

        fn visit_none<E: de::Error>(self) -> StdResult<Option<PathBuf>, E> {
            return Ok(None);
        }

        fn visit_unit<E: de::Error>(self) -> StdResult<Option<PathBuf>, E> {
            return Ok(None);
        }

        fn visit_some<D>(self, deserializer: D) -> StdResult<Option<PathBuf>, D::Error>
        where
            D: Deserializer<'de>,
        {
            return deserializer.deserialize_any(PathVisitor).map(Some);
        }
    }

    /// Deserialize an optional exact path.
    ///
    /// # Errors
    ///
    /// Rejects values that are neither null, a string, nor bytes.
    pub fn deserialize<'de, D>(deserializer: D) -> StdResult<Option<PathBuf>, D::Error>
    where
        D: Deserializer<'de>,
    {
        return deserializer.deserialize_option(OptionVisitor);
    }
}

/// An exact path value, for results and arguments that are a path on their own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExactPath(pub PathBuf);

impl Serialize for ExactPath {
    fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: Serializer,
    {
        return serialize(&self.0, serializer);
    }
}

impl<'de> serde::Deserialize<'de> for ExactPath {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        return deserialize(deserializer).map(Self);
    }
}

impl schemars::JsonSchema for ExactPath {
    fn schema_name() -> Cow<'static, str> {
        return Cow::Borrowed("ExactPath");
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        return schemars::json_schema!({
            "x-termtrace-path": true,
            "allOf": [generator.subschema_for::<PathValue>()],
        });
    }
}
