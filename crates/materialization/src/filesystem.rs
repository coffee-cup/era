use crate::{
    CaptureIssue, CaptureIssueKind, CaptureOptions, CaptureResult, CaptureStats,
    MaterializationError, MaterializeResult, MaterializeStats, Materializer, SymlinkPolicy,
    TreeChange, TreeComparisonResult, TreeScanResult, TreeScanStats, WorkingDirectory,
    WorkingDirectoryWatch,
    hash_cache::{FileFingerprint, HashCache},
};
use async_trait::async_trait;
use era_core::{EntryKind, ObjectId, Tree, TreeEntry};
use era_object_store::ObjectStore;
use std::{
    cmp::Ordering,
    collections::BTreeSet,
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::fs;
use tracing::{debug, trace};

/// Copy-based filesystem materializer for ordinary working directories.
#[derive(Debug, Clone, Default)]
pub struct FilesystemMaterializer {
    options: CaptureOptions,
    hash_cache: Arc<Mutex<HashCache>>,
}

impl FilesystemMaterializer {
    /// Creates a materializer with default capture options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a materializer with explicit capture options.
    #[must_use]
    pub fn with_options(options: CaptureOptions) -> Self {
        Self {
            options,
            hash_cache: Arc::new(Mutex::new(HashCache::default())),
        }
    }

    /// Returns this materializer's capture options.
    #[must_use]
    pub fn options(&self) -> &CaptureOptions {
        &self.options
    }

    /// Invalidates cached hashes for changed relative paths.
    pub fn invalidate_paths<I, P>(&self, paths: I)
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut cache = self.hash_cache.lock().expect("hash cache mutex poisoned");
        for path in paths {
            cache.invalidate_path(path.as_ref());
        }
    }

    /// Captures the current working directory into blob and tree objects.
    pub async fn capture_tree(
        &self,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<CaptureResult, MaterializationError> {
        let root = working_directory.root();
        debug!(root = %root.display(), "capturing working directory");
        ensure_capture_root(root).await?;

        let mut stats = CaptureStats::default();
        let mut issues = Vec::new();
        let root_tree_id = self
            .capture_directory(
                root.to_path_buf(),
                PathBuf::new(),
                object_store,
                &mut stats,
                &mut issues,
            )
            .await?;

        debug!(
            root = %root.display(),
            %root_tree_id,
            files = stats.files_seen,
            directories = stats.directories_seen,
            bytes = stats.bytes_read,
            ignored = stats.ignored_entries,
            symlinks_skipped = stats.symlinks_skipped,
            "captured working directory"
        );

        Ok(CaptureResult::new(root_tree_id, stats, issues))
    }

    /// Scans the current working directory without storing objects.
    pub async fn scan_tree(
        &self,
        working_directory: &WorkingDirectory,
    ) -> Result<TreeScanResult, MaterializationError> {
        let root = working_directory.root();
        debug!(root = %root.display(), "scanning working directory");
        ensure_capture_root(root).await?;

        let mut stats = TreeScanStats::default();
        let mut issues = Vec::new();
        let root_tree_id = self
            .scan_directory(root.to_path_buf(), PathBuf::new(), &mut stats, &mut issues)
            .await?;

        debug!(
            root = %root.display(),
            %root_tree_id,
            files = stats.files_seen,
            directories = stats.directories_seen,
            bytes = stats.bytes_read,
            ignored = stats.ignored_entries,
            symlinks_skipped = stats.symlinks_skipped,
            "scanned working directory"
        );

        Ok(TreeScanResult::new(root_tree_id, stats, issues))
    }

    /// Compares the current working directory with a stored tree.
    pub async fn compare_tree(
        &self,
        saved_root_tree_id: ObjectId,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<TreeComparisonResult, MaterializationError> {
        let root = working_directory.root();
        debug!(root = %root.display(), %saved_root_tree_id, "comparing working directory");
        ensure_capture_root(root).await?;

        let mut state = ComparisonState::default();
        let current_root_tree_id = self
            .compare_directory(
                saved_root_tree_id,
                root.to_path_buf(),
                PathBuf::new(),
                object_store,
                &mut state,
            )
            .await?;

        debug!(
            root = %root.display(),
            %saved_root_tree_id,
            %current_root_tree_id,
            changes = state.changes.len(),
            files = state.stats.files_seen,
            directories = state.stats.directories_seen,
            bytes = state.stats.bytes_read,
            ignored = state.stats.ignored_entries,
            symlinks_skipped = state.stats.symlinks_skipped,
            "compared working directory"
        );

        Ok(TreeComparisonResult::new(
            saved_root_tree_id,
            current_root_tree_id,
            state.changes,
            state.stats,
            state.issues,
        ))
    }

    /// Watches the working directory and emits filesystem change hints.
    pub async fn watch(
        &self,
        working_directory: &WorkingDirectory,
    ) -> Result<WorkingDirectoryWatch, MaterializationError> {
        let root = working_directory.root();
        debug!(root = %root.display(), "watching working directory");
        ensure_capture_root(root).await?;
        WorkingDirectoryWatch::new(working_directory, self.options.clone())
    }

    /// Reconciles the working directory to match a stored tree.
    pub async fn materialize_tree(
        &self,
        root_tree_id: ObjectId,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<MaterializeResult, MaterializationError> {
        let root = working_directory.root();
        debug!(root = %root.display(), %root_tree_id, "materializing working directory");
        ensure_capture_root(root).await?;

        self.invalidate_paths([PathBuf::new()]);
        let mut stats = MaterializeStats::default();
        self.materialize_directory(root_tree_id, root.to_path_buf(), object_store, &mut stats)
            .await?;

        debug!(
            root = %root.display(),
            %root_tree_id,
            files_written = stats.files_written,
            directories_created = stats.directories_created,
            entries_removed = stats.entries_removed,
            bytes_written = stats.bytes_written,
            "materialized working directory"
        );

        Ok(MaterializeResult::new(stats))
    }

    fn capture_directory<'a>(
        &'a self,
        directory_path: PathBuf,
        relative_path: PathBuf,
        object_store: &'a dyn ObjectStore,
        stats: &'a mut CaptureStats,
        issues: &'a mut Vec<CaptureIssue>,
    ) -> Pin<Box<dyn Future<Output = Result<ObjectId, MaterializationError>> + Send + 'a>> {
        Box::pin(async move {
            trace!(path = %directory_path.display(), "capturing directory");
            stats.directories_seen += 1;

            let directory_entries = read_directory_entries(&directory_path, &relative_path).await?;
            let mut tree_entries = Vec::new();

            for entry in directory_entries {
                if entry.file_type.is_dir() {
                    if self.options.excludes_directory_name(&entry.name) {
                        trace!(path = %entry.path.display(), "skipping excluded directory");
                        stats.ignored_entries += 1;
                        continue;
                    }

                    let child_tree_id = self
                        .capture_directory(
                            entry.path.clone(),
                            entry.relative_path.clone(),
                            object_store,
                            stats,
                            issues,
                        )
                        .await?;
                    tree_entries.push(TreeEntry::tree(entry.name, child_tree_id).map_err(
                        |source| MaterializationError::InvalidTreeEntry {
                            path: entry.path,
                            source,
                        },
                    )?);
                } else if entry.file_type.is_file() {
                    let id = self
                        .capture_current_file(&entry, object_store, stats)
                        .await?;

                    tree_entries.push(TreeEntry::blob(entry.name, id).map_err(|source| {
                        MaterializationError::InvalidTreeEntry {
                            path: entry.path,
                            source,
                        }
                    })?);
                } else if entry.file_type.is_symlink() {
                    match self.options.symlink_policy() {
                        SymlinkPolicy::Skip => {
                            trace!(path = %entry.path.display(), "skipping symlink");
                            stats.symlinks_skipped += 1;
                            issues.push(CaptureIssue::new(
                                entry.relative_path,
                                CaptureIssueKind::SkippedSymlink,
                            ));
                        }
                        SymlinkPolicy::Error => {
                            return Err(MaterializationError::SymlinkUnsupported {
                                path: entry.path,
                            });
                        }
                    }
                } else {
                    return Err(MaterializationError::UnsupportedFileType { path: entry.path });
                }
            }

            let tree = Tree::new(tree_entries).map_err(|source| {
                MaterializationError::InvalidTreeEntry {
                    path: directory_path.clone(),
                    source,
                }
            })?;
            let tree_id = object_store.put_tree(&tree).await?;
            stats.trees_stored += 1;
            trace!(path = %directory_path.display(), %tree_id, entries = tree.entries().len(), "captured directory tree");
            Ok(tree_id)
        })
    }

    fn scan_directory<'a>(
        &'a self,
        directory_path: PathBuf,
        relative_path: PathBuf,
        stats: &'a mut TreeScanStats,
        issues: &'a mut Vec<CaptureIssue>,
    ) -> Pin<Box<dyn Future<Output = Result<ObjectId, MaterializationError>> + Send + 'a>> {
        Box::pin(async move {
            trace!(path = %directory_path.display(), "scanning directory");
            stats.directories_seen += 1;

            let directory_entries = read_directory_entries(&directory_path, &relative_path).await?;
            let mut tree_entries = Vec::new();

            for entry in directory_entries {
                if entry.file_type.is_dir() {
                    if self.options.excludes_directory_name(&entry.name) {
                        trace!(path = %entry.path.display(), "skipping excluded directory");
                        stats.ignored_entries += 1;
                        continue;
                    }

                    let child_tree_id = self
                        .scan_directory(
                            entry.path.clone(),
                            entry.relative_path.clone(),
                            stats,
                            issues,
                        )
                        .await?;
                    tree_entries.push(TreeEntry::tree(entry.name, child_tree_id).map_err(
                        |source| MaterializationError::InvalidTreeEntry {
                            path: entry.path,
                            source,
                        },
                    )?);
                } else if entry.file_type.is_file() {
                    let id = self.scan_current_file_id(&entry, stats).await?;

                    tree_entries.push(TreeEntry::blob(entry.name, id).map_err(|source| {
                        MaterializationError::InvalidTreeEntry {
                            path: entry.path,
                            source,
                        }
                    })?);
                } else if entry.file_type.is_symlink() {
                    match self.options.symlink_policy() {
                        SymlinkPolicy::Skip => {
                            trace!(path = %entry.path.display(), "skipping symlink");
                            stats.symlinks_skipped += 1;
                            issues.push(CaptureIssue::new(
                                entry.relative_path,
                                CaptureIssueKind::SkippedSymlink,
                            ));
                        }
                        SymlinkPolicy::Error => {
                            return Err(MaterializationError::SymlinkUnsupported {
                                path: entry.path,
                            });
                        }
                    }
                } else {
                    return Err(MaterializationError::UnsupportedFileType { path: entry.path });
                }
            }

            let tree = Tree::new(tree_entries).map_err(|source| {
                MaterializationError::InvalidTreeEntry {
                    path: directory_path.clone(),
                    source,
                }
            })?;
            let tree_id = tree.id();
            trace!(path = %directory_path.display(), %tree_id, entries = tree.entries().len(), "scanned directory tree");
            Ok(tree_id)
        })
    }

    fn compare_directory<'a>(
        &'a self,
        saved_tree_id: ObjectId,
        directory_path: PathBuf,
        relative_path: PathBuf,
        object_store: &'a dyn ObjectStore,
        state: &'a mut ComparisonState,
    ) -> Pin<Box<dyn Future<Output = Result<ObjectId, MaterializationError>> + Send + 'a>> {
        Box::pin(async move {
            trace!(path = %directory_path.display(), %saved_tree_id, "comparing directory");
            state.stats.directories_seen += 1;

            let saved_tree = object_store.get_tree(&saved_tree_id).await?;
            let current_entries = self
                .read_included_directory_entries(&directory_path, &relative_path, state)
                .await?;
            let saved_entries = saved_tree.entries();
            let mut current_tree_entries = Vec::new();
            let mut saved_index = 0;
            let mut current_index = 0;

            while saved_index < saved_entries.len() || current_index < current_entries.len() {
                let ordering = match (
                    saved_entries.get(saved_index),
                    current_entries.get(current_index),
                ) {
                    (Some(saved), Some(current)) => {
                        saved.name().as_bytes().cmp(current.name.as_bytes())
                    }
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => break,
                };

                match ordering {
                    Ordering::Less => {
                        let saved_entry = &saved_entries[saved_index];
                        state.changes.push(TreeChange::deleted(join_relative(
                            &relative_path,
                            saved_entry.name(),
                        )));
                        saved_index += 1;
                    }
                    Ordering::Greater => {
                        let current_entry = current_entries[current_index].clone();
                        current_tree_entries.push(
                            self.scan_current_entry(current_entry.clone(), state)
                                .await?,
                        );
                        state
                            .changes
                            .push(TreeChange::added(current_entry.relative_path));
                        current_index += 1;
                    }
                    Ordering::Equal => {
                        let saved_entry = saved_entries[saved_index].clone();
                        let current_entry = current_entries[current_index].clone();
                        let current_tree_entry = self
                            .compare_matching_entry(saved_entry, current_entry, object_store, state)
                            .await?;
                        current_tree_entries.push(current_tree_entry);
                        saved_index += 1;
                        current_index += 1;
                    }
                }
            }

            let current_tree = Tree::new(current_tree_entries).map_err(|source| {
                MaterializationError::InvalidTreeEntry {
                    path: directory_path.clone(),
                    source,
                }
            })?;
            let current_tree_id = current_tree.id();
            trace!(path = %directory_path.display(), %saved_tree_id, %current_tree_id, changes = state.changes.len(), "compared directory");
            Ok(current_tree_id)
        })
    }

    async fn read_included_directory_entries(
        &self,
        directory_path: &Path,
        relative_path: &Path,
        state: &mut ComparisonState,
    ) -> Result<Vec<DirectoryEntry>, MaterializationError> {
        let directory_entries = read_directory_entries(directory_path, relative_path).await?;
        let mut included_entries = Vec::new();

        for entry in directory_entries {
            if entry.file_type.is_dir() && self.options.excludes_directory_name(&entry.name) {
                trace!(path = %entry.path.display(), "skipping excluded directory");
                state.stats.ignored_entries += 1;
                continue;
            }

            if entry.file_type.is_symlink() {
                match self.options.symlink_policy() {
                    SymlinkPolicy::Skip => {
                        trace!(path = %entry.path.display(), "skipping symlink");
                        state.stats.symlinks_skipped += 1;
                        state.issues.push(CaptureIssue::new(
                            entry.relative_path,
                            CaptureIssueKind::SkippedSymlink,
                        ));
                        continue;
                    }
                    SymlinkPolicy::Error => {
                        return Err(MaterializationError::SymlinkUnsupported { path: entry.path });
                    }
                }
            }

            if !entry.file_type.is_dir() && !entry.file_type.is_file() {
                return Err(MaterializationError::UnsupportedFileType { path: entry.path });
            }

            included_entries.push(entry);
        }

        Ok(included_entries)
    }

    async fn scan_current_entry(
        &self,
        entry: DirectoryEntry,
        state: &mut ComparisonState,
    ) -> Result<TreeEntry, MaterializationError> {
        if entry.file_type.is_dir() {
            let id = self
                .scan_directory(
                    entry.path.clone(),
                    entry.relative_path.clone(),
                    &mut state.stats,
                    &mut state.issues,
                )
                .await?;
            TreeEntry::tree(entry.name, id).map_err(|source| {
                MaterializationError::InvalidTreeEntry {
                    path: entry.path,
                    source,
                }
            })
        } else if entry.file_type.is_file() {
            let id = self.scan_current_file_id(&entry, &mut state.stats).await?;
            TreeEntry::blob(entry.name, id).map_err(|source| {
                MaterializationError::InvalidTreeEntry {
                    path: entry.path,
                    source,
                }
            })
        } else if entry.file_type.is_symlink() {
            match self.options.symlink_policy() {
                SymlinkPolicy::Skip => {
                    Err(MaterializationError::UnsupportedFileType { path: entry.path })
                }
                SymlinkPolicy::Error => {
                    Err(MaterializationError::SymlinkUnsupported { path: entry.path })
                }
            }
        } else {
            Err(MaterializationError::UnsupportedFileType { path: entry.path })
        }
    }

    async fn compare_matching_entry(
        &self,
        saved_entry: TreeEntry,
        current_entry: DirectoryEntry,
        object_store: &dyn ObjectStore,
        state: &mut ComparisonState,
    ) -> Result<TreeEntry, MaterializationError> {
        match (saved_entry.kind(), current_entry.file_type.is_dir()) {
            (EntryKind::Blob, false) if current_entry.file_type.is_file() => {
                let id = self
                    .scan_current_file_id(&current_entry, &mut state.stats)
                    .await?;
                if id != saved_entry.id() {
                    state
                        .changes
                        .push(TreeChange::modified(current_entry.relative_path));
                }
                TreeEntry::blob(current_entry.name, id).map_err(|source| {
                    MaterializationError::InvalidTreeEntry {
                        path: current_entry.path,
                        source,
                    }
                })
            }
            (EntryKind::Tree, true) => {
                let id = self
                    .compare_directory(
                        saved_entry.id(),
                        current_entry.path.clone(),
                        current_entry.relative_path.clone(),
                        object_store,
                        state,
                    )
                    .await?;
                TreeEntry::tree(current_entry.name, id).map_err(|source| {
                    MaterializationError::InvalidTreeEntry {
                        path: current_entry.path,
                        source,
                    }
                })
            }
            (EntryKind::Blob, true) => {
                let id = self
                    .scan_directory(
                        current_entry.path.clone(),
                        current_entry.relative_path.clone(),
                        &mut state.stats,
                        &mut state.issues,
                    )
                    .await?;
                state
                    .changes
                    .push(TreeChange::type_changed(current_entry.relative_path));
                TreeEntry::tree(current_entry.name, id).map_err(|source| {
                    MaterializationError::InvalidTreeEntry {
                        path: current_entry.path,
                        source,
                    }
                })
            }
            (EntryKind::Tree, false) if current_entry.file_type.is_file() => {
                let id = self
                    .scan_current_file_id(&current_entry, &mut state.stats)
                    .await?;
                state
                    .changes
                    .push(TreeChange::type_changed(current_entry.relative_path));
                TreeEntry::blob(current_entry.name, id).map_err(|source| {
                    MaterializationError::InvalidTreeEntry {
                        path: current_entry.path,
                        source,
                    }
                })
            }
            _ => Err(MaterializationError::UnsupportedFileType {
                path: current_entry.path,
            }),
        }
    }

    async fn capture_current_file(
        &self,
        entry: &DirectoryEntry,
        object_store: &dyn ObjectStore,
        stats: &mut CaptureStats,
    ) -> Result<ObjectId, MaterializationError> {
        let metadata = fs::metadata(&entry.path)
            .await
            .map_err(|source| io_error(entry.path.clone(), source))?;
        let fingerprint = FileFingerprint::from_metadata(&metadata);

        let cached_id = self.cached_file_id(&entry.relative_path, &fingerprint);
        if let Some(cached) = cached_id
            && cached.stored
        {
            stats.files_seen += 1;
            stats.hash_cache_hits += 1;
            trace!(path = %entry.path.display(), id = %cached.object_id, "captured file blob from hash cache");
            return Ok(cached.object_id);
        }

        let bytes = fs::read(&entry.path)
            .await
            .map_err(|source| io_error(entry.path.clone(), source))?;
        let id = object_store.put_blob(&bytes).await?;

        stats.files_seen += 1;
        stats.bytes_read += bytes.len() as u64;
        stats.blobs_stored += 1;
        stats.hash_cache_misses += 1;
        self.store_cached_file_id(&entry.relative_path, fingerprint, id, true);
        trace!(path = %entry.path.display(), %id, bytes = bytes.len(), "captured file blob");

        Ok(id)
    }

    async fn scan_current_file_id(
        &self,
        entry: &DirectoryEntry,
        stats: &mut TreeScanStats,
    ) -> Result<ObjectId, MaterializationError> {
        let metadata = fs::metadata(&entry.path)
            .await
            .map_err(|source| io_error(entry.path.clone(), source))?;
        let fingerprint = FileFingerprint::from_metadata(&metadata);

        if let Some(cached) = self.cached_file_id(&entry.relative_path, &fingerprint) {
            stats.files_seen += 1;
            stats.hash_cache_hits += 1;
            trace!(path = %entry.path.display(), id = %cached.object_id, "scanned file blob from hash cache");
            return Ok(cached.object_id);
        }

        let bytes = fs::read(&entry.path)
            .await
            .map_err(|source| io_error(entry.path.clone(), source))?;
        let id = ObjectId::from_content(&bytes);

        stats.files_seen += 1;
        stats.bytes_read += bytes.len() as u64;
        stats.hash_cache_misses += 1;
        self.store_cached_file_id(&entry.relative_path, fingerprint, id, false);
        trace!(path = %entry.path.display(), %id, bytes = bytes.len(), "scanned file blob");
        Ok(id)
    }

    fn cached_file_id(
        &self,
        path: &Path,
        fingerprint: &FileFingerprint,
    ) -> Option<crate::hash_cache::CachedFileHash> {
        self.hash_cache
            .lock()
            .expect("hash cache mutex poisoned")
            .get(path, fingerprint)
    }

    fn store_cached_file_id(
        &self,
        path: &Path,
        fingerprint: FileFingerprint,
        object_id: ObjectId,
        stored: bool,
    ) {
        self.hash_cache
            .lock()
            .expect("hash cache mutex poisoned")
            .insert(path, fingerprint, object_id, stored);
    }

    fn materialize_directory<'a>(
        &'a self,
        tree_id: ObjectId,
        directory_path: PathBuf,
        object_store: &'a dyn ObjectStore,
        stats: &'a mut MaterializeStats,
    ) -> Pin<Box<dyn Future<Output = Result<(), MaterializationError>> + Send + 'a>> {
        Box::pin(async move {
            trace!(path = %directory_path.display(), %tree_id, "materializing directory");
            let tree = object_store.get_tree(&tree_id).await?;
            let target_names = tree
                .entries()
                .iter()
                .map(|entry| entry.name().to_owned())
                .collect::<BTreeSet<_>>();

            prune_directory(&directory_path, &target_names, &self.options, stats).await?;

            for entry in tree.entries() {
                let path = directory_path.join(entry.name());
                match entry.kind() {
                    EntryKind::Blob => {
                        materialize_blob(&path, entry.id(), object_store, stats).await?;
                    }
                    EntryKind::Tree => {
                        prepare_directory(&path, stats).await?;
                        self.materialize_directory(entry.id(), path, object_store, stats)
                            .await?;
                    }
                }
            }

            Ok(())
        })
    }
}

#[async_trait]
impl Materializer for FilesystemMaterializer {
    async fn capture_tree(
        &self,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<CaptureResult, MaterializationError> {
        FilesystemMaterializer::capture_tree(self, working_directory, object_store).await
    }

    async fn scan_tree(
        &self,
        working_directory: &WorkingDirectory,
    ) -> Result<TreeScanResult, MaterializationError> {
        FilesystemMaterializer::scan_tree(self, working_directory).await
    }

    async fn compare_tree(
        &self,
        saved_root_tree_id: ObjectId,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<TreeComparisonResult, MaterializationError> {
        FilesystemMaterializer::compare_tree(
            self,
            saved_root_tree_id,
            working_directory,
            object_store,
        )
        .await
    }

    async fn materialize_tree(
        &self,
        root_tree_id: ObjectId,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<MaterializeResult, MaterializationError> {
        FilesystemMaterializer::materialize_tree(
            self,
            root_tree_id,
            working_directory,
            object_store,
        )
        .await
    }

    async fn watch(
        &self,
        working_directory: &WorkingDirectory,
    ) -> Result<WorkingDirectoryWatch, MaterializationError> {
        FilesystemMaterializer::watch(self, working_directory).await
    }
}

#[derive(Debug, Default)]
struct ComparisonState {
    stats: TreeScanStats,
    issues: Vec<CaptureIssue>,
    changes: Vec<TreeChange>,
}

#[derive(Debug, Clone)]
struct DirectoryEntry {
    name: String,
    path: PathBuf,
    relative_path: PathBuf,
    file_type: std::fs::FileType,
}

async fn ensure_capture_root(root: &Path) -> Result<(), MaterializationError> {
    let metadata = fs::symlink_metadata(root)
        .await
        .map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => MaterializationError::RootMissing {
                path: root.to_path_buf(),
            },
            _ => io_error(root.to_path_buf(), source),
        })?;
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        return Err(MaterializationError::SymlinkUnsupported {
            path: root.to_path_buf(),
        });
    }

    if !file_type.is_dir() {
        return Err(MaterializationError::RootNotDirectory {
            path: root.to_path_buf(),
        });
    }

    Ok(())
}

async fn read_directory_entries(
    directory_path: &Path,
    relative_path: &Path,
) -> Result<Vec<DirectoryEntry>, MaterializationError> {
    let mut reader = fs::read_dir(directory_path)
        .await
        .map_err(|source| io_error(directory_path.to_path_buf(), source))?;
    let mut entries = Vec::new();

    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|source| io_error(directory_path.to_path_buf(), source))?
    {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .await
            .map_err(|source| io_error(path.clone(), source))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| MaterializationError::PathNotUtf8 { path: path.clone() })?;
        let relative_path = join_relative(relative_path, &name);

        entries.push(DirectoryEntry {
            name,
            path,
            relative_path,
            file_type,
        });
    }

    entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    Ok(entries)
}

async fn prune_directory(
    directory_path: &Path,
    target_names: &BTreeSet<String>,
    options: &CaptureOptions,
    stats: &mut MaterializeStats,
) -> Result<(), MaterializationError> {
    let entries = read_directory_entries(directory_path, Path::new("")).await?;

    for entry in entries {
        if target_names.contains(&entry.name) {
            continue;
        }

        if entry.file_type.is_dir() && options.excludes_directory_name(&entry.name) {
            trace!(path = %entry.path.display(), "preserving excluded directory");
            continue;
        }

        if entry.file_type.is_symlink() {
            match options.symlink_policy() {
                SymlinkPolicy::Skip => {
                    trace!(path = %entry.path.display(), "preserving skipped symlink");
                    continue;
                }
                SymlinkPolicy::Error => {
                    return Err(MaterializationError::SymlinkUnsupported { path: entry.path });
                }
            }
        }

        remove_path(&entry.path, entry.file_type, stats).await?;
    }

    Ok(())
}

async fn materialize_blob(
    path: &Path,
    id: ObjectId,
    object_store: &dyn ObjectStore,
    stats: &mut MaterializeStats,
) -> Result<(), MaterializationError> {
    prepare_file_path(path, stats).await?;
    let bytes = object_store.get_blob(&id).await?;
    fs::write(path, &bytes)
        .await
        .map_err(|source| io_error(path.to_path_buf(), source))?;

    stats.files_written += 1;
    stats.bytes_written += bytes.len() as u64;
    trace!(path = %path.display(), %id, bytes = bytes.len(), "materialized file blob");
    Ok(())
}

async fn prepare_file_path(
    path: &Path,
    stats: &mut MaterializeStats,
) -> Result<(), MaterializationError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(metadata) => remove_path(path, metadata.file_type(), stats).await,
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(path.to_path_buf(), source)),
    }
}

async fn prepare_directory(
    path: &Path,
    stats: &mut MaterializeStats,
) -> Result<(), MaterializationError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(metadata) => {
            remove_path(path, metadata.file_type(), stats).await?;
            fs::create_dir(path)
                .await
                .map_err(|source| io_error(path.to_path_buf(), source))?;
            stats.directories_created += 1;
            Ok(())
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .await
                .map_err(|source| io_error(path.to_path_buf(), source))?;
            stats.directories_created += 1;
            Ok(())
        }
        Err(source) => Err(io_error(path.to_path_buf(), source)),
    }
}

async fn remove_path(
    path: &Path,
    file_type: std::fs::FileType,
    stats: &mut MaterializeStats,
) -> Result<(), MaterializationError> {
    if file_type.is_dir() {
        fs::remove_dir_all(path)
            .await
            .map_err(|source| io_error(path.to_path_buf(), source))?;
    } else {
        fs::remove_file(path)
            .await
            .map_err(|source| io_error(path.to_path_buf(), source))?;
    }

    stats.entries_removed += 1;
    trace!(path = %path.display(), "removed working-directory entry");
    Ok(())
}

fn join_relative(parent: &Path, name: &str) -> PathBuf {
    if parent.as_os_str().is_empty() {
        PathBuf::from(name)
    } else {
        parent.join(name)
    }
}

fn io_error(path: PathBuf, source: io::Error) -> MaterializationError {
    MaterializationError::Io { path, source }
}
