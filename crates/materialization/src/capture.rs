use crate::{MaterializationError, WorkingDirectory};
use async_trait::async_trait;
use era_core::ObjectId;
use era_object_store::ObjectStore;
use std::{collections::BTreeSet, path::PathBuf};

const DEFAULT_EXCLUDED_DIRECTORY_NAMES: &[&str] = &[
    ".era",
    ".git",
    "target",
    "node_modules",
    ".next",
    "dist",
    "build",
    ".cache",
    "__pycache__",
];

/// Async materialization capability used by the repository layer.
#[async_trait]
pub trait Materializer: Send + Sync {
    /// Captures the current working directory into blob and tree objects.
    async fn capture_tree(
        &self,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<CaptureResult, MaterializationError>;
}

/// Configuration for scanning a working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOptions {
    excluded_directory_names: BTreeSet<String>,
    symlink_policy: SymlinkPolicy,
}

impl CaptureOptions {
    /// Creates options with no excluded directory names and default symlink handling.
    #[must_use]
    pub fn no_exclusions() -> Self {
        Self {
            excluded_directory_names: BTreeSet::new(),
            symlink_policy: SymlinkPolicy::default(),
        }
    }

    /// Returns the exact directory names skipped during capture.
    #[must_use]
    pub fn excluded_directory_names(&self) -> &BTreeSet<String> {
        &self.excluded_directory_names
    }

    /// Returns `true` if a directory with this exact name should be skipped.
    #[must_use]
    pub fn excludes_directory_name(&self, name: &str) -> bool {
        self.excluded_directory_names.contains(name)
    }

    /// Adds an exact directory name to skip during capture.
    #[must_use]
    pub fn with_excluded_directory(mut self, name: impl Into<String>) -> Self {
        self.excluded_directory_names.insert(name.into());
        self
    }

    /// Removes an exact directory name from the skip list.
    #[must_use]
    pub fn without_excluded_directory(mut self, name: &str) -> Self {
        self.excluded_directory_names.remove(name);
        self
    }

    /// Sets how symlinks are handled during capture.
    #[must_use]
    pub fn with_symlink_policy(mut self, policy: SymlinkPolicy) -> Self {
        self.symlink_policy = policy;
        self
    }

    /// Returns how symlinks are handled during capture.
    #[must_use]
    pub fn symlink_policy(&self) -> SymlinkPolicy {
        self.symlink_policy
    }
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            excluded_directory_names: DEFAULT_EXCLUDED_DIRECTORY_NAMES
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            symlink_policy: SymlinkPolicy::default(),
        }
    }
}

/// Policy for symlinks found during capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SymlinkPolicy {
    /// Skip symlinks, record them as non-fatal issues, and do not follow them.
    #[default]
    Skip,
    /// Return an error when a symlink is found.
    Error,
}

/// Result of capturing a working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    /// Object ID of the captured root tree.
    pub root_tree_id: ObjectId,
    /// Aggregate scan and storage counts.
    pub stats: CaptureStats,
    /// Non-fatal issues encountered during capture.
    pub issues: Vec<CaptureIssue>,
}

impl CaptureResult {
    /// Creates a capture result.
    #[must_use]
    pub fn new(root_tree_id: ObjectId, stats: CaptureStats, issues: Vec<CaptureIssue>) -> Self {
        Self {
            root_tree_id,
            stats,
            issues,
        }
    }
}

/// Aggregate counts for a capture operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureStats {
    /// Regular files captured into blobs.
    pub files_seen: usize,
    /// Directories captured into trees, including the root directory.
    pub directories_seen: usize,
    /// Bytes read from regular files.
    pub bytes_read: u64,
    /// Blob store writes requested for captured regular files.
    pub blobs_stored: usize,
    /// Tree store writes requested for captured directories.
    pub trees_stored: usize,
    /// Directory entries skipped by configured exclusions.
    pub ignored_entries: usize,
    /// Symlink entries skipped by [`SymlinkPolicy::Skip`].
    pub symlinks_skipped: usize,
}

/// A non-fatal capture issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureIssue {
    /// Path relative to the working directory root.
    pub path: PathBuf,
    /// Reason this issue was recorded.
    pub kind: CaptureIssueKind,
}

impl CaptureIssue {
    /// Creates a capture issue.
    #[must_use]
    pub fn new(path: PathBuf, kind: CaptureIssueKind) -> Self {
        Self { path, kind }
    }
}

/// Reason a non-fatal capture issue was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureIssueKind {
    /// A symlink was skipped instead of followed.
    SkippedSymlink,
}
