use crate::{
    BranchName, RepositoryError,
    error::io_error,
    refs::{
        branch_ref_exists, branch_ref_path, create_branch_ref, create_ref_layout, head_path,
        list_branch_refs, metadata_dir, objects_dir, read_branch_ref, read_head_branch,
        write_branch_ref, write_head,
    },
};
use era_core::{ObjectId, ObjectKind, Snapshot, SnapshotProvenance};
use era_materialization::{
    CaptureResult, MaterializeResult, Materializer, TreeChange, TreeComparisonResult,
    WorkingDirectory,
};
use era_object_store::LocalObjectStore;
use std::{
    collections::{BTreeMap, HashSet},
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::fs;
use tracing::{debug, trace};

/// Workspace ID used until repositories support multiple registered workspaces.
pub const DEFAULT_WORKSPACE_ID: &str = "default";

/// A local Era repository rooted at a working directory.
#[derive(Debug, Clone)]
pub struct Repository {
    root: PathBuf,
    metadata_dir: PathBuf,
    object_store: LocalObjectStore,
}

impl Repository {
    /// Initializes a new repository, captures the initial tree, and points `main` at it.
    pub async fn init(
        root: impl Into<PathBuf>,
        materializer: &dyn Materializer,
        request: SnapshotRequest,
    ) -> Result<InitResult, RepositoryError> {
        let root = root.into();
        debug!(root = %root.display(), "initializing repository");
        ensure_working_root(&root).await?;

        let metadata_dir = metadata_dir(&root);
        if path_exists(&metadata_dir).await? {
            return Err(RepositoryError::AlreadyInitialized { path: metadata_dir });
        }

        fs::create_dir(&metadata_dir)
            .await
            .map_err(|source| io_error(metadata_dir.clone(), source))?;
        create_ref_layout(&metadata_dir).await?;
        let object_store = LocalObjectStore::open(objects_dir(&metadata_dir)).await?;
        let repository = Self {
            root,
            metadata_dir,
            object_store,
        };

        let branch = BranchName::main();
        let snapshot = repository
            .capture_snapshot(materializer, request, Vec::new())
            .await?;
        write_head(&repository.metadata_dir, &branch).await?;
        write_branch_ref(&repository.metadata_dir, &branch, snapshot.snapshot_id).await?;

        debug!(%snapshot.snapshot_id, branch = branch.as_str(), "repository initialized");
        Ok(InitResult {
            repository,
            snapshot,
        })
    }

    /// Opens an existing repository at `root`.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, RepositoryError> {
        let root = root.into();
        debug!(root = %root.display(), "opening repository");
        ensure_working_root(&root).await?;

        let metadata_dir = metadata_dir(&root);
        ensure_metadata_dir(&metadata_dir).await?;
        read_head_branch(&metadata_dir).await?;
        let object_store = LocalObjectStore::open(objects_dir(&metadata_dir)).await?;

        Ok(Self {
            root,
            metadata_dir,
            object_store,
        })
    }

    /// Returns the working-directory root path.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the `.era` metadata directory path.
    #[must_use]
    pub fn metadata_dir(&self) -> &Path {
        &self.metadata_dir
    }

    /// Returns the local object store for this repository.
    #[must_use]
    pub fn object_store(&self) -> &LocalObjectStore {
        &self.object_store
    }

    /// Captures a manual snapshot and advances the current branch reference.
    pub async fn snapshot(
        &self,
        materializer: &dyn Materializer,
        request: SnapshotRequest,
    ) -> Result<SnapshotResult, RepositoryError> {
        let branch = self.current_branch().await?;
        let parent = read_branch_ref(&self.metadata_dir, &branch).await?;
        debug!(branch = branch.as_str(), parent = %parent, "creating manual snapshot");

        let snapshot = self
            .capture_snapshot(materializer, request, vec![parent])
            .await?;
        write_branch_ref(&self.metadata_dir, &branch, snapshot.snapshot_id).await?;

        debug!(branch = branch.as_str(), %snapshot.snapshot_id, "advanced branch ref");
        Ok(snapshot)
    }

    /// Captures and advances the current branch only when the working tree changed.
    pub async fn snapshot_if_changed(
        &self,
        materializer: &dyn Materializer,
        request: SnapshotRequest,
    ) -> Result<Option<SnapshotResult>, RepositoryError> {
        let branch = self.current_branch().await?;
        let parent = read_branch_ref(&self.metadata_dir, &branch).await?;
        let parent_snapshot = self.object_store.get_snapshot(&parent).await?;
        let capture = self.capture_working_tree(materializer).await?;
        if capture.root_tree_id == parent_snapshot.root_tree_id() {
            debug!(branch = branch.as_str(), parent = %parent, "working tree unchanged; skipping snapshot");
            return Ok(None);
        }

        let snapshot = self.store_snapshot(capture, request, vec![parent]).await?;
        write_branch_ref(&self.metadata_dir, &branch, snapshot.snapshot_id).await?;

        debug!(branch = branch.as_str(), %snapshot.snapshot_id, "advanced branch ref after change capture");
        Ok(Some(snapshot))
    }

    /// Returns whether the working directory matches the current branch snapshot.
    pub async fn working_tree_status(
        &self,
        materializer: &dyn Materializer,
    ) -> Result<WorkingTreeStatus, RepositoryError> {
        let snapshot_id = self.current_snapshot_id().await?;
        let snapshot = self.object_store.get_snapshot(&snapshot_id).await?;
        let working_directory = WorkingDirectory::new(&self.root);
        let comparison = materializer
            .compare_tree(
                snapshot.root_tree_id(),
                &working_directory,
                &self.object_store,
            )
            .await?;
        let current_root_tree_id = comparison.current_root_tree_id;
        let clean = comparison.is_clean();

        Ok(WorkingTreeStatus {
            snapshot_id,
            snapshot,
            current_root_tree_id,
            clean,
            comparison,
        })
    }

    /// Lists local branches sorted by name.
    pub async fn branches(&self) -> Result<Vec<BranchHead>, RepositoryError> {
        let current = self.current_branch().await?;
        let branches = list_branch_refs(&self.metadata_dir)
            .await?
            .into_iter()
            .map(|(name, snapshot_id)| {
                let is_current = name == current;
                BranchHead {
                    name,
                    snapshot_id,
                    is_current,
                }
            })
            .collect();

        Ok(branches)
    }

    /// Creates a branch at the current saved state, saving unsnapped work first.
    pub async fn create_branch(
        &self,
        materializer: &dyn Materializer,
        name: BranchName,
    ) -> Result<BranchOperationResult, RepositoryError> {
        if branch_ref_exists(&self.metadata_dir, &name).await? {
            return Err(RepositoryError::BranchAlreadyExists { name });
        }

        let saved_snapshot = self.save_current_state_if_changed(materializer).await?;
        let snapshot_id = self.current_snapshot_id().await?;
        create_branch_ref(&self.metadata_dir, &name, snapshot_id).await?;

        debug!(branch = name.as_str(), %snapshot_id, "created branch");
        Ok(BranchOperationResult {
            branch: name,
            snapshot_id,
            saved_snapshot,
        })
    }

    /// Switches HEAD to an existing branch, saving unsnapped work first.
    pub async fn switch_branch(
        &self,
        materializer: &dyn Materializer,
        name: BranchName,
    ) -> Result<SwitchResult, RepositoryError> {
        if !branch_ref_exists(&self.metadata_dir, &name).await? {
            return Err(RepositoryError::BranchNotFound { name });
        }

        let saved_snapshot = self.save_current_state_if_changed(materializer).await?;
        let snapshot_id = read_branch_ref(&self.metadata_dir, &name).await?;
        let snapshot = self.object_store.get_snapshot(&snapshot_id).await?;
        let working_directory = WorkingDirectory::new(&self.root);
        let materialization = materializer
            .materialize_tree(
                snapshot.root_tree_id(),
                &working_directory,
                &self.object_store,
            )
            .await?;
        write_head(&self.metadata_dir, &name).await?;

        debug!(branch = name.as_str(), %snapshot_id, "switched branch");
        Ok(SwitchResult {
            branch: name,
            snapshot_id,
            snapshot,
            saved_snapshot,
            materialization,
        })
    }

    /// Restores a snapshot target into the working directory without moving the current branch.
    pub async fn restore(
        &self,
        materializer: &dyn Materializer,
        target: &str,
    ) -> Result<RestoreResult, RepositoryError> {
        let resolved = self.resolve_snapshot_target(target).await?;
        let saved_snapshot = self.save_current_state_if_changed(materializer).await?;
        let working_directory = WorkingDirectory::new(&self.root);
        let materialization = materializer
            .materialize_tree(
                resolved.snapshot.root_tree_id(),
                &working_directory,
                &self.object_store,
            )
            .await?;

        debug!(target, snapshot = %resolved.snapshot_id, "restored snapshot target");
        Ok(RestoreResult {
            snapshot_id: resolved.snapshot_id,
            snapshot: resolved.snapshot,
            saved_snapshot,
            materialization,
        })
    }

    /// Resolves a snapshot target from a full ID, unique prefix, or exact message.
    pub async fn resolve_snapshot_target(
        &self,
        target: &str,
    ) -> Result<ResolvedSnapshot, RepositoryError> {
        let full_id_snapshot = if let Ok(id) = ObjectId::from_hex(target) {
            if self
                .object_store
                .contains(ObjectKind::Snapshot, &id)
                .await?
            {
                Some((id, self.object_store.get_snapshot(&id).await?))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((snapshot_id, snapshot)) = full_id_snapshot {
            return Ok(ResolvedSnapshot {
                snapshot_id,
                snapshot,
            });
        }

        let mut matches = BTreeMap::new();
        let target_is_hex_prefix =
            !target.is_empty() && target.bytes().all(|byte| byte.is_ascii_hexdigit());
        for entry in self.timeline().await? {
            if target_is_hex_prefix && entry.snapshot_id.to_hex().starts_with(target) {
                matches.insert(entry.snapshot_id, entry.snapshot.clone());
            }

            if entry.snapshot.message() == Some(target) {
                matches.insert(entry.snapshot_id, entry.snapshot);
            }
        }

        match matches.len() {
            0 => Err(RepositoryError::SnapshotTargetNotFound {
                target: target.to_owned(),
            }),
            1 => {
                let (snapshot_id, snapshot) = matches.into_iter().next().expect("one match");
                Ok(ResolvedSnapshot {
                    snapshot_id,
                    snapshot,
                })
            }
            _ => Err(RepositoryError::SnapshotTargetAmbiguous {
                target: target.to_owned(),
                matches: matches.keys().copied().collect(),
            }),
        }
    }

    /// Returns the branch named by HEAD.
    pub async fn current_branch(&self) -> Result<BranchName, RepositoryError> {
        read_head_branch(&self.metadata_dir).await
    }

    /// Returns the current snapshot ID for HEAD's branch.
    pub async fn current_snapshot_id(&self) -> Result<ObjectId, RepositoryError> {
        let branch = self.current_branch().await?;
        read_branch_ref(&self.metadata_dir, &branch).await
    }

    /// Returns the first-parent timeline from newest to oldest.
    pub async fn timeline(&self) -> Result<Vec<TimelineEntry>, RepositoryError> {
        let mut current = self.current_snapshot_id().await?;
        let mut seen = HashSet::new();
        let mut entries = Vec::new();

        loop {
            if !seen.insert(current) {
                return Err(RepositoryError::SnapshotCycle { id: current });
            }

            let snapshot = self.object_store.get_snapshot(&current).await?;
            trace!(%current, parents = snapshot.parents().len(), "timeline snapshot loaded");
            let next = snapshot.parents().first().copied();
            entries.push(TimelineEntry {
                snapshot_id: current,
                snapshot,
            });

            match next {
                Some(parent) => current = parent,
                None => break,
            }
        }

        Ok(entries)
    }

    /// Returns the path used for the current branch ref file.
    pub async fn current_branch_ref_path(&self) -> Result<PathBuf, RepositoryError> {
        let branch = self.current_branch().await?;
        Ok(branch_ref_path(&self.metadata_dir, &branch))
    }

    /// Returns the path used for the HEAD file.
    #[must_use]
    pub fn head_path(&self) -> PathBuf {
        head_path(&self.metadata_dir)
    }

    async fn save_current_state_if_changed(
        &self,
        materializer: &dyn Materializer,
    ) -> Result<Option<SnapshotResult>, RepositoryError> {
        self.snapshot_if_changed(materializer, SnapshotRequest::automatic())
            .await
    }

    async fn capture_snapshot(
        &self,
        materializer: &dyn Materializer,
        request: SnapshotRequest,
        parents: Vec<ObjectId>,
    ) -> Result<SnapshotResult, RepositoryError> {
        let capture = self.capture_working_tree(materializer).await?;
        self.store_snapshot(capture, request, parents).await
    }

    async fn capture_working_tree(
        &self,
        materializer: &dyn Materializer,
    ) -> Result<CaptureResult, RepositoryError> {
        let working_directory = WorkingDirectory::new(&self.root);
        materializer
            .capture_tree(&working_directory, &self.object_store)
            .await
            .map_err(RepositoryError::from)
    }

    async fn store_snapshot(
        &self,
        capture: CaptureResult,
        request: SnapshotRequest,
        parents: Vec<ObjectId>,
    ) -> Result<SnapshotResult, RepositoryError> {
        let timestamp_millis = request.resolve_timestamp()?;
        let snapshot = Snapshot::new(
            capture.root_tree_id,
            parents,
            timestamp_millis,
            request.author,
            request.message,
            request.provenance,
        );
        let snapshot_id = self.object_store.put_snapshot(&snapshot).await?;

        Ok(SnapshotResult {
            snapshot_id,
            snapshot,
            capture,
        })
    }
}

/// Why an automatic snapshot was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutoSnapshotTrigger {
    /// Snapshot created before switching context or restoring history.
    Safety,
    /// Snapshot created from a filesystem watch event.
    Watch,
    /// Snapshot created from a full reconciliation pass.
    Reconcile,
}

impl AutoSnapshotTrigger {
    /// Returns the stable provenance value for this trigger.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safety => "safety",
            Self::Watch => "watch",
            Self::Reconcile => "reconcile",
        }
    }
}

/// Metadata used when creating a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRequest {
    timestamp_millis: Option<u64>,
    author: Option<String>,
    message: Option<String>,
    provenance: SnapshotProvenance,
}

impl SnapshotRequest {
    /// Creates request metadata for a repository initialization snapshot.
    #[must_use]
    pub fn initial() -> Self {
        Self {
            timestamp_millis: None,
            author: None,
            message: None,
            provenance: SnapshotProvenance::initial(),
        }
    }

    /// Creates request metadata for a manually requested snapshot.
    #[must_use]
    pub fn manual(message: impl Into<String>) -> Self {
        Self {
            timestamp_millis: None,
            author: None,
            message: Some(message.into()),
            provenance: SnapshotProvenance::manual(),
        }
    }

    /// Creates request metadata for an automatic safety snapshot.
    #[must_use]
    pub fn automatic() -> Self {
        Self::automatic_for_trigger(AutoSnapshotTrigger::Safety)
    }

    /// Creates request metadata for an automatic snapshot with structured trigger metadata.
    #[must_use]
    pub fn automatic_for_trigger(trigger: AutoSnapshotTrigger) -> Self {
        Self {
            timestamp_millis: None,
            author: None,
            message: None,
            provenance: SnapshotProvenance::automatic()
                .with_attribute("trigger", trigger.as_str())
                .with_attribute("workspace", DEFAULT_WORKSPACE_ID),
        }
    }

    /// Sets a deterministic timestamp in milliseconds since the Unix epoch.
    #[must_use]
    pub fn with_timestamp_millis(mut self, timestamp_millis: u64) -> Self {
        self.timestamp_millis = Some(timestamp_millis);
        self
    }

    /// Sets the snapshot author.
    #[must_use]
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Sets structured provenance for the snapshot.
    #[must_use]
    pub fn with_provenance(mut self, provenance: SnapshotProvenance) -> Self {
        self.provenance = provenance;
        self
    }

    /// Adds or replaces a structured provenance attribute.
    #[must_use]
    pub fn with_provenance_attribute(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.provenance = self.provenance.with_attribute(key, value);
        self
    }

    fn resolve_timestamp(&self) -> Result<u64, RepositoryError> {
        match self.timestamp_millis {
            Some(timestamp_millis) => Ok(timestamp_millis),
            None => {
                let millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|source| RepositoryError::ClockBeforeUnixEpoch { source })?
                    .as_millis();
                u64::try_from(millis).map_err(|_| RepositoryError::TimestampOverflow { millis })
            }
        }
    }
}

/// Result returned when initializing a repository.
#[derive(Debug, Clone)]
pub struct InitResult {
    /// Open repository handle.
    pub repository: Repository,
    /// Initial snapshot result.
    pub snapshot: SnapshotResult,
}

/// Result returned when creating a snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotResult {
    /// Stored snapshot object ID.
    pub snapshot_id: ObjectId,
    /// Stored snapshot contents.
    pub snapshot: Snapshot,
    /// Working-directory capture result used by this snapshot.
    pub capture: CaptureResult,
}

/// Status of the working tree relative to the current branch snapshot.
#[derive(Debug, Clone)]
pub struct WorkingTreeStatus {
    /// Current branch snapshot ID.
    pub snapshot_id: ObjectId,
    /// Current branch snapshot contents.
    pub snapshot: Snapshot,
    /// Root tree ID computed from the current working directory.
    pub current_root_tree_id: ObjectId,
    /// Whether the working directory root tree matches the saved snapshot root tree.
    pub clean: bool,
    /// Read-only comparison result used for the status.
    pub comparison: TreeComparisonResult,
}

impl WorkingTreeStatus {
    /// Returns `true` when the working directory matches the current snapshot.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.clean
    }

    /// Returns path-level changes from the current snapshot to the working directory.
    #[must_use]
    pub fn changes(&self) -> &[TreeChange] {
        &self.comparison.changes
    }
}

/// A local branch and the snapshot it points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchHead {
    /// Branch name.
    pub name: BranchName,
    /// Snapshot ID stored in the branch ref.
    pub snapshot_id: ObjectId,
    /// Whether this branch is currently checked out.
    pub is_current: bool,
}

/// Result returned when creating a branch.
#[derive(Debug, Clone)]
pub struct BranchOperationResult {
    /// Branch name that was created.
    pub branch: BranchName,
    /// Snapshot ID the new branch points at.
    pub snapshot_id: ObjectId,
    /// Safety snapshot created before the operation when the working tree was dirty.
    pub saved_snapshot: Option<SnapshotResult>,
}

/// Result returned when switching branches.
#[derive(Debug, Clone)]
pub struct SwitchResult {
    /// Branch switched to.
    pub branch: BranchName,
    /// Snapshot ID materialized for the branch.
    pub snapshot_id: ObjectId,
    /// Snapshot materialized for the branch.
    pub snapshot: Snapshot,
    /// Safety snapshot created before the operation when the working tree was dirty.
    pub saved_snapshot: Option<SnapshotResult>,
    /// Filesystem materialization result.
    pub materialization: MaterializeResult,
}

/// Result returned when restoring a snapshot target.
#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// Snapshot ID materialized into the working directory.
    pub snapshot_id: ObjectId,
    /// Snapshot materialized into the working directory.
    pub snapshot: Snapshot,
    /// Safety snapshot created before the operation when the working tree was dirty.
    pub saved_snapshot: Option<SnapshotResult>,
    /// Filesystem materialization result.
    pub materialization: MaterializeResult,
}

/// A resolved snapshot target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSnapshot {
    /// Resolved snapshot ID.
    pub snapshot_id: ObjectId,
    /// Resolved snapshot contents.
    pub snapshot: Snapshot,
}

/// A timeline entry loaded from snapshot history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEntry {
    /// Snapshot object ID.
    pub snapshot_id: ObjectId,
    /// Snapshot contents.
    pub snapshot: Snapshot,
}

async fn ensure_working_root(root: &Path) -> Result<(), RepositoryError> {
    let metadata = fs::symlink_metadata(root)
        .await
        .map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => RepositoryError::RootMissing {
                path: root.to_path_buf(),
            },
            _ => io_error(root.to_path_buf(), source),
        })?;

    if !metadata.file_type().is_dir() {
        return Err(RepositoryError::RootNotDirectory {
            path: root.to_path_buf(),
        });
    }

    Ok(())
}

async fn ensure_metadata_dir(metadata_dir: &Path) -> Result<(), RepositoryError> {
    let metadata =
        fs::symlink_metadata(metadata_dir)
            .await
            .map_err(|source| match source.kind() {
                io::ErrorKind::NotFound => RepositoryError::NotRepository {
                    path: metadata_dir.to_path_buf(),
                },
                _ => io_error(metadata_dir.to_path_buf(), source),
            })?;

    if !metadata.file_type().is_dir() {
        return Err(RepositoryError::MetadataNotDirectory {
            path: metadata_dir.to_path_buf(),
        });
    }

    Ok(())
}

async fn path_exists(path: &Path) -> Result<bool, RepositoryError> {
    fs::try_exists(path)
        .await
        .map_err(|source| io_error(path.to_path_buf(), source))
}
