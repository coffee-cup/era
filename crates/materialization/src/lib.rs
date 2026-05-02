//! Working-directory materialization and filesystem observation.

use std::path::{Path, PathBuf};

/// A materialized working directory root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingDirectory {
    root: PathBuf,
}

impl WorkingDirectory {
    /// Creates a working directory descriptor.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the working directory root path.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_directory_exposes_root() {
        let directory = WorkingDirectory::new("/tmp/project");

        assert_eq!(directory.root(), Path::new("/tmp/project"));
    }
}
