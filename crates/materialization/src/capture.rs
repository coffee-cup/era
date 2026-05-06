use crate::{MaterializationError, WorkingDirectory, WorkingDirectoryWatch};
use async_trait::async_trait;
use era_core::ObjectId;
use era_object_store::ObjectStore;
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

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

    /// Captures the working directory, allowing implementations to reuse trusted change hints.
    async fn capture_tree_with_hints(
        &self,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
        hints: &[PathBuf],
    ) -> Result<CaptureResult, MaterializationError> {
        let _ = hints;
        self.capture_tree(working_directory, object_store).await
    }

    /// Scans the current working directory and returns the tree ID it would capture.
    async fn scan_tree(
        &self,
        working_directory: &WorkingDirectory,
    ) -> Result<TreeScanResult, MaterializationError>;

    /// Compares the current working directory with a stored root tree.
    async fn compare_tree(
        &self,
        saved_root_tree_id: ObjectId,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<TreeComparisonResult, MaterializationError>;

    /// Reconciles the working directory to match a stored tree.
    async fn materialize_tree(
        &self,
        root_tree_id: ObjectId,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<MaterializeResult, MaterializationError>;

    /// Watches a working directory and emits filesystem change hints.
    async fn watch(
        &self,
        working_directory: &WorkingDirectory,
    ) -> Result<WorkingDirectoryWatch, MaterializationError>;
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

    /// Returns `true` if any path segment matches an excluded directory name.
    #[must_use]
    pub fn excludes_path(&self, path: &Path) -> bool {
        path.components().any(|component| match component {
            Component::Normal(name) => name
                .to_str()
                .is_some_and(|name| self.excludes_directory_name(name)),
            _ => false,
        })
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

/// Result of scanning a working directory without storing objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeScanResult {
    /// Object ID of the scanned root tree.
    pub root_tree_id: ObjectId,
    /// Aggregate scan counts.
    pub stats: TreeScanStats,
    /// Non-fatal issues encountered during scan.
    pub issues: Vec<CaptureIssue>,
}

impl TreeScanResult {
    /// Creates a tree scan result.
    #[must_use]
    pub fn new(root_tree_id: ObjectId, stats: TreeScanStats, issues: Vec<CaptureIssue>) -> Self {
        Self {
            root_tree_id,
            stats,
            issues,
        }
    }
}

/// Result of comparing a working directory with a stored tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeComparisonResult {
    /// Root tree ID of the saved snapshot used as the comparison base.
    pub saved_root_tree_id: ObjectId,
    /// Root tree ID computed from the current working directory.
    pub current_root_tree_id: ObjectId,
    /// Path-level changes from the saved tree to the current working directory.
    pub changes: Vec<TreeChange>,
    /// Aggregate scan counts for the current working directory.
    pub stats: TreeScanStats,
    /// Non-fatal issues encountered during comparison.
    pub issues: Vec<CaptureIssue>,
}

impl TreeComparisonResult {
    /// Creates a tree comparison result.
    #[must_use]
    pub fn new(
        saved_root_tree_id: ObjectId,
        current_root_tree_id: ObjectId,
        changes: Vec<TreeChange>,
        stats: TreeScanStats,
        issues: Vec<CaptureIssue>,
    ) -> Self {
        Self {
            saved_root_tree_id,
            current_root_tree_id,
            changes,
            stats,
            issues,
        }
    }

    /// Returns `true` when no path-level changes were found.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.saved_root_tree_id == self.current_root_tree_id && self.changes.is_empty()
    }
}

/// A path-level change between a saved tree and the current working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeChange {
    /// Path relative to the working directory root.
    pub path: PathBuf,
    /// Kind of change detected for this path.
    pub kind: TreeChangeKind,
}

impl TreeChange {
    /// Creates a path-level change.
    #[must_use]
    pub fn new(path: PathBuf, kind: TreeChangeKind) -> Self {
        Self { path, kind }
    }

    /// Creates an added-path change.
    #[must_use]
    pub fn added(path: impl Into<PathBuf>) -> Self {
        Self::new(path.into(), TreeChangeKind::Added)
    }

    /// Creates a modified-path change.
    #[must_use]
    pub fn modified(path: impl Into<PathBuf>) -> Self {
        Self::new(path.into(), TreeChangeKind::Modified)
    }

    /// Creates a deleted-path change.
    #[must_use]
    pub fn deleted(path: impl Into<PathBuf>) -> Self {
        Self::new(path.into(), TreeChangeKind::Deleted)
    }

    /// Creates a type-changed-path change.
    #[must_use]
    pub fn type_changed(path: impl Into<PathBuf>) -> Self {
        Self::new(path.into(), TreeChangeKind::TypeChanged)
    }
}

/// Kind of path-level tree change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeChangeKind {
    /// Path exists only in the current working directory.
    Added,
    /// Path exists in both trees but points at different file contents.
    Modified,
    /// Path exists only in the saved tree.
    Deleted,
    /// Path changed between a file and a directory.
    TypeChanged,
}

/// Aggregate counts for a read-only tree scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TreeScanStats {
    /// Regular files included in the scanned tree.
    pub files_seen: usize,
    /// Directories included in the scanned tree, including the root directory.
    pub directories_seen: usize,
    /// Bytes read from regular files.
    pub bytes_read: u64,
    /// Directory entries skipped by configured exclusions.
    pub ignored_entries: usize,
    /// Symlink entries skipped by [`SymlinkPolicy::Skip`].
    pub symlinks_skipped: usize,
    /// File hashes reused from the capture cache.
    pub hash_cache_hits: usize,
    /// File hashes read from disk because no reusable cache entry existed.
    pub hash_cache_misses: usize,
}

/// Result of materializing a stored tree into a working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeResult {
    /// Aggregate filesystem reconciliation counts.
    pub stats: MaterializeStats,
}

impl MaterializeResult {
    /// Creates a materialization result.
    #[must_use]
    pub fn new(stats: MaterializeStats) -> Self {
        Self { stats }
    }
}

/// Aggregate counts for materializing a tree into a working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MaterializeStats {
    /// Regular files written from blob objects.
    pub files_written: usize,
    /// Directories created while materializing tree objects.
    pub directories_created: usize,
    /// Filesystem entries removed because they were not present in the target tree.
    pub entries_removed: usize,
    /// Bytes written to regular files.
    pub bytes_written: u64,
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
    /// File hashes reused from the capture cache.
    pub hash_cache_hits: usize,
    /// File hashes read from disk because no reusable cache entry existed.
    pub hash_cache_misses: usize,
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
