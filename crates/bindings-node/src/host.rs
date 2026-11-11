//! Conversion between Rust model values and JavaScript values, driven only by serde.
//!
//! 64- and 128-bit integers always become `bigint`, smaller integers and floats become
//! `number`, byte strings become `Buffer`, and exact paths become a `string`, or a `Buffer`
//! when the path is not UTF-8. Each model type converts through the same code; nothing here
//! knows field names.

use napi::bindgen_prelude::{
    BigInt, Buffer, FromNapiValue, Null, Object, ToNapiValue, TypeName, ValidateNapiValue,
};
use napi::{sys, Env, ValueType};
use serde::{de::DeserializeOwned, ser, Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use termtrace_model::value::{
    path_bytes::PathValue, ByteString, BYTES_MARKER, DECIMAL_MARKER, JSON_MARKER, PATH_MARKER,
};

/// A model value returned to JavaScript as plain data of its generated type.
///
/// Generated declarations name the inner Rust type; `Model<T>` is declared as `T`.
pub struct Model<T>(pub T);

impl<T: Serialize> ToNapiValue for Model<T> {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> napi::Result<sys::napi_value> {
        let value = val
            .0
            .serialize(Host { json: false })
            .map_err(|error| error.0)?;

        // SAFETY: napi supplies the live environment for this conversion.
        return unsafe { NativeValue::to_napi_value(env, value) };
    }
}

impl<T> TypeName for Model<T> {
    fn type_name() -> &'static str {
        return "Model";
    }

    fn value_type() -> ValueType {
        return ValueType::Unknown;
    }
}

unsafe fn reject_cycles(
    env: sys::napi_env,
    value: sys::napi_value,
    parents: &mut Vec<sys::napi_value>,
) -> napi::Result<()> {
    // SAFETY: all handles belong to the live call, and every traversal follows a type check.
    unsafe {
        let mut kind = 0;
        napi::check_status!(sys::napi_typeof(env, value, &mut kind))?;
        if ValueType::from(kind) != ValueType::Object {
            return Ok(());
        }

        let mut binary = false;
        napi::check_status!(sys::napi_is_typedarray(env, value, &mut binary))?;
        if binary {
            return Ok(());
        }

        napi::check_status!(sys::napi_is_buffer(env, value, &mut binary))?;
        if binary {
            return Ok(());
        }

        napi::check_status!(sys::napi_is_arraybuffer(env, value, &mut binary))?;
        if binary {
            return Ok(());
        }

        for parent in parents.iter() {
            let mut same = false;
            napi::check_status!(sys::napi_strict_equals(env, *parent, value, &mut same))?;
            if same {
                return Err(type_error("cyclic model argument".to_owned()));
            }
        }

        parents.push(value);

        let object = Object::from_raw(env, value);
        for key in Object::keys(&object)? {
            if let Some(child) = object.get::<napi::Unknown>(&key)? {
                reject_cycles(env, napi::JsValue::raw(&child), parents)?;
            }
        }

        parents.pop();
        return Ok(());
    }
}

/// A model value accepted from JavaScript plain data of its generated type.
///
/// Generated declarations name the inner Rust type; `Arg<T>` is declared as `T`.
pub struct Arg<T>(pub T);

impl<T: DeserializeOwned> FromNapiValue for Arg<T> {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> napi::Result<Self> {
        // SAFETY: napi supplies a live env and value handle for this conversion.
        let value = unsafe {
            reject_cycles(env, napi_val, &mut Vec::new())?;

            napi::bindgen_prelude::Unknown::from_napi_value(env, napi_val)?
        };

        return Env::from(env)
            .from_js_value(value)
            .map(Arg)
            .map_err(|error| {
                return type_error(error.reason.clone());
            });
    }
}

impl<T> TypeName for Arg<T> {
    fn type_name() -> &'static str {
        return "Arg";
    }

    fn value_type() -> ValueType {
        return ValueType::Unknown;
    }
}

impl<T> ValidateNapiValue for Arg<T> {
    unsafe fn validate(_: sys::napi_env, _: sys::napi_value) -> napi::Result<sys::napi_value> {
        return Ok(std::ptr::null_mut());
    }
}

/// Serialization failure carrying the JavaScript error to throw.
pub(crate) struct HostError(napi::Error);

impl fmt::Debug for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return fmt::Debug::fmt(&self.0, f);
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return fmt::Display::fmt(&self.0, f);
    }
}

impl std::error::Error for HostError {}

impl ser::Error for HostError {
    fn custom<M: fmt::Display>(msg: M) -> Self {
        return Self(napi::Error::from_reason(msg.to_string()));
    }
}

impl From<napi::Error> for HostError {
    fn from(error: napi::Error) -> Self {
        return Self(error);
    }
}

type Result<T, E = HostError> = std::result::Result<T, E>;

/// A JavaScript value built from a model, before it is handed to the engine.
#[derive(Serialize, Deserialize)]
pub enum NativeValue {
    /// `null`.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A string.
    String(String),
    /// A `BigInt` given by its sign and little-endian 64-bit words.
    Integer {
        /// Whether the value is negative.
        negative: bool,
        /// Magnitude words, least significant first.
        words: Vec<u64>,
    },
    /// A `Uint8Array`.
    Bytes(Vec<u8>),
    /// An array.
    Array(Vec<NativeValue>),
    /// A plain object.
    Object(BTreeMap<String, NativeValue>),
}

impl ToNapiValue for NativeValue {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> napi::Result<sys::napi_value> {
        // SAFETY: napi supplies a live environment, shared by every child conversion.
        unsafe {
            return match val {
                Self::Null => Null::to_napi_value(env, Null),
                Self::Bool(v) => bool::to_napi_value(env, v),
                Self::Number(v) => f64::to_napi_value(env, v),
                Self::String(v) => String::to_napi_value(env, v),
                Self::Integer { negative, words } => BigInt::to_napi_value(
                    env,
                    BigInt {
                        sign_bit: negative,
                        words,
                    },
                ),
                Self::Bytes(v) => Buffer::to_napi_value(env, Buffer::from(v)),
                Self::Array(v) => Vec::<Self>::to_napi_value(env, v),
                Self::Object(fields) => {
                    let mut object = Object::new(&Env::from(env))?;
                    for (key, val) in fields {
                        object.set(key, val)?;
                    }

                    Object::to_napi_value(env, object)
                }
            };
        }
    }
}

impl From<bool> for NativeValue {
    fn from(value: bool) -> Self {
        return Self::Bool(value);
    }
}

impl From<f64> for NativeValue {
    fn from(value: f64) -> Self {
        return Self::Number(value);
    }
}

impl From<String> for NativeValue {
    fn from(value: String) -> Self {
        return Self::String(value);
    }
}

impl From<&str> for NativeValue {
    fn from(value: &str) -> Self {
        return Self::String(value.to_owned());
    }
}

impl From<Null> for NativeValue {
    fn from(_: Null) -> Self {
        return Self::Null;
    }
}

impl From<Vec<NativeValue>> for NativeValue {
    fn from(value: Vec<NativeValue>) -> Self {
        return Self::Array(value);
    }
}

impl From<BTreeMap<String, NativeValue>> for NativeValue {
    fn from(value: BTreeMap<String, NativeValue>) -> Self {
        return Self::Object(value);
    }
}

pub(crate) fn encode<T: Serialize + ?Sized>(value: &T) -> napi::Result<String> {
    let value = value.serialize(Host { json: false }).map_err(|e| e.0)?;
    return serde_json::to_string(&value).map_err(|e| napi::Error::from_reason(e.to_string()));
}

// Largest integer a JavaScript number holds exactly.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

#[derive(Clone, Copy)]
struct Host {
    // Inside arbitrary JSON every integer is a JavaScript number, not a model big integer.
    json: bool,
}

impl Host {
    fn leaf<T: Into<NativeValue>>(self, value: T) -> NativeValue {
        return value.into();
    }

    fn object(self) -> BTreeMap<String, NativeValue> {
        return BTreeMap::new();
    }

    fn variant(self, variant: &str, value: NativeValue) -> NativeValue {
        let mut object = self.object();
        object.insert(variant.to_owned(), value);

        return self.leaf(object);
    }

    fn integer(self, negative: bool, magnitude: u128) -> Result<NativeValue> {
        if !self.json {
            let words = vec![
                u64::try_from(magnitude & u128::from(u64::MAX)).unwrap_or(0),
                u64::try_from(magnitude >> 64).unwrap_or(0),
            ];

            return Ok(NativeValue::Integer { negative, words });
        }

        let Some(exact) = u64::try_from(magnitude)
            .ok()
            .filter(|value| return *value <= MAX_SAFE_INTEGER)
        else {
            return Err(<HostError as ser::Error>::custom(
                "JSON integer is outside the JavaScript safe-integer range",
            ));
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "the value is at most 2^53 - 1, which f64 holds exactly"
        )]
        let number = exact as f64;

        return Ok(self.leaf(if negative { -number } else { number }));
    }

    fn decimal<T: Serialize + ?Sized>(self, text: &T) -> Result<NativeValue> {
        let serde_json::Value::String(text) =
            serde_json::to_value(text).map_err(<HostError as ser::Error>::custom)?
        else {
            return Err(<HostError as ser::Error>::custom(
                "decimal marker must carry text",
            ));
        };
        let value = text
            .parse::<u128>()
            .map_err(<HostError as ser::Error>::custom)?;

        return self.integer(false, value);
    }

    fn bytes<T: Serialize + ?Sized>(self, encoded: &T) -> Result<NativeValue> {
        let text = serde_json::to_value(encoded).map_err(<HostError as ser::Error>::custom)?;
        let bytes: ByteString =
            serde_json::from_value(text).map_err(<HostError as ser::Error>::custom)?;

        return Ok(NativeValue::Bytes(bytes.into_bytes()));
    }

    fn path<T: Serialize + ?Sized>(self, repr: &T) -> Result<NativeValue> {
        return match serde_json::to_value(repr).map_err(<HostError as ser::Error>::custom)? {
            serde_json::Value::String(text) => Ok(self.leaf(text)),
            bytes => {
                let bytes: PathValue =
                    serde_json::from_value(bytes).map_err(<HostError as ser::Error>::custom)?;

                Ok(NativeValue::Bytes(bytes.into_bytes().into_bytes()))
            }
        };
    }
}

impl ser::Serializer for Host {
    type Ok = NativeValue;
    type Error = HostError;
    type SerializeSeq = Sequence;
    type SerializeTuple = Sequence;
    type SerializeTupleStruct = Sequence;
    type SerializeTupleVariant = VariantSequence;
    type SerializeMap = Mapping;
    type SerializeStruct = Mapping;
    type SerializeStructVariant = VariantMapping;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok> {
        return Ok(self.leaf(v));
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok> {
        return Ok(self.leaf(f64::from(v)));
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok> {
        return Ok(self.leaf(f64::from(v)));
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok> {
        return Ok(self.leaf(f64::from(v)));
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok> {
        return self.integer(v < 0, u128::from(v.unsigned_abs()));
    }

    fn serialize_i128(self, v: i128) -> Result<Self::Ok> {
        return self.integer(v < 0, v.unsigned_abs());
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok> {
        return Ok(self.leaf(f64::from(v)));
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok> {
        return Ok(self.leaf(f64::from(v)));
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok> {
        return Ok(self.leaf(f64::from(v)));
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok> {
        return self.integer(false, u128::from(v));
    }

    fn serialize_u128(self, v: u128) -> Result<Self::Ok> {
        return self.integer(false, v);
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok> {
        return Ok(self.leaf(f64::from(v)));
    }

    fn serialize_f64(self, v: f64) -> Result<Self::Ok> {
        return Ok(self.leaf(v));
    }

    fn serialize_char(self, v: char) -> Result<Self::Ok> {
        return Ok(self.leaf(v.to_string()));
    }

    fn serialize_str(self, v: &str) -> Result<Self::Ok> {
        return Ok(self.leaf(v));
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok> {
        return Ok(NativeValue::Bytes(v.to_vec()));
    }

    fn serialize_none(self) -> Result<Self::Ok> {
        return Ok(self.leaf(Null));
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok> {
        return value.serialize(self);
    }

    fn serialize_unit(self) -> Result<Self::Ok> {
        return Ok(self.leaf(Null));
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok> {
        return Ok(self.leaf(Null));
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok> {
        return Ok(self.leaf(variant));
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok> {
        return match name {
            BYTES_MARKER => self.bytes(value),
            PATH_MARKER => self.path(value),
            DECIMAL_MARKER => self.decimal(value),
            JSON_MARKER => value.serialize(Self { json: true }),
            _ => value.serialize(self),
        };
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok> {
        let inner = value.serialize(self)?;

        return Ok(self.variant(variant, inner));
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq> {
        return Ok(Sequence {
            host: self,
            items: Vec::with_capacity(len.unwrap_or(0)),
        });
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple> {
        return self.serialize_seq(Some(len));
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        return self.serialize_seq(Some(len));
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        return Ok(VariantSequence {
            variant,
            sequence: self.serialize_seq(Some(len))?,
        });
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap> {
        return Ok(Mapping {
            host: self,
            object: self.object(),
            key: None,
        });
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Self::SerializeStruct> {
        return self.serialize_map(Some(len));
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        return Ok(VariantMapping {
            variant,
            mapping: self.serialize_map(Some(len))?,
        });
    }
}

struct Sequence {
    host: Host,
    items: Vec<NativeValue>,
}

impl Sequence {
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(self.host)?);

        return Ok(());
    }

    fn array(self) -> NativeValue {
        return self.host.leaf(self.items);
    }
}

impl ser::SerializeSeq for Sequence {
    type Ok = NativeValue;
    type Error = HostError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        return self.push(value);
    }

    fn end(self) -> Result<Self::Ok> {
        return Ok(self.array());
    }
}

impl ser::SerializeTuple for Sequence {
    type Ok = NativeValue;
    type Error = HostError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        return self.push(value);
    }

    fn end(self) -> Result<Self::Ok> {
        return Ok(self.array());
    }
}

impl ser::SerializeTupleStruct for Sequence {
    type Ok = NativeValue;
    type Error = HostError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        return self.push(value);
    }

    fn end(self) -> Result<Self::Ok> {
        return Ok(self.array());
    }
}

struct VariantSequence {
    variant: &'static str,
    sequence: Sequence,
}

impl ser::SerializeTupleVariant for VariantSequence {
    type Ok = NativeValue;
    type Error = HostError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        return self.sequence.push(value);
    }

    fn end(self) -> Result<Self::Ok> {
        let host = self.sequence.host;
        let array = self.sequence.array();

        return Ok(host.variant(self.variant, array));
    }
}

struct Mapping {
    host: Host,
    object: BTreeMap<String, NativeValue>,
    key: Option<String>,
}

impl Mapping {
    fn field<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) -> Result<()> {
        let value = value.serialize(self.host)?;
        self.object.insert(key.to_owned(), value);
        return Ok(());
    }
}

impl ser::SerializeMap for Mapping {
    type Ok = NativeValue;
    type Error = HostError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        let key = match serde_json::to_value(key).map_err(<HostError as ser::Error>::custom)? {
            serde_json::Value::String(text) => text,
            other => other.to_string(),
        };
        self.key = Some(key);
        return Ok(());
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let key = self
            .key
            .take()
            .ok_or_else(|| return <HostError as ser::Error>::custom("map value without a key"))?;

        return self.field(&key, value);
    }

    fn end(self) -> Result<Self::Ok> {
        return Ok(self.host.leaf(self.object));
    }
}

impl ser::SerializeStruct for Mapping {
    type Ok = NativeValue;
    type Error = HostError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        return self.field(key, value);
    }

    fn end(self) -> Result<Self::Ok> {
        return Ok(self.host.leaf(self.object));
    }
}

struct VariantMapping {
    variant: &'static str,
    mapping: Mapping,
}

impl ser::SerializeStructVariant for VariantMapping {
    type Ok = NativeValue;
    type Error = HostError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        return self.mapping.field(key, value);
    }

    fn end(self) -> Result<Self::Ok> {
        let host = self.mapping.host;
        let object = host.leaf(self.mapping.object);

        return Ok(host.variant(self.variant, object));
    }
}

fn argument_error(class: &str, message: String) -> napi::Error {
    let mut error = napi::Error::new(napi::Status::InvalidArg, message);
    error.cause = Some(Box::new(napi::Error::new(
        napi::Status::InvalidArg,
        format!("termtrace-argument:{class}"),
    )));
    return error;
}

/// An argument of the wrong JavaScript type; `index.js` throws it as a `TypeError`.
pub(crate) fn type_error(message: String) -> napi::Error {
    return argument_error("TypeError", message);
}

/// An argument outside its valid range; `index.js` throws it as a `RangeError`.
pub(crate) fn range_error(message: String) -> napi::Error {
    return argument_error("RangeError", message);
}

/// Validate a whole-number argument that must fit a `u16` terminal coordinate.
///
/// # Errors
///
/// Fails when `value` is not a whole number from 0 through 65535.
pub(crate) fn cell_coordinate(value: f64, what: &str) -> napi::Result<u16> {
    if value.fract() != 0.0 || !(0.0..=65_535.0).contains(&value) {
        return Err(range_error(format!(
            "{what} must be a whole number from 0 through 65535, got {value}"
        )));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is a whole number checked to lie within u16"
    )]
    return Ok(value as u16);
}

/// Validate a whole-number non-negative index.
///
/// # Errors
///
/// Fails when `value` is negative, fractional, or not a safe integer.
pub(crate) fn index(value: f64, what: &str) -> napi::Result<usize> {
    if value.fract() != 0.0 || !(0.0..=9_007_199_254_740_991.0).contains(&value) {
        return Err(range_error(format!(
            "{what} must be a non-negative safe integer, got {value}"
        )));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is a non-negative safe integer"
    )]
    return Ok(value as usize);
}

/// Validate an optional whole-milliseconds duration.
///
/// # Errors
///
/// Fails when the value is negative, fractional, not finite, or above the wait bound.
pub(crate) fn millis(
    value: Option<f64>,
    default: std::time::Duration,
) -> napi::Result<std::time::Duration> {
    return value.map_or(Ok(default), |milliseconds| {
        return termtrace_sdk::locator::wait_duration_from_millis(milliseconds)
            .map_err(range_error);
    });
}
