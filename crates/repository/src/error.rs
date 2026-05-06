use crate::{BranchName, InvalidBranchName, InvalidWorkspaceId, WorkspaceId};
use era_core::ObjectId;
use era_materialization::MaterializationError;
use era_object_store::ObjectStoreError;
use std::{error::Error, fmt, io, path::PathBuf, time::SystemTimeError};

/// Errors returned by repository orchestration.
#[derive(Debug)]
pub enum RepositoryError {
    /// The requested working-directory root does not exist.
    RootMissing { path: PathBuf },
    /// The requested working-directory root exists but is not a directory.
    RootNotDirectory { path: PathBuf },
    /// The working directory already has Era metadata.
    AlreadyInitialized { path: PathBuf },
    /// The working directory does not contain Era metadata.
    NotRepository { path: PathBuf },
    /// The repository metadata directory exists but is not a directory.
    MetadataNotDirectory { path: PathBuf },
    /// Repository metadata did not contain a HEAD file.
    HeadMissing { path: PathBuf },
    /// HEAD existed but did not contain a supported branch ref.
    InvalidHead { path: PathBuf, contents: String },
    /// The current branch ref file was missing.
    RefMissing { path: PathBuf },
    /// A branch ref file did not contain a valid snapshot ID.
    InvalidRef { path: PathBuf, contents: String },
    /// A branch name could not be represented safely in the ref layout.
    InvalidBranchName { source: InvalidBranchName },
    /// A workspace ID could not be represented safely in the metadata layout.
    InvalidWorkspaceId { source: InvalidWorkspaceId },
    /// A branch ref file name could not be represented as UTF-8.
    InvalidBranchRefName { path: PathBuf },
    /// A branch already exists.
    BranchAlreadyExists { name: BranchName },
    /// A branch does not exist.
    BranchNotFound { name: BranchName },
    /// A workspace ID already belongs to another path.
    WorkspaceAlreadyExists {
        id: WorkspaceId,
        existing_path: PathBuf,
        requested_path: PathBuf,
    },
    /// A workspace does not exist.
    WorkspaceNotFound { id: WorkspaceId },
    /// A workspace path could not be represented safely in the metadata layout.
    InvalidWorkspaceRefName { path: PathBuf },
    /// A workspace pointer file is invalid.
    InvalidWorkspacePointer { path: PathBuf, contents: String },
    /// A workspace path is nested inside another workspace.
    WorkspaceInsideWorkspace {
        path: PathBuf,
        workspace_root: PathBuf,
    },
    /// A path is already an Era repository metadata directory or repository root.
    WorkspacePathIsRepository { path: PathBuf },
    /// A metadata lock could not be acquired before the timeout.
    LockTimeout { path: PathBuf },
    /// A snapshot target could not be resolved.
    SnapshotTargetNotFound { target: String },
    /// A snapshot target matched more than one snapshot.
    SnapshotTargetAmbiguous {
        target: String,
        matches: Vec<ObjectId>,
    },
    /// Timeline traversal found a cycle in snapshot parent pointers.
    SnapshotCycle { id: ObjectId },
    /// The system clock was before the Unix epoch.
    ClockBeforeUnixEpoch { source: SystemTimeError },
    /// The current timestamp did not fit Era's snapshot timestamp field.
    TimestampOverflow { millis: u128 },
    /// A repository metadata filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
    /// The object store failed while reading or writing repository objects.
    ObjectStore { source: ObjectStoreError },
    /// The materializer failed while capturing the working directory.
    Materialization { source: MaterializationError },
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootMissing { path } => write!(
                formatter,
                "repository working directory is missing: {}",
                path.display()
            ),
            Self::RootNotDirectory { path } => write!(
                formatter,
                "repository working-directory root is not a directory: {}",
                path.display()
            ),
            Self::AlreadyInitialized { path } => write!(
                formatter,
                "repository is already initialized: {}",
                path.display()
            ),
            Self::NotRepository { path } => {
                write!(formatter, "not an Era repository: {}", path.display())
            }
            Self::MetadataNotDirectory { path } => write!(
                formatter,
                "Era metadata path is not a directory: {}",
                path.display()
            ),
            Self::HeadMissing { path } => {
                write!(formatter, "repository HEAD is missing: {}", path.display())
            }
            Self::InvalidHead { path, contents } => write!(
                formatter,
                "repository HEAD at {} is invalid: {:?}",
                path.display(),
                contents
            ),
            Self::RefMissing { path } => write!(
                formatter,
                "current branch ref is missing: {}",
                path.display()
            ),
            Self::InvalidRef { path, contents } => write!(
                formatter,
                "branch ref at {} is invalid: {:?}",
                path.display(),
                contents
            ),
            Self::InvalidBranchName { source } => write!(formatter, "{source}"),
            Self::InvalidWorkspaceId { source } => write!(formatter, "{source}"),
            Self::InvalidBranchRefName { path } => write!(
                formatter,
                "branch ref name is not UTF-8: {}",
                path.display()
            ),
            Self::BranchAlreadyExists { name } => {
                write!(formatter, "branch already exists: {name}")
            }
            Self::BranchNotFound { name } => write!(formatter, "branch not found: {name}"),
            Self::WorkspaceAlreadyExists {
                id,
                existing_path,
                requested_path,
            } => write!(
                formatter,
                "workspace {id} is already registered at {}; requested {}",
                existing_path.display(),
                requested_path.display()
            ),
            Self::WorkspaceNotFound { id } => write!(formatter, "workspace not found: {id}"),
            Self::InvalidWorkspaceRefName { path } => write!(
                formatter,
                "workspace ref name is not UTF-8: {}",
                path.display()
            ),
            Self::InvalidWorkspacePointer { path, contents } => write!(
                formatter,
                "workspace pointer at {} is invalid: {:?}",
                path.display(),
                contents
            ),
            Self::WorkspaceInsideWorkspace {
                path,
                workspace_root,
            } => write!(
                formatter,
                "refusing to create a workspace inside another workspace: {} is inside {}",
                path.display(),
                workspace_root.display()
            ),
            Self::WorkspacePathIsRepository { path } => write!(
                formatter,
                "workspace path is already an Era repository: {}",
                path.display()
            ),
            Self::LockTimeout { path } => {
                write!(
                    formatter,
                    "timed out waiting for repository lock: {}",
                    path.display()
                )
            }
            Self::SnapshotTargetNotFound { target } => {
                write!(formatter, "snapshot target not found: {target}")
            }
            Self::SnapshotTargetAmbiguous { target, matches } => {
                write!(formatter, "snapshot target is ambiguous: {target} matches")?;
                for id in matches {
                    write!(formatter, " {id}")?;
                }
                Ok(())
            }
            Self::SnapshotCycle { id } => {
                write!(formatter, "snapshot timeline contains a cycle at {id}")
            }
            Self::ClockBeforeUnixEpoch { source } => {
                write!(formatter, "system clock is before the Unix epoch: {source}")
            }
            Self::TimestampOverflow { millis } => write!(
                formatter,
                "snapshot timestamp {millis}ms since the Unix epoch does not fit in u64"
            ),
            Self::Io { path, source } => write!(
                formatter,
                "repository filesystem error at {}: {source}",
                path.display()
            ),
            Self::ObjectStore { source } => write!(formatter, "object store error: {source}"),
            Self::Materialization { source } => {
                write!(formatter, "materialization error: {source}")
            }
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidBranchName { source } => Some(source),
            Self::InvalidWorkspaceId { source } => Some(source),
            Self::ClockBeforeUnixEpoch { source } => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::ObjectStore { source } => Some(source),
            Self::Materialization { source } => Some(source),
            Self::RootMissing { .. }
            | Self::RootNotDirectory { .. }
            | Self::AlreadyInitialized { .. }
            | Self::NotRepository { .. }
            | Self::MetadataNotDirectory { .. }
            | Self::HeadMissing { .. }
            | Self::InvalidHead { .. }
            | Self::RefMissing { .. }
            | Self::InvalidRef { .. }
            | Self::InvalidBranchRefName { .. }
            | Self::BranchAlreadyExists { .. }
            | Self::BranchNotFound { .. }
            | Self::WorkspaceAlreadyExists { .. }
            | Self::WorkspaceNotFound { .. }
            | Self::InvalidWorkspaceRefName { .. }
            | Self::InvalidWorkspacePointer { .. }
            | Self::WorkspaceInsideWorkspace { .. }
            | Self::WorkspacePathIsRepository { .. }
            | Self::LockTimeout { .. }
            | Self::SnapshotTargetNotFound { .. }
            | Self::SnapshotTargetAmbiguous { .. }
            | Self::SnapshotCycle { .. }
            | Self::TimestampOverflow { .. } => None,
        }
    }
}

impl From<ObjectStoreError> for RepositoryError {
    fn from(source: ObjectStoreError) -> Self {
        Self::ObjectStore { source }
    }
}

impl From<MaterializationError> for RepositoryError {
    fn from(source: MaterializationError) -> Self {
        Self::Materialization { source }
    }
}

impl From<InvalidBranchName> for RepositoryError {
    fn from(source: InvalidBranchName) -> Self {
        Self::InvalidBranchName { source }
    }
}

impl From<InvalidWorkspaceId> for RepositoryError {
    fn from(source: InvalidWorkspaceId) -> Self {
        Self::InvalidWorkspaceId { source }
    }
}

pub(crate) fn io_error(path: PathBuf, source: io::Error) -> RepositoryError {
    RepositoryError::Io { path, source }
}
