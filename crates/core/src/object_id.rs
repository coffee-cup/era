use std::{error::Error, fmt, str::FromStr};

/// Number of bytes in a BLAKE3 content hash.
pub const OBJECT_ID_BYTES: usize = 32;

/// Number of lowercase hexadecimal characters in an [`ObjectId`].
pub const OBJECT_ID_HEX_LENGTH: usize = OBJECT_ID_BYTES * 2;

/// Identifier for content-addressed objects.
///
/// Era object IDs are BLAKE3 hashes represented canonically as 64 lowercase
/// hexadecimal characters at API boundaries.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId([u8; OBJECT_ID_BYTES]);

impl ObjectId {
    /// Creates an object identifier from raw hash bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; OBJECT_ID_BYTES]) -> Self {
        Self(bytes)
    }

    /// Hashes content with BLAKE3 and returns its object identifier.
    #[must_use]
    pub fn from_content(content: impl AsRef<[u8]>) -> Self {
        let hash = blake3::hash(content.as_ref());
        Self(*hash.as_bytes())
    }

    /// Parses a hexadecimal object identifier.
    pub fn from_hex(value: &str) -> Result<Self, ParseObjectIdError> {
        value.parse()
    }

    /// Returns the raw hash bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; OBJECT_ID_BYTES] {
        &self.0
    }

    /// Returns the canonical lowercase hexadecimal representation.
    #[must_use]
    pub fn to_hex(self) -> String {
        let mut output = String::with_capacity(OBJECT_ID_HEX_LENGTH);
        for byte in self.0 {
            output.push(hex_digit(byte >> 4));
            output.push(hex_digit(byte & 0x0f));
        }
        output
    }

    /// Returns the first byte of the object ID as lowercase hex.
    ///
    /// Local stores use this as a simple shard prefix to avoid putting every
    /// object in a single directory.
    #[must_use]
    pub fn shard_prefix(self) -> String {
        let byte = self.0[0];
        let mut output = String::with_capacity(2);
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
        output
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ObjectId({self})")
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for ObjectId {
    type Err = ParseObjectIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let input = value.as_bytes();
        if input.len() != OBJECT_ID_HEX_LENGTH {
            return Err(ParseObjectIdError::InvalidLength {
                actual: input.len(),
            });
        }

        let mut bytes = [0_u8; OBJECT_ID_BYTES];
        for (index, chunk) in input.chunks_exact(2).enumerate() {
            let high = decode_hex_digit(chunk[0]).ok_or(ParseObjectIdError::InvalidHex {
                index: index * 2,
                byte: chunk[0],
            })?;
            let low = decode_hex_digit(chunk[1]).ok_or(ParseObjectIdError::InvalidHex {
                index: index * 2 + 1,
                byte: chunk[1],
            })?;
            bytes[index] = (high << 4) | low;
        }

        Ok(Self(bytes))
    }
}

impl TryFrom<&str> for ObjectId {
    type Error = ParseObjectIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_hex(value)
    }
}

/// Error returned when parsing an object ID from text fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseObjectIdError {
    /// The value was not exactly 64 bytes long.
    InvalidLength { actual: usize },
    /// The value contained a non-hex byte.
    InvalidHex { index: usize, byte: u8 },
}

impl fmt::Display for ParseObjectIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength { actual } => write!(
                formatter,
                "object ID must be {OBJECT_ID_HEX_LENGTH} hex characters, got {actual}"
            ),
            Self::InvalidHex { index, byte } => write!(
                formatter,
                "object ID contains non-hex byte 0x{byte:02x} at index {index}"
            ),
        }
    }
}

impl Error for ParseObjectIdError {}

fn hex_digit(value: u8) -> char {
    debug_assert!(value < 16);
    char::from(b"0123456789abcdef"[usize::from(value)])
}

fn decode_hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_uses_blake3() {
        let id = ObjectId::from_content([]);

        assert_eq!(
            id.to_string(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn object_id_round_trips_through_hex() {
        let original = ObjectId::from_content(b"hello");
        let parsed = ObjectId::from_hex(&original.to_string()).unwrap();

        assert_eq!(parsed, original);
    }

    #[test]
    fn parser_accepts_uppercase_hex_but_display_is_lowercase() {
        let parsed =
            ObjectId::from_hex("AF1349B9F5F9A1A6A0404DEA36DCC9499BCB25C9ADC112B7CC9A93CAE41F3262")
                .unwrap();

        assert_eq!(
            parsed.to_string(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn parser_rejects_wrong_length() {
        let error = ObjectId::from_hex("abc").unwrap_err();

        assert_eq!(error, ParseObjectIdError::InvalidLength { actual: 3 });
    }

    #[test]
    fn parser_rejects_non_hex_characters() {
        let error =
            ObjectId::from_hex("zf1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262")
                .unwrap_err();

        assert_eq!(
            error,
            ParseObjectIdError::InvalidHex {
                index: 0,
                byte: b'z'
            }
        );
    }
}
