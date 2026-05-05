use era_core::TreeError;
use era_object_store::ObjectStoreError;
use notify::Error as NotifyError;
use std::{error::Error, fmt, io, path::PathBuf};

/// Errors returned while scanning or materializing a working directory.
#[derive(Debug)]
pub enum MaterializationError {
    /// The working directory root does not exist.
    RootMissing { path: PathBuf },
    /// The working directory root exists but is not a directory.
    RootNotDirectory { path: PathBuf },
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
    /// A filesystem watcher operation failed.
    Watch { path: PathBuf, source: NotifyError },
    /// A path segment could not be represented as UTF-8.
    PathNotUtf8 { path: PathBuf },
    /// A filesystem entry cannot be represented in the current tree model.
    UnsupportedFileType { path: PathBuf },
    /// A symlink was encountered while symlink handling was configured to error.
    SymlinkUnsupported { path: PathBuf },
    /// A captured directory entry was not valid for a tree object.
    InvalidTreeEntry { path: PathBuf, source: TreeError },
    /// The object store failed while storing captured data.
    ObjectStore { source: ObjectStoreError },
}

impl fmt::Display for MaterializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootMissing { path } => {
                write!(
                    formatter,
                    "working directory root is missing: {}",
                    path.display()
                )
            }
            Self::RootNotDirectory { path } => write!(
                formatter,
                "working directory root is not a directory: {}",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "filesystem error at {}: {source}",
                    path.display()
                )
            }
            Self::Watch { path, source } => {
                write!(
                    formatter,
                    "filesystem watch error at {}: {source}",
                    path.display()
                )
            }
            Self::PathNotUtf8 { path } => write!(
                formatter,
                "path contains a non-UTF-8 segment and cannot be captured: {}",
                path.display()
            ),
            Self::UnsupportedFileType { path } => write!(
                formatter,
                "filesystem entry type is not supported for capture: {}",
                path.display()
            ),
            Self::SymlinkUnsupported { path } => write!(
                formatter,
                "symlink capture is not supported in this mode: {}",
                path.display()
            ),
            Self::InvalidTreeEntry { path, source } => write!(
                formatter,
                "directory entry at {} cannot be represented in a tree: {source}",
                path.display()
            ),
            Self::ObjectStore { source } => write!(formatter, "object store error: {source}"),
        }
    }
}

impl Error for MaterializationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Watch { source, .. } => Some(source),
            Self::InvalidTreeEntry { source, .. } => Some(source),
            Self::ObjectStore { source } => Some(source),
            Self::RootMissing { .. }
            | Self::RootNotDirectory { .. }
            | Self::PathNotUtf8 { .. }
            | Self::UnsupportedFileType { .. }
            | Self::SymlinkUnsupported { .. } => None,
        }
    }
}

impl From<ObjectStoreError> for MaterializationError {
    fn from(source: ObjectStoreError) -> Self {
        Self::ObjectStore { source }
    }
}
