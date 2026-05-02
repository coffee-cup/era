//! Content-addressed object storage abstractions.

/// Bytes for a stored file object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    bytes: Vec<u8>,
}

impl Blob {
    /// Creates a blob from raw bytes.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    /// Returns the blob contents.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_retains_bytes() {
        let blob = Blob::new(b"hello".to_vec());

        assert_eq!(blob.bytes(), b"hello");
    }
}
