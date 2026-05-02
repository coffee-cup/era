use era_core::{ObjectId, ObjectKind, SnapshotError, TreeError};
use std::{error::Error, fmt, io, path::PathBuf};

/// Errors returned by object store implementations.
#[derive(Debug)]
pub enum ObjectStoreError {
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
    /// A requested object was not present.
    MissingObject {
        kind: ObjectKind,
        id: ObjectId,
        path: PathBuf,
    },
    /// An object path existed, but its contents did not hash to the expected ID.
    HashMismatch {
        kind: ObjectKind,
        path: PathBuf,
        expected: ObjectId,
        actual: ObjectId,
    },
    /// A tree object was hash-valid, but did not decode as canonical tree bytes.
    InvalidTreeObject {
        id: ObjectId,
        path: PathBuf,
        source: TreeError,
    },
    /// A snapshot object was hash-valid, but did not decode as canonical snapshot bytes.
    InvalidSnapshotObject {
        id: ObjectId,
        path: PathBuf,
        source: SnapshotError,
    },
    /// The store could not allocate a collision-free temporary path.
    TempFileExhausted {
        kind: ObjectKind,
        directory: PathBuf,
        id: ObjectId,
    },
}

impl fmt::Display for ObjectStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(
                formatter,
                "object store filesystem error at {}: {source}",
                path.display()
            ),
            Self::MissingObject { kind, id, path } => {
                write!(
                    formatter,
                    "missing {kind} object {id} at {}",
                    path.display()
                )
            }
            Self::HashMismatch {
                kind,
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "{kind} object integrity check failed at {}: expected {expected}, got {actual}",
                path.display()
            ),
            Self::InvalidTreeObject { id, path, source } => write!(
                formatter,
                "tree object {id} at {} is not canonical: {source}",
                path.display()
            ),
            Self::InvalidSnapshotObject { id, path, source } => write!(
                formatter,
                "snapshot object {id} at {} is not canonical: {source}",
                path.display()
            ),
            Self::TempFileExhausted {
                kind,
                directory,
                id,
            } => write!(
                formatter,
                "could not allocate temporary file for {kind} object {id} in {}",
                directory.display()
            ),
        }
    }
}

impl Error for ObjectStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidTreeObject { source, .. } => Some(source),
            Self::InvalidSnapshotObject { source, .. } => Some(source),
            Self::MissingObject { .. }
            | Self::HashMismatch { .. }
            | Self::TempFileExhausted { .. } => None,
        }
    }
}
