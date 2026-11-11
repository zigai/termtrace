//! Strict deserialization of Python plain data into model values.

use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
    types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple},
};
use serde::de::{self, IntoDeserializer as _};
use std::fmt;
use termtrace_model::execution::application::MAX_SDK_JSON_DEPTH;

pub(crate) struct InputError(pub(crate) PyErr);

impl fmt::Debug for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return fmt::Debug::fmt(&self.0, f);
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return fmt::Display::fmt(&self.0, f);
    }
}

impl std::error::Error for InputError {}

impl de::Error for InputError {
    fn custom<M: fmt::Display>(msg: M) -> Self {
        return Self(PyValueError::new_err(msg.to_string()));
    }
}

impl From<PyErr> for InputError {
    fn from(error: PyErr) -> Self {
        return Self(error);
    }
}

type Result<T> = std::result::Result<T, InputError>;

pub(crate) struct Input<'a, 'py> {
    object: &'a Bound<'py, PyAny>,
    depth: usize,
}

impl<'a, 'py> Input<'a, 'py> {
    pub(crate) const fn new(object: &'a Bound<'py, PyAny>) -> Self {
        return Self { object, depth: 0 };
    }

    fn nested<'b>(&self, object: &'b Bound<'py, PyAny>) -> Result<Input<'b, 'py>> {
        // Cyclic or unboundedly nested input would otherwise overflow the native stack.
        if self.depth >= MAX_SDK_JSON_DEPTH {
            return Err(InputError(PyValueError::new_err(format!(
                "value nesting exceeds {MAX_SDK_JSON_DEPTH} levels"
            ))));
        }
        return Ok(Input {
            object,
            depth: self.depth + 1,
        });
    }

    fn integer<'de, V: de::Visitor<'de>>(&self, visitor: V) -> Result<V::Value> {
        if let Ok(value) = self.object.extract::<i64>() {
            return visitor.visit_i64(value);
        }

        if let Ok(value) = self.object.extract::<u64>() {
            return visitor.visit_u64(value);
        }

        if let Ok(value) = self.object.extract::<i128>() {
            return visitor.visit_i128(value);
        }
        return visitor.visit_u128(self.object.extract::<u128>()?);
    }

    fn sequence<'de, V: de::Visitor<'de>>(&self, visitor: V) -> Result<V::Value> {
        let items = self.object.try_iter()?.collect::<PyResult<Vec<_>>>()?;

        return visitor.visit_seq(Sequence {
            input: self,
            items: items.into_iter(),
        });
    }

    fn mapping<'de, V: de::Visitor<'de>>(
        &self,
        dict: &Bound<'py, PyDict>,
        visitor: V,
    ) -> Result<V::Value> {
        let entries = dict.iter().collect::<Vec<_>>();

        return visitor.visit_map(Mapping {
            input: self,
            entries: entries.into_iter(),
            value: None,
        });
    }
}

impl<'de> de::Deserializer<'de> for Input<'_, '_> {
    type Error = InputError;

    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let object = self.object;
        if object.is_none() {
            return visitor.visit_unit();
        }

        if let Ok(value) = object.cast::<PyBool>() {
            return visitor.visit_bool(value.is_true());
        }

        if object.is_instance_of::<PyInt>() {
            return self.integer(visitor);
        }

        if let Ok(value) = object.cast::<PyFloat>() {
            return visitor.visit_f64(value.value());
        }

        if let Ok(text) = object.cast::<PyString>() {
            if let Ok(text) = text.to_str() {
                return visitor.visit_str(text);
            }

            // Surrogate escapes stand for filesystem bytes, so exact paths survive.
            let bytes = object
                .py()
                .import("os")?
                .call_method1("fsencode", (text,))?;

            return visitor.visit_bytes(bytes.cast::<PyBytes>().map_err(PyErr::from)?.as_bytes());
        }

        if let Ok(bytes) = object.cast::<PyBytes>() {
            return visitor.visit_bytes(bytes.as_bytes());
        }

        if object.is_instance_of::<PyList>() || object.is_instance_of::<PyTuple>() {
            return self.sequence(visitor);
        }

        if let Ok(dict) = object.cast::<PyDict>() {
            return self.mapping(dict, visitor);
        }

        let os = object.py().import("os")?;

        if object.is_instance(&os.getattr("PathLike")?)? {
            let path = os.call_method1("fspath", (object,))?;

            return self.nested(&path)?.deserialize_any(visitor);
        }
        return Err(InputError(PyTypeError::new_err(format!(
            "unsupported value of type {}",
            object.get_type().name()?
        ))));
    }

    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if self.object.is_none() {
            return visitor.visit_none();
        }
        return visitor.visit_some(self);
    }

    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        return visitor.visit_newtype_struct(self);
    }

    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        if let Ok(text) = self.object.cast::<PyString>() {
            return visitor.visit_enum(text.to_str()?.to_owned().into_deserializer());
        }

        let dict = self.object.cast::<PyDict>().map_err(PyErr::from)?;
        let mut entries = dict.iter();
        let (Some((variant, value)), None) = (entries.next(), entries.next()) else {
            return Err(de::Error::custom(
                "an enum mapping must have exactly one entry",
            ));
        };

        return visitor.visit_enum(Variant {
            input: &self,
            variant,
            value,
        });
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

struct Sequence<'a, 'b, 'py> {
    input: &'a Input<'b, 'py>,
    items: std::vec::IntoIter<Bound<'py, PyAny>>,
}

impl<'de> de::SeqAccess<'de> for Sequence<'_, '_, '_> {
    type Error = InputError;

    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>> {
        let Some(item) = self.items.next() else {
            return Ok(None);
        };

        return seed.deserialize(self.input.nested(&item)?).map(Some);
    }

    fn size_hint(&self) -> Option<usize> {
        return Some(self.items.len());
    }
}

struct Mapping<'a, 'b, 'py> {
    input: &'a Input<'b, 'py>,
    entries: std::vec::IntoIter<(Bound<'py, PyAny>, Bound<'py, PyAny>)>,
    value: Option<Bound<'py, PyAny>>,
}

impl<'de> de::MapAccess<'de> for Mapping<'_, '_, '_> {
    type Error = InputError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.value = Some(value);

        return seed.deserialize(self.input.nested(&key)?).map(Some);
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let value = self.value.take().ok_or_else(|| {
            return <InputError as de::Error>::custom("map value requested before its key");
        })?;

        return seed.deserialize(self.input.nested(&value)?);
    }

    fn size_hint(&self) -> Option<usize> {
        return Some(self.entries.len());
    }
}

struct Variant<'a, 'b, 'py> {
    input: &'a Input<'b, 'py>,
    variant: Bound<'py, PyAny>,
    value: Bound<'py, PyAny>,
}

impl<'py, 'de> de::EnumAccess<'de> for Variant<'_, '_, 'py> {
    type Error = InputError;
    type Variant = Payload<'py>;

    fn variant_seed<T: de::DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<(T::Value, Self::Variant)> {
        let variant = seed.deserialize(self.input.nested(&self.variant)?)?;
        let depth = self.input.depth + 1;

        return Ok((
            variant,
            Payload {
                value: self.value,
                depth,
            },
        ));
    }
}

struct Payload<'py> {
    value: Bound<'py, PyAny>,
    depth: usize,
}

impl Payload<'_> {
    const fn input(&self) -> Input<'_, '_> {
        return Input {
            object: &self.value,
            depth: self.depth,
        };
    }
}

impl<'de> de::VariantAccess<'de> for Payload<'_> {
    type Error = InputError;

    fn unit_variant(self) -> Result<()> {
        return de::Deserialize::deserialize(self.input());
    }

    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        return seed.deserialize(self.input());
    }

    fn tuple_variant<V: de::Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        return de::Deserializer::deserialize_seq(self.input(), visitor);
    }

    fn struct_variant<V: de::Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        return de::Deserializer::deserialize_map(self.input(), visitor);
    }
}
