use crate::{
    BranchName, RepositoryError, WorkspaceId,
    error::io_error,
    refs::{
        acquire_metadata_lock, branch_lock_path, branch_ref_exists, branch_ref_path,
        create_branch_ref, create_ref_layout, create_workspace_ref, head_path, list_branch_refs,
        list_workspace_paths, list_workspace_refs, metadata_dir, objects_dir, read_branch_ref,
        read_head_branch, read_workspace_path, read_workspace_pointer, read_workspace_ref,
        workspace_pointer_path, workspace_record_exists, workspace_record_lock_path,
        workspace_ref_exists, workspace_ref_lock_path, workspace_ref_path, write_branch_ref,
        write_head, write_workspace_path, write_workspace_pointer, write_workspace_ref,
    },
    workspace::{DEFAULT_WORKSPACE_ID, WorkspacePointer},
};
use era_core::{ObjectId, ObjectKind, Snapshot, SnapshotProvenance};
use era_materialization::{
    CaptureResult, MaterializeResult, Materializer, TreeChange, TreeComparisonResult,
    WorkingDirectory,
};
use era_object_store::LocalObjectStore;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::fs;
use tracing::{debug, trace};

/// A local Era repository opened in the context of one materialized workspace.
#[derive(Debug, Clone)]
pub struct Repository {
    root: PathBuf,
    metadata_dir: PathBuf,
    object_store: LocalObjectStore,
    cursor: RepositoryCursor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RepositoryCursor {
    CurrentBranch,
    Workspace(WorkspaceId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CursorReference {
    Branch(BranchName),
    Workspace(WorkspaceId),
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
            cursor: RepositoryCursor::CurrentBranch,
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

    /// Opens an existing repository or connected external workspace at `root`.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, RepositoryError> {
        let root = root.into();
        debug!(root = %root.display(), "opening repository workspace");
        ensure_working_root(&root).await?;

        let era_marker = metadata_dir(&root);
        let metadata =
            fs::symlink_metadata(&era_marker)
                .await
                .map_err(|source| match source.kind() {
                    io::ErrorKind::NotFound => RepositoryError::NotRepository {
                        path: era_marker.clone(),
                    },
                    _ => io_error(era_marker.clone(), source),
                })?;

        if metadata.file_type().is_dir() {
            return Self::open_metadata_dir(root, era_marker, RepositoryCursor::CurrentBranch)
                .await;
        }

        if metadata.file_type().is_file() {
            let pointer = read_workspace_pointer(&root).await?;
            ensure_metadata_dir(&pointer.metadata_dir).await?;
            read_head_branch(&pointer.metadata_dir).await?;
            if !workspace_ref_exists(&pointer.metadata_dir, &pointer.workspace_id).await? {
                return Err(RepositoryError::WorkspaceNotFound {
                    id: pointer.workspace_id,
                });
            }
            return Self::open_metadata_dir(
                root,
                pointer.metadata_dir,
                RepositoryCursor::Workspace(pointer.workspace_id),
            )
            .await;
        }

        Err(RepositoryError::NotRepository { path: era_marker })
    }

    /// Opens a repository argument that may point at a repo root, `.era`, `.era/objects`, or workspace.
    pub async fn open_repository_path(path: impl Into<PathBuf>) -> Result<Self, RepositoryError> {
        let path = path.into();
        if let Some(metadata) = metadata_argument(&path).await? {
            let root = metadata
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| path.clone());
            return Self::open_metadata_dir(root, metadata, RepositoryCursor::CurrentBranch).await;
        }

        Self::open(path).await
    }

    /// Ensures `workspace_path` is connected to `repo_path`, then opens it as a workspace.
    pub async fn open_or_add_workspace(
        repo_path: impl Into<PathBuf>,
        workspace_path: impl Into<PathBuf>,
        workspace_id: WorkspaceId,
        materializer: &dyn Materializer,
    ) -> Result<Self, RepositoryError> {
        let repo = Self::open_repository_path(repo_path).await?;
        let path = workspace_path.into();
        repo.add_workspace(
            materializer,
            AddWorkspaceOptions {
                path: path.clone(),
                workspace_id,
                from: None,
            },
        )
        .await?;
        Self::open(path).await
    }

    async fn open_metadata_dir(
        root: PathBuf,
        metadata_dir: PathBuf,
        cursor: RepositoryCursor,
    ) -> Result<Self, RepositoryError> {
        ensure_metadata_dir(&metadata_dir).await?;
        read_head_branch(&metadata_dir).await?;
        let object_store = LocalObjectStore::open(objects_dir(&metadata_dir)).await?;

        Ok(Self {
            root,
            metadata_dir,
            object_store,
            cursor,
        })
    }

    /// Returns the materialized workspace root path for this repository handle.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the `.era` metadata directory path for the shared repository.
    #[must_use]
    pub fn metadata_dir(&self) -> &Path {
        &self.metadata_dir
    }

    /// Returns the local object store for this repository.
    #[must_use]
    pub fn object_store(&self) -> &LocalObjectStore {
        &self.object_store
    }

    /// Returns information about the mutable cursor this handle operates on.
    pub async fn cursor_info(&self) -> Result<CursorInfo, RepositoryError> {
        match self.current_cursor_reference().await? {
            CursorReference::Branch(branch) => Ok(CursorInfo::Branch(branch)),
            CursorReference::Workspace(workspace) => Ok(CursorInfo::Workspace(workspace)),
        }
    }

    /// Returns the connected workspace ID, if this handle is opened from an external workspace.
    #[must_use]
    pub fn workspace_id(&self) -> Option<&WorkspaceId> {
        match &self.cursor {
            RepositoryCursor::CurrentBranch => None,
            RepositoryCursor::Workspace(workspace) => Some(workspace),
        }
    }

    /// Captures a manual snapshot and advances the current cursor.
    pub async fn snapshot(
        &self,
        materializer: &dyn Materializer,
        request: SnapshotRequest,
    ) -> Result<SnapshotResult, RepositoryError> {
        let capture = self.capture_working_tree(materializer).await?;
        let cursor = self.current_cursor_reference().await?;
        let _lock = self.acquire_cursor_lock(&cursor).await?;
        let parent = self.read_cursor_ref(&cursor).await?;
        debug!(cursor = cursor.as_log_value(), parent = %parent, "creating manual snapshot");

        let snapshot = self.store_snapshot(capture, request, vec![parent]).await?;
        self.write_cursor_ref(&cursor, snapshot.snapshot_id).await?;

        debug!(cursor = cursor.as_log_value(), %snapshot.snapshot_id, "advanced cursor ref");
        Ok(snapshot)
    }

    /// Captures and advances the current cursor only when the working tree changed.
    pub async fn snapshot_if_changed(
        &self,
        materializer: &dyn Materializer,
        request: SnapshotRequest,
    ) -> Result<Option<SnapshotResult>, RepositoryError> {
        let capture = self.capture_working_tree(materializer).await?;
        let cursor = self.current_cursor_reference().await?;
        let _lock = self.acquire_cursor_lock(&cursor).await?;
        let parent = self.read_cursor_ref(&cursor).await?;
        let parent_snapshot = self.object_store.get_snapshot(&parent).await?;
        if capture.root_tree_id == parent_snapshot.root_tree_id() {
            debug!(cursor = cursor.as_log_value(), parent = %parent, "working tree unchanged; skipping snapshot");
            return Ok(None);
        }

        let snapshot = self.store_snapshot(capture, request, vec![parent]).await?;
        self.write_cursor_ref(&cursor, snapshot.snapshot_id).await?;

        debug!(cursor = cursor.as_log_value(), %snapshot.snapshot_id, "advanced cursor ref after change capture");
        Ok(Some(snapshot))
    }

    /// Returns whether the working directory matches the current cursor snapshot.
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

    /// Lists registered workspaces sorted by ID.
    pub async fn workspaces(&self) -> Result<Vec<WorkspaceHead>, RepositoryError> {
        let paths: HashMap<WorkspaceId, Option<PathBuf>> = list_workspace_paths(&self.metadata_dir)
            .await?
            .into_iter()
            .collect();
        let current_workspace = self.workspace_id().cloned();
        let mut workspaces = Vec::new();

        for (id, snapshot_id) in list_workspace_refs(&self.metadata_dir).await? {
            let path = paths.get(&id).cloned().flatten();
            let is_current = current_workspace.as_ref() == Some(&id);
            workspaces.push(WorkspaceHead {
                id,
                snapshot_id,
                path,
                is_current,
            });
        }

        Ok(workspaces)
    }

    /// Creates or adopts a workspace connected to this repository's object store and refs.
    pub async fn add_workspace(
        &self,
        materializer: &dyn Materializer,
        options: AddWorkspaceOptions,
    ) -> Result<WorkspaceAddResult, RepositoryError> {
        let requested_path = options.path;
        let workspace_id = options.workspace_id;
        let target_path = absolute_path_for_create(&requested_path).await?;
        let source_root = absolute_existing_path(&self.root).await?;

        if target_path == source_root || workspace_pointer_path(&target_path).is_dir() {
            return Err(RepositoryError::WorkspacePathIsRepository { path: target_path });
        }
        if target_path.starts_with(&source_root) {
            return Err(RepositoryError::WorkspaceInsideWorkspace {
                path: target_path,
                workspace_root: source_root,
            });
        }
        for (existing_id, existing_path) in list_workspace_paths(&self.metadata_dir).await? {
            let Some(existing_path) = existing_path else {
                continue;
            };
            let existing_root = if path_exists(&existing_path).await? {
                absolute_existing_path(&existing_path).await?
            } else {
                existing_path
            };
            if target_path == existing_root && existing_id != workspace_id {
                return Err(RepositoryError::WorkspaceAlreadyExists {
                    id: existing_id,
                    existing_path: existing_root,
                    requested_path: target_path,
                });
            }
            if target_path != existing_root && target_path.starts_with(&existing_root) {
                return Err(RepositoryError::WorkspaceInsideWorkspace {
                    path: target_path,
                    workspace_root: existing_root,
                });
            }
        }

        let existed = path_exists(&target_path).await?;
        if existed {
            ensure_working_root(&target_path).await?;
        } else {
            fs::create_dir_all(&target_path)
                .await
                .map_err(|source| io_error(target_path.clone(), source))?;
        }
        let was_empty = directory_is_empty(&target_path).await?;

        let base = match options.from {
            Some(target) => self.resolve_snapshot_target(&target).await?.snapshot_id,
            None => {
                self.save_current_state_if_changed(materializer).await?;
                self.current_snapshot_id().await?
            }
        };

        let lock_path = workspace_record_lock_path(&self.metadata_dir, &workspace_id);
        let _lock = acquire_metadata_lock(lock_path).await?;

        let already_registered = workspace_record_exists(&self.metadata_dir, &workspace_id).await?;
        let ref_exists = workspace_ref_exists(&self.metadata_dir, &workspace_id).await?;
        if already_registered {
            let existing_path = read_workspace_path(&self.metadata_dir, &workspace_id).await?;
            if existing_path != target_path {
                return Err(RepositoryError::WorkspaceAlreadyExists {
                    id: workspace_id,
                    existing_path,
                    requested_path: target_path,
                });
            }
        }

        if !ref_exists {
            create_workspace_ref(&self.metadata_dir, &workspace_id, base).await?;
        }
        if !already_registered {
            write_workspace_path(&self.metadata_dir, &workspace_id, &target_path).await?;
        }

        let pointer = WorkspacePointer::new(
            absolute_path_preserving_symlinks(&self.metadata_dir)?,
            workspace_id.clone(),
        );
        write_workspace_pointer(&target_path, &pointer).await?;

        let materialized = (!existed || was_empty) && !already_registered;
        let materialization = if materialized {
            let snapshot_id = read_workspace_ref(&self.metadata_dir, &workspace_id).await?;
            let snapshot = self.object_store.get_snapshot(&snapshot_id).await?;
            let working_directory = WorkingDirectory::new(&target_path);
            Some(
                materializer
                    .materialize_tree(
                        snapshot.root_tree_id(),
                        &working_directory,
                        &self.object_store,
                    )
                    .await?,
            )
        } else {
            None
        };

        let snapshot_id = read_workspace_ref(&self.metadata_dir, &workspace_id).await?;
        Ok(WorkspaceAddResult {
            workspace_id,
            path: target_path,
            snapshot_id,
            created: !already_registered,
            materialized,
            materialization,
        })
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

    /// Switches this repository handle to an existing branch, saving unsnapped work first.
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

        match &self.cursor {
            RepositoryCursor::CurrentBranch => write_head(&self.metadata_dir, &name).await?,
            RepositoryCursor::Workspace(workspace) => {
                let cursor = CursorReference::Workspace(workspace.clone());
                let _lock = self.acquire_cursor_lock(&cursor).await?;
                self.write_cursor_ref(&cursor, snapshot_id).await?;
            }
        }

        debug!(branch = name.as_str(), %snapshot_id, "switched branch");
        Ok(SwitchResult {
            branch: name,
            snapshot_id,
            snapshot,
            saved_snapshot,
            materialization,
        })
    }

    /// Restores a snapshot target into the working directory without moving the current cursor.
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

    /// Resolves a snapshot target from a full ID, branch/workspace ref, unique prefix, or exact message.
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

        if let Ok(branch) = BranchName::new(target)
            && branch_ref_exists(&self.metadata_dir, &branch).await?
        {
            let snapshot_id = read_branch_ref(&self.metadata_dir, &branch).await?;
            let snapshot = self.object_store.get_snapshot(&snapshot_id).await?;
            return Ok(ResolvedSnapshot {
                snapshot_id,
                snapshot,
            });
        }

        if let Ok(workspace) = WorkspaceId::new(target)
            && workspace_ref_exists(&self.metadata_dir, &workspace).await?
        {
            let snapshot_id = read_workspace_ref(&self.metadata_dir, &workspace).await?;
            let snapshot = self.object_store.get_snapshot(&snapshot_id).await?;
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

    /// Returns the current snapshot ID for this handle's cursor.
    pub async fn current_snapshot_id(&self) -> Result<ObjectId, RepositoryError> {
        let cursor = self.current_cursor_reference().await?;
        self.read_cursor_ref(&cursor).await
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

    /// Returns the path used for this handle's current cursor ref file.
    pub async fn current_cursor_ref_path(&self) -> Result<PathBuf, RepositoryError> {
        let cursor = self.current_cursor_reference().await?;
        Ok(match cursor {
            CursorReference::Branch(branch) => branch_ref_path(&self.metadata_dir, &branch),
            CursorReference::Workspace(workspace) => {
                workspace_ref_path(&self.metadata_dir, &workspace)
            }
        })
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
        let mut request = SnapshotRequest::automatic();
        if let Some(workspace) = self.workspace_id() {
            request = request.with_provenance_attribute("workspace", workspace.as_str());
        }
        self.snapshot_if_changed(materializer, request).await
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

    async fn current_cursor_reference(&self) -> Result<CursorReference, RepositoryError> {
        match &self.cursor {
            RepositoryCursor::CurrentBranch => {
                Ok(CursorReference::Branch(self.current_branch().await?))
            }
            RepositoryCursor::Workspace(workspace) => {
                Ok(CursorReference::Workspace(workspace.clone()))
            }
        }
    }

    async fn acquire_cursor_lock(
        &self,
        cursor: &CursorReference,
    ) -> Result<crate::refs::MetadataLock, RepositoryError> {
        let path = match cursor {
            CursorReference::Branch(branch) => branch_lock_path(&self.metadata_dir, branch),
            CursorReference::Workspace(workspace) => {
                workspace_ref_lock_path(&self.metadata_dir, workspace)
            }
        };
        acquire_metadata_lock(path).await
    }

    async fn read_cursor_ref(&self, cursor: &CursorReference) -> Result<ObjectId, RepositoryError> {
        match cursor {
            CursorReference::Branch(branch) => read_branch_ref(&self.metadata_dir, branch).await,
            CursorReference::Workspace(workspace) => {
                read_workspace_ref(&self.metadata_dir, workspace).await
            }
        }
    }

    async fn write_cursor_ref(
        &self,
        cursor: &CursorReference,
        snapshot_id: ObjectId,
    ) -> Result<(), RepositoryError> {
        match cursor {
            CursorReference::Branch(branch) => {
                write_branch_ref(&self.metadata_dir, branch, snapshot_id).await
            }
            CursorReference::Workspace(workspace) => {
                write_workspace_ref(&self.metadata_dir, workspace, snapshot_id).await
            }
        }
    }
}

impl CursorReference {
    fn as_log_value(&self) -> String {
        match self {
            Self::Branch(branch) => format!("branch:{}", branch.as_str()),
            Self::Workspace(workspace) => format!("workspace:{}", workspace.as_str()),
        }
    }
}

/// Information about the mutable cursor a repository handle operates on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorInfo {
    /// The repository root uses the branch named by HEAD.
    Branch(BranchName),
    /// An external workspace uses its own workspace cursor.
    Workspace(WorkspaceId),
}

impl CursorInfo {
    /// Returns the user-facing cursor kind.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Branch(_) => "Branch",
            Self::Workspace(_) => "Workspace",
        }
    }

    /// Returns the user-facing cursor name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Branch(branch) => branch.as_str(),
            Self::Workspace(workspace) => workspace.as_str(),
        }
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

    /// Creates request metadata for a manually requested labeled snapshot.
    #[must_use]
    pub fn manual(message: impl Into<String>) -> Self {
        Self {
            timestamp_millis: None,
            author: None,
            message: Some(message.into()),
            provenance: SnapshotProvenance::manual(),
        }
    }

    /// Creates request metadata for a manually requested unlabeled snapshot.
    #[must_use]
    pub fn manual_unlabeled() -> Self {
        Self {
            timestamp_millis: None,
            author: None,
            message: None,
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

/// Options for creating or adopting a connected workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddWorkspaceOptions {
    /// Directory that should become a connected workspace.
    pub path: PathBuf,
    /// Workspace ID to register.
    pub workspace_id: WorkspaceId,
    /// Optional base snapshot/branch/workspace/label target. Defaults to the current saved state.
    pub from: Option<String>,
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

/// Status of the working tree relative to the current cursor snapshot.
#[derive(Debug, Clone)]
pub struct WorkingTreeStatus {
    /// Current cursor snapshot ID.
    pub snapshot_id: ObjectId,
    /// Current cursor snapshot contents.
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

/// A connected workspace and the snapshot it points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceHead {
    /// Workspace ID.
    pub id: WorkspaceId,
    /// Snapshot ID stored in the workspace ref.
    pub snapshot_id: ObjectId,
    /// Registered workspace path.
    pub path: Option<PathBuf>,
    /// Whether this repository handle is opened in this workspace.
    pub is_current: bool,
}

/// Result returned when adding or adopting a workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceAddResult {
    /// Workspace ID that was registered.
    pub workspace_id: WorkspaceId,
    /// Workspace path that was connected.
    pub path: PathBuf,
    /// Snapshot ID the workspace cursor points at.
    pub snapshot_id: ObjectId,
    /// Whether a new workspace registry record was created.
    pub created: bool,
    /// Whether Era materialized the base snapshot into the workspace.
    pub materialized: bool,
    /// Filesystem materialization result when a missing/empty workspace was populated.
    pub materialization: Option<MaterializeResult>,
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

async fn directory_is_empty(path: &Path) -> Result<bool, RepositoryError> {
    let mut reader = fs::read_dir(path)
        .await
        .map_err(|source| io_error(path.to_path_buf(), source))?;
    Ok(reader
        .next_entry()
        .await
        .map_err(|source| io_error(path.to_path_buf(), source))?
        .is_none())
}

async fn absolute_existing_path(path: &Path) -> Result<PathBuf, RepositoryError> {
    fs::canonicalize(path)
        .await
        .map_err(|source| io_error(path.to_path_buf(), source))
}

async fn absolute_path_for_create(path: &Path) -> Result<PathBuf, RepositoryError> {
    if path_exists(path).await? {
        return absolute_existing_path(path).await;
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let parent = absolute_existing_path(parent).await?;
    let name = path
        .file_name()
        .ok_or_else(|| RepositoryError::RootMissing {
            path: path.to_path_buf(),
        })?;
    Ok(parent.join(name))
}

fn absolute_path_preserving_symlinks(path: &Path) -> Result<PathBuf, RepositoryError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|source| io_error(path.to_path_buf(), source))
}

async fn metadata_argument(path: &Path) -> Result<Option<PathBuf>, RepositoryError> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error(path.to_path_buf(), source)),
    };
    if !metadata.file_type().is_dir() {
        return Ok(None);
    }

    if path.file_name().and_then(|name| name.to_str()) == Some("objects")
        && let Some(parent) = path.parent()
        && parent.file_name().and_then(|name| name.to_str()) == Some(".era")
    {
        return Ok(Some(parent.to_path_buf()));
    }

    if path.file_name().and_then(|name| name.to_str()) == Some(".era") {
        return Ok(Some(path.to_path_buf()));
    }

    Ok(None)
}
