use crate::{
    BranchName, RepositoryError,
    error::io_error,
    refs::{
        branch_ref_path, create_ref_layout, head_path, metadata_dir, objects_dir, read_branch_ref,
        read_head_branch, write_branch_ref, write_head,
    },
};
use era_core::{ObjectId, Snapshot, SnapshotProvenance};
use era_materialization::{CaptureResult, Materializer, WorkingDirectory};
use era_object_store::LocalObjectStore;
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::fs;
use tracing::{debug, trace};

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

    async fn capture_snapshot(
        &self,
        materializer: &dyn Materializer,
        request: SnapshotRequest,
        parents: Vec<ObjectId>,
    ) -> Result<SnapshotResult, RepositoryError> {
        let timestamp_millis = request.resolve_timestamp()?;
        let working_directory = WorkingDirectory::new(&self.root);
        let capture = materializer
            .capture_tree(&working_directory, &self.object_store)
            .await?;
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
