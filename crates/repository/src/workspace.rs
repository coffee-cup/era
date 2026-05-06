use std::{error::Error, fmt, path::PathBuf, str::FromStr};

/// Workspace ID used until callers choose a specific workspace.
pub const DEFAULT_WORKSPACE_ID: &str = "default";

/// A durable per-directory workspace identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// Creates a workspace ID after validating it is safe to use as one metadata path segment.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidWorkspaceId> {
        let value = value.into();
        validate_workspace_id(&value)?;
        Ok(Self(value))
    }

    /// Returns the default workspace ID.
    #[must_use]
    pub fn default_id() -> Self {
        Self(DEFAULT_WORKSPACE_ID.to_owned())
    }

    /// Returns the workspace ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for WorkspaceId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for WorkspaceId {
    type Err = InvalidWorkspaceId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// Error returned for invalid workspace IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidWorkspaceId {
    id: String,
    reason: InvalidWorkspaceIdReason,
}

impl InvalidWorkspaceId {
    fn new(id: String, reason: InvalidWorkspaceIdReason) -> Self {
        Self { id, reason }
    }

    /// Returns the invalid workspace ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns why the workspace ID is invalid.
    #[must_use]
    pub fn reason(&self) -> InvalidWorkspaceIdReason {
        self.reason
    }
}

impl fmt::Display for InvalidWorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid workspace ID {:?}: {}",
            self.id, self.reason
        )
    }
}

impl Error for InvalidWorkspaceId {}

/// Reason a workspace ID is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvalidWorkspaceIdReason {
    /// Workspace IDs cannot be empty.
    Empty,
    /// `.` is not a valid workspace ID.
    CurrentDirectory,
    /// `..` is not a valid workspace ID.
    ParentDirectory,
    /// Slash is rejected so workspace IDs map to one metadata path segment.
    ContainsSlash,
    /// Backslash is rejected so workspace IDs are portable across platforms.
    ContainsBackslash,
    /// Workspace IDs cannot contain NUL.
    ContainsNul,
}

impl fmt::Display for InvalidWorkspaceIdReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("workspace ID is empty"),
            Self::CurrentDirectory => formatter.write_str("workspace ID cannot be '.'"),
            Self::ParentDirectory => formatter.write_str("workspace ID cannot be '..'"),
            Self::ContainsSlash => formatter.write_str("workspace ID contains '/'"),
            Self::ContainsBackslash => formatter.write_str("workspace ID contains '\\'"),
            Self::ContainsNul => formatter.write_str("workspace ID contains NUL"),
        }
    }
}

fn validate_workspace_id(id: &str) -> Result<(), InvalidWorkspaceId> {
    let reason = if id.is_empty() {
        Some(InvalidWorkspaceIdReason::Empty)
    } else if id == "." {
        Some(InvalidWorkspaceIdReason::CurrentDirectory)
    } else if id == ".." {
        Some(InvalidWorkspaceIdReason::ParentDirectory)
    } else if id.contains('/') {
        Some(InvalidWorkspaceIdReason::ContainsSlash)
    } else if id.contains('\\') {
        Some(InvalidWorkspaceIdReason::ContainsBackslash)
    } else if id.contains('\0') {
        Some(InvalidWorkspaceIdReason::ContainsNul)
    } else {
        None
    };

    match reason {
        Some(reason) => Err(InvalidWorkspaceId::new(id.to_owned(), reason)),
        None => Ok(()),
    }
}

/// Pointer stored in an external workspace's `.era` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePointer {
    pub(crate) metadata_dir: PathBuf,
    pub(crate) workspace_id: WorkspaceId,
}

impl WorkspacePointer {
    pub(crate) fn new(metadata_dir: PathBuf, workspace_id: WorkspaceId) -> Self {
        Self {
            metadata_dir,
            workspace_id,
        }
    }

    pub(crate) fn to_pointer_file(&self) -> String {
        format!(
            "era-workspace-v1\nmetadata: {}\nworkspace: {}\n",
            self.metadata_dir.display(),
            self.workspace_id
        )
    }
}
