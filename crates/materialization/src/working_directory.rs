use std::path::{Path, PathBuf};

/// A materialized working directory root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingDirectory {
    root: PathBuf,
}

impl WorkingDirectory {
    /// Creates a working directory descriptor.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the working directory root path.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}
