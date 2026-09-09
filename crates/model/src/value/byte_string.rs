use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{borrow::Cow, fmt, result::Result as StdResult};

/// Raw bytes serialized as standard padded Base64.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ByteString(Vec<u8>);

impl ByteString {
    /// Wrap an owned byte buffer.
    pub fn new(bytes: Vec<u8>) -> Self {
        return Self(bytes);
    }

    /// Borrow the original bytes.
    pub fn as_bytes(&self) -> &[u8] {
        return &self.0;
    }

    /// Consume this value and return the original bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        return self.0;
    }
}

impl fmt::Debug for ByteString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return f
            .debug_struct("ByteString")
            .field("len", &self.0.len())
            .finish_non_exhaustive();
    }
}

impl From<Vec<u8>> for ByteString {
    fn from(bytes: Vec<u8>) -> Self {
        return Self::new(bytes);
    }
}

impl From<&[u8]> for ByteString {
    fn from(bytes: &[u8]) -> Self {
        return Self::new(bytes.to_vec());
    }
}

impl Serialize for ByteString {
    fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: Serializer,
    {
        return serializer.serialize_str(&STANDARD.encode(&self.0));
    }
}

impl<'de> Deserialize<'de> for ByteString {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        return STANDARD
            .decode(encoded)
            .map(Self)
            .map_err(de::Error::custom);
    }
}

impl schemars::JsonSchema for ByteString {
    fn schema_name() -> Cow<'static, str> {
        return Cow::Borrowed("Base64Bytes");
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        return schemars::json_schema!({
            "type": "string",
            "contentEncoding": "base64",
            "pattern": "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$"
        });
    }
}
