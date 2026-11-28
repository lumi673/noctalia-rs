//! Byte and text transport.
//!
//! `Bytes` and `Utf8Text` are the strict-mode types. Both map to a Luau byte
//! string. `Bytes` carries arbitrary bytes; `Utf8Text` is validated UTF-8.
//! Neither uses a marker when it crosses a JSON boundary: invalid bytes are
//! rejected instead.
//!
//! `ByteBuf` and the `__noctalia_rs_bytes` marker remain for the auto/off
//! bridge, where JSON strings cannot carry arbitrary bytes.

use std::fmt;
use std::ops::Deref;
use std::str::{Utf8Error, from_utf8};

use serde::de::{Deserializer, Error as DeError};
use serde::ser::{Error as SerError, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
/// Marker key used by Rust and generated Luau to identify byte buffers.
pub const BYTES_KEY: &str = "__noctalia_rs_bytes";

/// A buffer of arbitrary bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ByteBuf(Vec<u8>);

impl ByteBuf {
    /// Wraps an existing byte vector.
    #[inline]
    #[must_use]
    pub const fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns the raw bytes.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the buffer and returns the raw bytes.
    #[inline]
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for ByteBuf {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl From<&[u8]> for ByteBuf {
    #[inline]
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

impl AsRef<[u8]> for ByteBuf {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Deref for ByteBuf {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Arbitrary bytes that map to a Luau byte string.
///
/// Unlike [`ByteBuf`], this type never emits a bridge marker. A JSON boundary
/// is only valid while the bytes are UTF-8 text.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bytes(Vec<u8>);

impl Bytes {
    /// Wraps an existing byte vector.
    #[inline]
    #[must_use]
    pub const fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns the raw bytes.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the value and returns the raw bytes.
    #[inline]
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }

    /// Returns the bytes as text when they are valid UTF-8.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        from_utf8(&self.0).ok()
    }
}

impl From<Vec<u8>> for Bytes {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl From<&[u8]> for Bytes {
    #[inline]
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

impl From<Utf8Text> for Bytes {
    #[inline]
    fn from(value: Utf8Text) -> Self {
        Self(value.0.into_bytes())
    }
}

impl AsRef<[u8]> for Bytes {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Deref for Bytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Serialize for Bytes {
    #[inline]
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let text = from_utf8(&self.0).map_err(|error| {
            <S::Error as SerError>::custom(format!(
                "Bytes cannot cross a JSON boundary as invalid UTF-8: {error}"
            ))
        })?;
        serializer.serialize_str(text)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    #[inline]
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = Json::deserialize(deserializer)?;
        value.as_str().map_or_else(
            || {
                Err(<D::Error as DeError>::custom(
                    "Bytes expects a JSON string; byte markers are not part of the strict JSON model",
                ))
            },
            |text| Ok(Self(text.as_bytes().to_vec())),
        )
    }
}

/// Validated UTF-8 text that maps to a Luau byte string.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Utf8Text(String);

impl Utf8Text {
    /// Wraps text that is already valid UTF-8.
    #[inline]
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// Validates a byte buffer as UTF-8.
    #[inline]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the byte buffer is the validated input"
    )]
    pub fn from_bytes(bytes: Bytes) -> Result<Self, Utf8Error> {
        let text = from_utf8(&bytes.0)?;
        Ok(Self(text.to_owned()))
    }

    /// Returns the text.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the encoded bytes.
    #[inline]
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Consumes the value and returns the text.
    #[inline]
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<Bytes> for Utf8Text {
    type Error = Utf8Error;

    #[inline]
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        Self::from_bytes(value)
    }
}

impl TryFrom<&[u8]> for Utf8Text {
    type Error = Utf8Error;

    #[inline]
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(Bytes::from(value))
    }
}

impl From<&str> for Utf8Text {
    #[inline]
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Utf8Text {
    #[inline]
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl Deref for Utf8Text {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for Utf8Text {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Utf8Text {
    #[inline]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for Utf8Text {
    #[inline]
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Utf8Text {
    #[inline]
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

impl From<Bytes> for ByteBuf {
    #[inline]
    fn from(value: Bytes) -> Self {
        Self::new(value.0)
    }
}

impl From<ByteBuf> for Bytes {
    #[inline]
    fn from(value: ByteBuf) -> Self {
        Self(value.0)
    }
}

impl Serialize for ByteBuf {
    #[inline]
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(BYTES_KEY, &encode_hex(&self.0))?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for ByteBuf {
    #[inline]
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = Json::deserialize(deserializer)?;
        Ok(Self(decode_json_bytes(&value)))
    }
}

/// Encodes bytes as lowercase hexadecimal.
#[must_use]
pub fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let high = DIGITS.get(usize::from(byte >> 4)).copied().unwrap_or(b'0');
        let low = DIGITS
            .get(usize::from(byte & 0x0f))
            .copied()
            .unwrap_or(b'0');
        result.push(char::from(high));
        result.push(char::from(low));
    }
    result
}

/// Decodes hexadecimal text. Odd trailing digits are ignored.
#[must_use]
pub fn decode_hex(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len().saturating_div(2));
    let mut index: usize = 0;
    while index.saturating_add(1) < bytes.len() {
        let next = index.saturating_add(1);
        if let (Some(high), Some(low)) = (
            bytes.get(index).copied().and_then(hex_value),
            bytes.get(next).copied().and_then(hex_value),
        ) {
            result.push(high.saturating_mul(16).saturating_add(low));
        }
        index = next.saturating_add(1);
    }
    result
}

/// Returns the bytes of a bridge marker object, if the value is one.
///
/// A marker is an object whose only key is [`BYTES_KEY`] and whose value is an
/// even-length hex string. Requiring both keeps a plain object that happens to
/// carry that key from changing type.
#[must_use]
pub fn marker_bytes(value: &Json) -> Option<Vec<u8>> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let text = object.get(BYTES_KEY)?.as_str()?;
    if text.len() % 2 != 0
        || !text.bytes().all(|byte| hex_value(byte).is_some())
    {
        return None;
    }
    Some(decode_hex(text))
}

/// Extracts bytes from either a marker object or a JSON string.
#[must_use]
pub fn decode_json_bytes(value: &Json) -> Vec<u8> {
    if let Some(bytes) = marker_bytes(value) {
        return bytes;
    }
    value
        .as_str()
        .map_or_else(Vec::new, |text| text.as_bytes().to_vec())
}

#[inline]
const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte.saturating_sub(b'0')),
        b'a'..=b'f' => Some(byte.saturating_sub(b'a').saturating_add(10)),
        b'A'..=b'F' => Some(byte.saturating_sub(b'A').saturating_add(10)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hex_round_trips_arbitrary_bytes() {
        let bytes = vec![0, 1, 15, 16, 127, 128, 255];
        assert_eq!(decode_hex(&encode_hex(&bytes)), bytes);
    }

    #[test]
    fn byte_buf_serializes_with_the_bridge_marker() {
        let value = serde_json::to_value(ByteBuf::new(vec![0xff, 0x00]))
            .expect("serialize");
        assert_eq!(value, json!({ "__noctalia_rs_bytes": "ff00" }));
    }

    #[test]
    fn byte_buf_deserializes_text_and_markers() {
        let text: ByteBuf =
            serde_json::from_value(json!("hello")).expect("deserialize text");
        let marked: ByteBuf =
            serde_json::from_value(json!({ "__noctalia_rs_bytes": "00ff" }))
                .expect("deserialize marker");
        assert_eq!(text.as_bytes(), b"hello");
        assert_eq!(marked.as_bytes(), &[0, 255]);
    }

    #[test]
    fn bytes_serialize_as_text_and_reject_invalid_utf8() {
        let text = Bytes::from(b"hello".as_slice());
        assert_eq!(
            serde_json::to_value(&text).expect("serialize text"),
            json!("hello")
        );

        let binary = Bytes::new(vec![0xff, 0x00]);
        let error = serde_json::to_value(&binary).expect_err("reject bytes");
        assert!(error.to_string().contains("invalid UTF-8"), "{error}");
        assert!(
            !error.to_string().contains(BYTES_KEY),
            "a marker must not cross the JSON boundary: {error}"
        );
    }

    #[test]
    fn bytes_reject_a_bridge_marker() {
        let error = serde_json::from_value::<Bytes>(json!({
            "__noctalia_rs_bytes": "00ff"
        }))
        .expect_err("reject marker");
        assert!(error.to_string().contains("byte markers"), "{error}");
    }

    #[test]
    fn utf8_text_validates_and_round_trips() {
        let valid =
            Utf8Text::from_bytes(Bytes::from("é".as_bytes())).expect("text");
        assert_eq!(valid.as_str(), "é");
        assert_eq!(Bytes::from(valid.clone()).as_bytes(), "é".as_bytes());

        let error = Utf8Text::try_from(&[0xff][..]).expect_err("invalid");
        assert_eq!(error.valid_up_to(), 0);
        assert_eq!(
            serde_json::to_value(&valid).expect("serialize"),
            json!("é")
        );
        let decoded: Utf8Text =
            serde_json::from_value(json!("ok")).expect("deserialize");
        assert_eq!(decoded.as_str(), "ok");
    }
}
