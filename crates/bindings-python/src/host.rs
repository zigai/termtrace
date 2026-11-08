//! Conversion between Rust model values and Python objects, driven only by serde.
//!
//! Every integer becomes an exact Python `int`, byte strings become `bytes`, and exact paths
//! become `str` using the filesystem encoding's `surrogateescape` handler, so `os.fsencode`
//! returns the original bytes. Each model type converts through the same code; nothing
//! here knows field names.

use crate::input::Input;
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString},
};
use pyo3_stub_gen::{PyStubType, TypeInfo};
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, ser, Serialize};
use std::{ffi::OsString, fmt, os::unix::ffi::OsStringExt as _, path::PathBuf};
use termtrace_model::value::{ByteString, BYTES_MARKER, DECIMAL_MARKER, PATH_MARKER};
use termtrace_sdk::contracts::types::input_name;

/// Python module that defines every generated model type.
pub const TYPES_MODULE: &str = "termtrace.types";

/// A model value returned to Python as plain data of its generated type.
pub struct Model<T>(pub T);

impl<'py, T: Serialize> IntoPyObject<'py> for Model<T> {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        return to_python(py, &self.0);
    }
}

impl<T: JsonSchema> PyStubType for Model<T> {
    fn type_output() -> TypeInfo {
        return model_type::<T>();
    }
}

/// A model value accepted from Python plain data of its generated type.
pub struct Arg<T>(pub T);

impl<'a, 'py, T: DeserializeOwned> FromPyObject<'a, 'py> for Arg<T> {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        return from_python(&obj).map(Arg);
    }
}

impl<T: JsonSchema> PyStubType for Arg<T> {
    fn type_output() -> TypeInfo {
        return TypeInfo::locally_defined(&input_name::<T>(), TYPES_MODULE.into());
    }
}

/// A filesystem path argument: `str`, `bytes`, or `os.PathLike`, kept byte-exact.
pub struct PathArg(pub PathBuf);

impl<'a, 'py> FromPyObject<'a, 'py> for PathArg {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        if let Ok(bytes) = obj.cast::<PyBytes>() {
            return Ok(Self(PathBuf::from(OsString::from_vec(
                bytes.as_bytes().to_vec(),
            ))));
        }
        return obj.extract::<PathBuf>().map(Self);
    }
}

impl std::ops::Deref for PathArg {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        return &self.0;
    }
}

impl PyStubType for PathArg {
    fn type_output() -> TypeInfo {
        let mut info = TypeInfo::unqualified("str | bytes | os.PathLike[str] | os.PathLike[bytes]");
        info.import.insert("os".into());

        return info;
    }
}

fn model_type<T: JsonSchema>() -> TypeInfo {
    return TypeInfo::locally_defined(&T::schema_name(), TYPES_MODULE.into());
}

/// Convert any serializable model value into Python data.
///
/// # Errors
///
/// Fails when the value's `Serialize` implementation fails or a marker carries malformed data.
pub fn to_python<'py, T: Serialize + ?Sized>(
    py: Python<'py>,
    value: &T,
) -> PyResult<Bound<'py, PyAny>> {
    return value.serialize(Host { py }).map_err(|error| return error.0);
}

/// Convert Python data into a model value.
///
/// Each value is read as exactly its own Python type: `bool` fields reject integers and
/// strings. `os.PathLike` objects and strings that carry surrogate escapes are read as
/// their filesystem paths, so exact paths survive the conversion.
///
/// # Errors
///
/// Raises `TypeError` or `ValueError` describing the first value that does not match `T`.
pub fn from_python<T: DeserializeOwned>(object: &Bound<'_, PyAny>) -> PyResult<T> {
    return T::deserialize(Input::new(object)).map_err(|error| return error.0);
}

/// Serialization failure carrying the Python exception to raise.
pub struct HostError(PyErr);

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
        return Self(PyValueError::new_err(msg.to_string()));
    }
}

impl From<PyErr> for HostError {
    fn from(error: PyErr) -> Self {
        return Self(error);
    }
}

type Result<T, E = HostError> = std::result::Result<T, E>;

#[derive(Clone, Copy)]
struct Host<'py> {
    py: Python<'py>,
}

impl<'py> Host<'py> {
    fn variant(self, variant: &str, value: Bound<'py, PyAny>) -> Result<Bound<'py, PyAny>> {
        let dict = PyDict::new(self.py);
        dict.set_item(variant, value)?;

        return Ok(dict.into_any());
    }

    fn bytes(self, encoded: &Bound<'py, PyAny>) -> Result<Bound<'py, PyAny>> {
        let text = encoded.cast::<PyString>().map_err(PyErr::from)?;
        let bytes: ByteString =
            serde::Deserialize::deserialize(serde::de::value::StrDeserializer::<
                serde::de::value::Error,
            >::new(text.to_str()?))
            .map_err(|error| return HostError(PyValueError::new_err(error.to_string())))?;

        return Ok(PyBytes::new(self.py, bytes.as_bytes()).into_any());
    }

    fn decimal(self, text: &Bound<'py, PyAny>) -> Result<Bound<'py, PyAny>> {
        let text = text.cast::<PyString>().map_err(PyErr::from)?;
        let value = text
            .to_str()
            .map_err(HostError)?
            .parse::<u128>()
            .map_err(|error| return HostError(PyValueError::new_err(error.to_string())))?;

        let Ok(int) = value.into_pyobject(self.py);

        return Ok(int.into_any());
    }

    fn path(self, repr: Bound<'py, PyAny>) -> Result<Bound<'py, PyAny>> {
        if repr.is_instance_of::<PyString>() {
            return Ok(repr);
        }

        let encoded = repr
            .cast::<PyDict>()
            .map_err(PyErr::from)?
            .get_item("base64")?
            .ok_or_else(|| return PyValueError::new_err("encoded path has no Base64 bytes"))?;

        return Ok(self.py.import("os")?.call_method1("fsdecode", (encoded,))?);
    }
}

impl<'py> ser::Serializer for Host<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = HostError;
    type SerializeSeq = Sequence<'py>;
    type SerializeTuple = Sequence<'py>;
    type SerializeTupleStruct = Sequence<'py>;
    type SerializeTupleVariant = VariantSequence<'py>;
    type SerializeMap = Mapping<'py>;
    type SerializeStruct = Mapping<'py>;
    type SerializeStructVariant = VariantMapping<'py>;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok> {
        return Ok(PyBool::new(self.py, v).to_owned().into_any());
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok> {
        return self.serialize_i64(i64::from(v));
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok> {
        return self.serialize_i64(i64::from(v));
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok> {
        return self.serialize_i64(i64::from(v));
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok> {
        return Ok(PyInt::new(self.py, v).into_any());
    }

    fn serialize_i128(self, v: i128) -> Result<Self::Ok> {
        let Ok(int) = v.into_pyobject(self.py);

        return Ok(int.into_any());
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok> {
        return self.serialize_u64(u64::from(v));
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok> {
        return self.serialize_u64(u64::from(v));
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok> {
        return self.serialize_u64(u64::from(v));
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok> {
        return Ok(PyInt::new(self.py, v).into_any());
    }

    fn serialize_u128(self, v: u128) -> Result<Self::Ok> {
        let Ok(int) = v.into_pyobject(self.py);

        return Ok(int.into_any());
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok> {
        return self.serialize_f64(f64::from(v));
    }

    fn serialize_f64(self, v: f64) -> Result<Self::Ok> {
        return Ok(PyFloat::new(self.py, v).into_any());
    }

    fn serialize_char(self, v: char) -> Result<Self::Ok> {
        return self.serialize_str(v.encode_utf8(&mut [0; 4]));
    }

    fn serialize_str(self, v: &str) -> Result<Self::Ok> {
        return Ok(PyString::new(self.py, v).into_any());
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok> {
        return Ok(PyBytes::new(self.py, v).into_any());
    }

    fn serialize_none(self) -> Result<Self::Ok> {
        return Ok(self.py.None().into_bound(self.py));
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok> {
        return value.serialize(self);
    }

    fn serialize_unit(self) -> Result<Self::Ok> {
        return self.serialize_none();
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok> {
        return self.serialize_none();
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok> {
        return self.serialize_str(variant);
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok> {
        let inner = value.serialize(self)?;

        return match name {
            BYTES_MARKER => self.bytes(&inner),
            PATH_MARKER => self.path(inner),
            DECIMAL_MARKER => self.decimal(&inner),
            _ => Ok(inner),
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

        return self.variant(variant, inner);
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
            dict: PyDict::new(self.py),
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

struct Sequence<'py> {
    host: Host<'py>,
    items: Vec<Bound<'py, PyAny>>,
}

impl<'py> Sequence<'py> {
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(self.host)?);

        return Ok(());
    }

    fn list(self) -> Result<Bound<'py, PyAny>> {
        return Ok(PyList::new(self.host.py, self.items)?.into_any());
    }
}

impl<'py> ser::SerializeSeq for Sequence<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = HostError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        return self.push(value);
    }

    fn end(self) -> Result<Self::Ok> {
        return self.list();
    }
}

impl<'py> ser::SerializeTuple for Sequence<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = HostError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        return self.push(value);
    }

    fn end(self) -> Result<Self::Ok> {
        return self.list();
    }
}

impl<'py> ser::SerializeTupleStruct for Sequence<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = HostError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        return self.push(value);
    }

    fn end(self) -> Result<Self::Ok> {
        return self.list();
    }
}

struct VariantSequence<'py> {
    variant: &'static str,
    sequence: Sequence<'py>,
}

impl<'py> ser::SerializeTupleVariant for VariantSequence<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = HostError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        return self.sequence.push(value);
    }

    fn end(self) -> Result<Self::Ok> {
        let host = self.sequence.host;
        let list = self.sequence.list()?;

        return host.variant(self.variant, list);
    }
}

struct Mapping<'py> {
    host: Host<'py>,
    dict: Bound<'py, PyDict>,
    key: Option<Bound<'py, PyAny>>,
}

impl Mapping<'_> {
    fn field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.dict.set_item(key, value.serialize(self.host)?)?;

        return Ok(());
    }
}

impl<'py> ser::SerializeMap for Mapping<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = HostError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        self.key = Some(key.serialize(self.host)?);

        return Ok(());
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let key = self
            .key
            .take()
            .ok_or_else(|| return <HostError as ser::Error>::custom("map value without a key"))?;

        self.dict.set_item(key, value.serialize(self.host)?)?;

        return Ok(());
    }

    fn end(self) -> Result<Self::Ok> {
        return Ok(self.dict.into_any());
    }
}

impl<'py> ser::SerializeStruct for Mapping<'py> {
    type Ok = Bound<'py, PyAny>;
    type Error = HostError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        return self.field(key, value);
    }

    fn end(self) -> Result<Self::Ok> {
        return Ok(self.dict.into_any());
    }
}

struct VariantMapping<'py> {
    variant: &'static str,
    mapping: Mapping<'py>,
}

impl<'py> ser::SerializeStructVariant for VariantMapping<'py> {
    type Ok = Bound<'py, PyAny>;
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
        let dict = self.mapping.dict.into_any();

        return host.variant(self.variant, dict);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt as _, path::PathBuf};

    #[derive(Serialize, serde::Deserialize, Debug, PartialEq)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Effect {
        Applied { seq: u64 },
        Rejected { reason: String },
    }

    #[derive(Serialize, serde::Deserialize, Debug, PartialEq)]
    struct Report {
        widest: u64,
        widest_wide: u128,
        payload: ByteString,
        #[serde(with = "termtrace_model::value::exact_path")]
        path: PathBuf,
        effect: Effect,
        missing: Option<u32>,
    }

    fn report() -> Report {
        return Report {
            widest: u64::MAX,
            widest_wide: u128::MAX,
            payload: ByteString::from(vec![0, 255, 10]),
            path: PathBuf::from(OsStr::from_bytes(b"/tmp/session-\xff")),
            effect: Effect::Applied { seq: (1 << 53) + 1 },
            missing: None,
        };
    }

    #[test]
    fn model_values_become_exact_python_data_and_round_trip() {
        Python::initialize();
        Python::attach(|py| {
            let value = to_python(py, &report()).expect("convert to Python");
            let check = |expression: &str| {
                let locals = PyDict::new(py);
                locals.set_item("value", &value).expect("bind value");
                let result: bool = py
                    .eval(
                        &std::ffi::CString::new(expression).expect("expression"),
                        None,
                        Some(&locals),
                    )
                    .and_then(|result| return result.extract())
                    .expect("evaluate");
                assert!(result, "{expression}");
            };
            check("value['widest'] == 2**64 - 1");
            check("value['widest_wide'] == 2**128 - 1");
            check("value['payload'] == b'\\x00\\xff\\n'");
            check("__import__('os').fsencode(value['path']) == b'/tmp/session-\\xff'");
            check("value['effect'] == {'kind': 'applied', 'seq': 2**53 + 1}");
            check("value['missing'] is None");
            let back: Report = from_python(&value).expect("convert back");
            assert_eq!(back, report());
        });
    }

    #[test]
    fn path_like_inputs_keep_exact_bytes() {
        Python::initialize();
        Python::attach(|py| {
            let value = to_python(py, &report()).expect("convert to Python");
            let dict = value.cast::<PyDict>().expect("dict");
            let pathlib = py.import("pathlib").expect("pathlib");
            let path = pathlib
                .getattr("Path")
                .and_then(|path| {
                    return path.call1((dict.get_item("path").expect("get").expect("path"),));
                })
                .expect("Path");

            dict.set_item("path", path).expect("replace path");
            let back: Report = from_python(&value).expect("convert back");
            assert_eq!(back.path.as_os_str().as_bytes(), b"/tmp/session-\xff");
        });
    }
}
