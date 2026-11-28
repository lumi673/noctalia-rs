//! Option-based JSON values for strict-first plugins.
//!
//! An object slot is `Option<T>`. `None` leaves the key out, and there is no
//! null variant to put in an array slot. `serde_json::Value::Null` belongs to
//! the helper path and never enters this model.

use std::error::Error as StdError;
use std::fmt;

use serde::de::{Error as _, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Number, Value};

/// A JSON object whose values are never null.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Object(Map<String, Value>);

impl Object {
    /// An empty object.
    #[must_use]
    pub fn new() -> Self {
        Self(Map::new())
    }

    /// Insert a value, or leave the key out when the option is `None`.
    pub fn insert<T>(
        &mut self,
        key: impl Into<String>,
        maybe: Option<T>,
    ) -> &mut Self
    where
        T: Into<Json>,
    {
        if let Some(present) = maybe {
            self.0.insert(key.into(), present.into().into_value());
        }
        self
    }

    /// Read a slot. A missing key and a stored null both read as `None`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|value| !value.is_null())
    }

    /// Remove a slot.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.0.remove(key)
    }

    /// Number of stored keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the object has no stored keys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Stored keys and values, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }

    /// Convert to a helper-compatible JSON value.
    #[must_use]
    pub fn into_value(self) -> Value {
        Value::Object(self.0)
    }
}

/// A JSON array with no null slots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Array(Vec<Value>);

impl Array {
    /// An empty array.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Append a value. There is no `Option<T>` overload: an absent slot would
    /// remove the array element in Luau.
    pub fn push<T>(&mut self, value: T) -> &mut Self
    where
        T: Into<Json>,
    {
        self.0.push(value.into().into_value());
        self
    }

    /// Read a slot.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Value> {
        self.0.get(index)
    }

    /// Number of stored values.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the array has no values.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Stored values in order.
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        self.0.iter()
    }

    /// Convert to a helper-compatible JSON value.
    #[must_use]
    pub fn into_value(self) -> Value {
        Value::Array(self.0)
    }
}

/// A JSON value with no null case.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Json {
    Bool(bool),
    Number(Number),
    String(String),
    Array(Array),
    Object(Object),
}

impl Json {
    /// Convert to a helper-compatible JSON value.
    #[must_use]
    pub fn into_value(self) -> Value {
        match self {
            Self::Bool(value) => Value::Bool(value),
            Self::Number(value) => Value::Number(value),
            Self::String(value) => Value::String(value),
            Self::Array(value) => value.into_value(),
            Self::Object(value) => value.into_value(),
        }
    }
}

macro_rules! from_json_scalar {
    ($($ty:ty => $variant:ident),+ $(,)?) => {
        $(
            impl From<$ty> for Json {
                fn from(value: $ty) -> Self {
                    Self::$variant(value.into())
                }
            }
        )+
    };
}

from_json_scalar!(bool => Bool, String => String, Number => Number, i8 => Number, i16 => Number, i32 => Number, i64 => Number, u8 => Number, u16 => Number, u32 => Number, u64 => Number);

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<Array> for Json {
    fn from(value: Array) -> Self {
        Self::Array(value)
    }
}

impl From<Object> for Json {
    fn from(value: Object) -> Self {
        Self::Object(value)
    }
}

impl From<Json> for Value {
    fn from(value: Json) -> Self {
        value.into_value()
    }
}

fn json_from_array(values: Vec<Value>) -> Result<Array, NullValueError> {
    let mut array = Array::new();
    for value in values {
        array.push(Json::try_from(value)?);
    }
    Ok(array)
}

fn json_from_object(
    values: Map<String, Value>,
) -> Result<Object, NullValueError> {
    let mut object = Object::new();
    for (key, value) in values {
        if !value.is_null() {
            object.insert(key, Some(Json::try_from(value)?));
        }
    }
    Ok(object)
}

impl TryFrom<Value> for Json {
    type Error = NullValueError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Null => Err(NullValueError),
            Value::Bool(inner) => Ok(Self::Bool(inner)),
            Value::Number(inner) => Ok(Self::Number(inner)),
            Value::String(inner) => Ok(Self::String(inner)),
            Value::Array(values) => json_from_array(values).map(Self::Array),
            Value::Object(values) => json_from_object(values).map(Self::Object),
        }
    }
}

impl TryFrom<Vec<Value>> for Array {
    type Error = NullValueError;

    fn try_from(values: Vec<Value>) -> Result<Self, Self::Error> {
        if values.iter().any(Value::is_null) {
            return Err(NullValueError);
        }
        Ok(Self(values))
    }
}

impl TryFrom<Map<String, Value>> for Object {
    type Error = NullValueError;

    fn try_from(values: Map<String, Value>) -> Result<Self, Self::Error> {
        if values.values().any(Value::is_null) {
            return Err(NullValueError);
        }
        Ok(Self(values))
    }
}

fn insert_non_null(object: &mut Object, key: String, value: Value) {
    if value.is_null() {
        return;
    }
    object.0.insert(key, value);
}

fn non_null_array(value: Value) -> Result<Value, &'static str> {
    if value.is_null() {
        Err("json null cannot be stored in a strict array")
    } else {
        Ok(value)
    }
}
impl Serialize for Object {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self.0.iter().filter(|(_, value)| !value.is_null())
        {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

struct ObjectVisitor;

impl<'de> Visitor<'de> for ObjectVisitor {
    type Value = Object;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object without null values")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = Object::new();
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            insert_non_null(&mut object, key, value);
        }
        Ok(object)
    }
}

impl<'de> Deserialize<'de> for Object {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ObjectVisitor)
    }
}

impl Serialize for Array {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self.0.iter())
    }
}

struct ArrayVisitor;

impl<'de> Visitor<'de> for ArrayVisitor {
    type Value = Array;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON array without null slots")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut array = Array::new();
        while let Some(value) = seq.next_element::<Value>()? {
            let present = non_null_array(value).map_err(A::Error::custom)?;
            array.0.push(present);
        }
        Ok(array)
    }
}

impl<'de> Deserialize<'de> for Array {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(ArrayVisitor)
    }
}

/// Returned when helper JSON contains a null the strict model cannot hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NullValueError;

impl fmt::Display for NullValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("json null is not part of the strict JSON model")
    }
}

impl StdError for NullValueError {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn none_omits_object_keys() {
        let mut object = Object::new();
        object
            .insert("present", Some(7))
            .insert("absent", None::<i64>);
        assert_eq!(
            serde_json::to_value(&object).expect("serialize"),
            json!({ "present": 7 })
        );
        assert!(object.get("absent").is_none());
    }

    #[test]
    fn null_object_keys_are_absent_on_read() {
        let object: Object =
            serde_json::from_value(json!({ "a": null, "b": 1 }))
                .expect("decode");
        assert_eq!(object.len(), 1);
        assert_eq!(
            serde_json::to_value(&object).expect("serialize"),
            json!({ "b": 1 })
        );
    }

    #[test]
    fn null_array_slots_are_rejected() {
        assert!(serde_json::from_value::<Array>(json!([1, null])).is_err());
        assert_eq!(
            serde_json::to_value(
                Json::try_from(json!({ "a": null })).expect("convert")
            )
            .expect("serialize"),
            json!({})
        );
        assert!(Json::try_from(json!({ "a": [null] })).is_err());
    }

    #[test]
    fn helper_conversion_keeps_all_non_null_values() {
        let value = json!({ "a": [1, true, "x"], "b": 2 });
        let json = Json::try_from(value.clone()).expect("convert");
        assert_eq!(json.into_value(), value);
    }
}
