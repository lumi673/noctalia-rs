//! Raw byte transport across the JSON bridge.
//!
//! Luau strings can hold arbitrary bytes while JSON strings cannot. Byte
//! buffers are carried through spool files as a tagged hex string and turned
//! back into Luau strings by the generated bridge.

use std::ops::Deref;

use serde::de::Deserializer;
use serde::ser::Serializer;
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

    /// Interprets the bytes as UTF-8 text, replacing invalid sequences.
    #[inline]
    #[must_use]
    pub fn to_text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
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

/// Extracts bytes from either a marker object or a JSON string.
#[must_use]
pub fn decode_json_bytes(value: &Json) -> Vec<u8> {
    if let Some(text) = value.get(BYTES_KEY).and_then(Json::as_str) {
        return decode_hex(text);
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
}
