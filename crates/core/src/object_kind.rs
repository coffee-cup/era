use std::fmt;

/// Kind of immutable object stored in an Era object store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    /// Raw file bytes.
    Blob,
    /// Canonical serialized directory listing.
    Tree,
}

impl fmt::Display for ObjectKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob => formatter.write_str("blob"),
            Self::Tree => formatter.write_str("tree"),
        }
    }
}
