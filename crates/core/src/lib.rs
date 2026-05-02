//! Shared domain types and primitives for the workspace.

/// Identifier for content-addressed objects.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectId(String);

impl ObjectId {
    /// Creates a new object identifier.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_id_exposes_inner_value() {
        let id = ObjectId::new("abc123");

        assert_eq!(id.as_str(), "abc123");
    }
}
