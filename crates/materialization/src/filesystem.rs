use crate::{
    CaptureIssue, CaptureIssueKind, CaptureOptions, CaptureResult, CaptureStats,
    MaterializationError, MaterializeResult, MaterializeStats, Materializer, SymlinkPolicy,
    TreeChange, TreeComparisonResult, TreeScanResult, TreeScanStats, WorkingDirectory,
    WorkingDirectoryWatch,
    capture_cache::{CachedDirectoryTree, CaptureCache, FileFingerprint},
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
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};
use tokio::fs;
use tracing::{debug, trace, warn};

/// Copy-based filesystem materializer for ordinary working directories.
#[derive(Debug, Clone)]
pub struct FilesystemMaterializer {
    options: CaptureOptions,
    cache: Arc<Mutex<CaptureCache>>,
    cache_path: Option<PathBuf>,
    cache_loaded: Arc<AtomicBool>,
    cache_dirty: Arc<AtomicBool>,
}

impl Default for FilesystemMaterializer {
    fn default() -> Self {
        Self::with_options(CaptureOptions::default())
    }
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
            cache: Arc::new(Mutex::new(CaptureCache::default())),
            cache_path: None,
            cache_loaded: Arc::new(AtomicBool::new(true)),
            cache_dirty: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates a materializer that persists its cache at `cache_path`.
    #[must_use]
    pub fn with_cache_path(cache_path: impl Into<PathBuf>) -> Self {
        Self::with_options_and_cache_path(CaptureOptions::default(), cache_path)
    }

    /// Creates a materializer with explicit options and a persistent cache path.
    #[must_use]
    pub fn with_options_and_cache_path(
        options: CaptureOptions,
        cache_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            options,
            cache: Arc::new(Mutex::new(CaptureCache::default())),
            cache_path: Some(cache_path.into()),
            cache_loaded: Arc::new(AtomicBool::new(false)),
            cache_dirty: Arc::new(AtomicBool::new(false)),
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
        let mut cache = self.cache.lock().expect("capture cache mutex poisoned");
        for path in paths {
            if cache.invalidate_path(path.as_ref()) {
                self.cache_dirty.store(true, AtomicOrdering::Relaxed);
            }
        }
    }

    async fn ensure_cache_loaded(&self) {
        if self.cache_loaded.load(AtomicOrdering::Acquire) {
            return;
        }

        let Some(path) = &self.cache_path else {
            self.cache_loaded.store(true, AtomicOrdering::Release);
            return;
        };

        let loaded = CaptureCache::load(path).await;
        let was_loaded = self.cache_loaded.swap(true, AtomicOrdering::AcqRel);
        if !was_loaded {
            *self.cache.lock().expect("capture cache mutex poisoned") = loaded;
            self.cache_dirty.store(false, AtomicOrdering::Release);
        }
    }

    async fn save_cache_if_dirty(&self) {
        let Some(path) = &self.cache_path else {
            return;
        };
        if !self.cache_dirty.swap(false, AtomicOrdering::AcqRel) {
            return;
        }

        let result = self
            .cache
            .lock()
            .expect("capture cache mutex poisoned")
            .flush();
        if let Err(source) = result {
            self.cache_dirty.store(true, AtomicOrdering::Release);
            warn!(path = %path.display(), %source, "capture cache could not be saved");
        }
    }

    fn load_all_cache_records(&self) {
        if let Err(message) = self
            .cache
            .lock()
            .expect("capture cache mutex poisoned")
            .load_all()
        {
            warn!(%message, "capture cache records could not be loaded; continuing with misses");
        }
    }

    fn drop_bulk_loaded_cache_records(&self) {
        self.cache
            .lock()
            .expect("capture cache mutex poisoned")
            .drop_bulk_loaded_records();
    }

    /// Captures the current working directory into blob and tree objects.
    pub async fn capture_tree(
        &self,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<CaptureResult, MaterializationError> {
        self.ensure_cache_loaded().await;
        self.load_all_cache_records();
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

        let result = CaptureResult::new(root_tree_id, stats, issues);
        self.save_cache_if_dirty().await;
        self.drop_bulk_loaded_cache_records();
        Ok(result)
    }

    /// Captures the working directory using trusted dirty-path hints when possible.
    pub async fn capture_tree_with_hints(
        &self,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
        hints: &[PathBuf],
    ) -> Result<CaptureResult, MaterializationError> {
        self.ensure_cache_loaded().await;
        let root = working_directory.root();
        debug!(root = %root.display(), hints = hints.len(), "capturing working directory with hints");
        ensure_capture_root(root).await?;

        if hints.is_empty() {
            return self.capture_tree(working_directory, object_store).await;
        }

        let hints = normalize_hints(&self.options, hints);
        if hints.is_empty() {
            if let Some(root_tree) = self.cached_directory(Path::new(""))
                && root_tree.stored
            {
                return Ok(CaptureResult::new(
                    root_tree.tree_id,
                    CaptureStats::default(),
                    Vec::new(),
                ));
            }
            return self.capture_tree(working_directory, object_store).await;
        }

        let mut stats = CaptureStats::default();
        let mut issues = Vec::new();
        match self
            .capture_hinted_paths(root, &hints, object_store, &mut stats, &mut issues)
            .await?
        {
            Some(root_tree_id) => {
                let result = CaptureResult::new(root_tree_id, stats, issues);
                self.save_cache_if_dirty().await;
                Ok(result)
            }
            None => self.capture_tree(working_directory, object_store).await,
        }
    }

    /// Scans the current working directory without storing objects.
    pub async fn scan_tree(
        &self,
        working_directory: &WorkingDirectory,
    ) -> Result<TreeScanResult, MaterializationError> {
        self.ensure_cache_loaded().await;
        self.load_all_cache_records();
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

        let result = TreeScanResult::new(root_tree_id, stats, issues);
        self.save_cache_if_dirty().await;
        self.drop_bulk_loaded_cache_records();
        Ok(result)
    }

    /// Compares the current working directory with a stored tree.
    pub async fn compare_tree(
        &self,
        saved_root_tree_id: ObjectId,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
    ) -> Result<TreeComparisonResult, MaterializationError> {
        self.ensure_cache_loaded().await;
        self.load_all_cache_records();
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

        let result = TreeComparisonResult::new(
            saved_root_tree_id,
            current_root_tree_id,
            state.changes,
            state.stats,
            state.issues,
        );
        self.save_cache_if_dirty().await;
        self.drop_bulk_loaded_cache_records();
        Ok(result)
    }

    /// Watches the working directory and emits filesystem change hints.
    pub async fn watch(
        &self,
        working_directory: &WorkingDirectory,
    ) -> Result<WorkingDirectoryWatch, MaterializationError> {
        self.ensure_cache_loaded().await;
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
        self.ensure_cache_loaded().await;
        let root = working_directory.root();
        debug!(root = %root.display(), %root_tree_id, "materializing working directory");
        ensure_capture_root(root).await?;

        let mut stats = MaterializeStats::default();
        self.materialize_directory(
            root_tree_id,
            root.to_path_buf(),
            PathBuf::new(),
            object_store,
            &mut stats,
        )
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

        let result = MaterializeResult::new(stats);
        self.save_cache_if_dirty().await;
        self.drop_bulk_loaded_cache_records();
        Ok(result)
    }

    async fn capture_hinted_paths(
        &self,
        root: &Path,
        hints: &[PathBuf],
        object_store: &dyn ObjectStore,
        stats: &mut CaptureStats,
        issues: &mut Vec<CaptureIssue>,
    ) -> Result<Option<ObjectId>, MaterializationError> {
        let Some(root_tree) = self.cached_directory(Path::new("")) else {
            return Ok(None);
        };
        if !root_tree.stored {
            return Ok(None);
        }

        for hint in hints {
            if hint.as_os_str().is_empty() {
                return Ok(None);
            }
            if self
                .capture_hinted_path(root, hint, object_store, stats, issues)
                .await?
                .is_none()
            {
                return Ok(None);
            }
        }

        Ok(self
            .cached_directory(Path::new(""))
            .filter(|directory| directory.stored)
            .map(|directory| directory.tree_id))
    }

    async fn capture_hinted_path(
        &self,
        root: &Path,
        relative_path: &Path,
        object_store: &dyn ObjectStore,
        stats: &mut CaptureStats,
        issues: &mut Vec<CaptureIssue>,
    ) -> Result<Option<()>, MaterializationError> {
        self.invalidate_paths([relative_path.to_path_buf()]);
        let parent_path = relative_parent(relative_path);
        let Some(parent_directory) = self.cached_directory(&parent_path) else {
            return Ok(None);
        };
        if !parent_directory.stored {
            return Ok(None);
        }

        let name = relative_file_name(relative_path)?;
        let path = root.join(relative_path);
        let replacement = match fs::symlink_metadata(&path).await {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                let entry = DirectoryEntry {
                    name: name.clone(),
                    path: path.clone(),
                    relative_path: relative_path.to_path_buf(),
                    file_type,
                };
                if should_skip_entry(&self.options, &entry) {
                    None
                } else if file_type.is_dir() {
                    let child_tree_id = self
                        .capture_directory(
                            path,
                            relative_path.to_path_buf(),
                            object_store,
                            stats,
                            issues,
                        )
                        .await?;
                    Some(
                        TreeEntry::tree(name.clone(), child_tree_id).map_err(|source| {
                            MaterializationError::InvalidTreeEntry {
                                path: root.join(relative_path),
                                source,
                            }
                        })?,
                    )
                } else if file_type.is_file() {
                    let base_id = parent_directory
                        .entries
                        .iter()
                        .find(|entry| entry.name() == name && entry.kind() == EntryKind::Blob)
                        .map(|entry| entry.id());
                    let id = self
                        .capture_current_file(&entry, object_store, stats, base_id)
                        .await?;
                    Some(TreeEntry::blob(name.clone(), id).map_err(|source| {
                        MaterializationError::InvalidTreeEntry {
                            path: root.join(relative_path),
                            source,
                        }
                    })?)
                } else if file_type.is_symlink() {
                    match self.options.symlink_policy() {
                        SymlinkPolicy::Skip => {
                            stats.symlinks_skipped += 1;
                            issues.push(CaptureIssue::new(
                                relative_path.to_path_buf(),
                                CaptureIssueKind::SkippedSymlink,
                            ));
                            None
                        }
                        SymlinkPolicy::Error => {
                            return Err(MaterializationError::SymlinkUnsupported { path });
                        }
                    }
                } else {
                    return Err(MaterializationError::UnsupportedFileType { path });
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => return Err(io_error(path, source)),
        };

        let entries = replace_cached_entry(parent_directory.entries, &name, replacement);
        self.store_cached_directory_entries(&parent_path, entries);
        self.rebuild_cached_ancestors(parent_path, object_store, stats)
            .await
            .map(Some)
    }

    fn rebuild_cached_ancestors<'a>(
        &'a self,
        start_path: PathBuf,
        object_store: &'a dyn ObjectStore,
        stats: &'a mut CaptureStats,
    ) -> Pin<Box<dyn Future<Output = Result<(), MaterializationError>> + Send + 'a>> {
        Box::pin(async move {
            let mut current_path = start_path;
            loop {
                let Some(directory) = self.cached_directory(&current_path) else {
                    return Err(MaterializationError::UnsupportedFileType { path: current_path });
                };
                let tree = Tree::new(directory.entries.clone()).map_err(|source| {
                    MaterializationError::InvalidTreeEntry {
                        path: current_path.clone(),
                        source,
                    }
                })?;
                let tree_id = tree.id();
                if !directory.stored || directory.tree_id != tree_id {
                    object_store.put_tree(&tree).await?;
                    stats.trees_stored += 1;
                }
                self.store_cached_directory(&current_path, tree_id, tree.entries().to_vec(), true);

                if current_path.as_os_str().is_empty() {
                    return Ok(());
                }

                let parent_path = relative_parent(&current_path);
                let Some(parent_directory) = self.cached_directory(&parent_path) else {
                    return Err(MaterializationError::UnsupportedFileType { path: parent_path });
                };
                let name = relative_file_name(&current_path)?;
                let entry = TreeEntry::tree(name.clone(), tree_id).map_err(|source| {
                    MaterializationError::InvalidTreeEntry {
                        path: current_path.clone(),
                        source,
                    }
                })?;
                let entries = replace_cached_entry(parent_directory.entries, &name, Some(entry));
                self.store_cached_directory_entries(&parent_path, entries);
                current_path = parent_path;
            }
        })
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
                if should_skip_entry(&self.options, &entry) {
                    trace!(path = %entry.path.display(), "skipping excluded entry");
                    stats.ignored_entries += 1;
                    continue;
                }

                if entry.file_type.is_dir() {
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
                        .capture_current_file(&entry, object_store, stats, None)
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
            let tree_id = tree.id();
            let cached = self.cached_directory(&relative_path);
            if !cached
                .as_ref()
                .is_some_and(|directory| directory.stored && directory.tree_id == tree_id)
            {
                object_store.put_tree(&tree).await?;
                stats.trees_stored += 1;
            }
            self.store_cached_directory(&relative_path, tree_id, tree.entries().to_vec(), true);
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
                if should_skip_entry(&self.options, &entry) {
                    trace!(path = %entry.path.display(), "skipping excluded entry");
                    stats.ignored_entries += 1;
                    continue;
                }

                if entry.file_type.is_dir() {
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
            self.store_cached_directory(&relative_path, tree_id, tree.entries().to_vec(), false);
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
            self.store_cached_directory(
                &relative_path,
                current_tree_id,
                current_tree.entries().to_vec(),
                false,
            );
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
            if should_skip_entry(&self.options, &entry) {
                trace!(path = %entry.path.display(), "skipping excluded entry");
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
        base_id: Option<ObjectId>,
    ) -> Result<ObjectId, MaterializationError> {
        let metadata = fs::metadata(&entry.path)
            .await
            .map_err(|source| io_error(entry.path.clone(), source))?;
        let fingerprint = FileFingerprint::from_metadata(&metadata);

        let cached = self.cached_file_lookup(&entry.relative_path, &fingerprint);
        if let Some(cached) = cached.matching
            && cached.stored
        {
            stats.files_seen += 1;
            stats.hash_cache_hits += 1;
            trace!(path = %entry.path.display(), id = %cached.object_id, "captured file blob from hash cache");
            return Ok(cached.object_id);
        }

        let base_id = base_id.or(cached.stored_base_id);
        let bytes = fs::read(&entry.path)
            .await
            .map_err(|source| io_error(entry.path.clone(), source))?;
        let id = object_store
            .put_blob_with_base(&bytes, base_id.as_ref())
            .await?;

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
    ) -> Option<crate::capture_cache::CachedFileHash> {
        self.cache
            .lock()
            .expect("capture cache mutex poisoned")
            .get_file(path, fingerprint)
    }

    fn cached_file_lookup(
        &self,
        path: &Path,
        fingerprint: &FileFingerprint,
    ) -> crate::capture_cache::CachedFileLookup {
        self.cache
            .lock()
            .expect("capture cache mutex poisoned")
            .file_lookup(path, fingerprint)
    }

    fn store_cached_file_id(
        &self,
        path: &Path,
        fingerprint: FileFingerprint,
        object_id: ObjectId,
        stored: bool,
    ) {
        if self
            .cache
            .lock()
            .expect("capture cache mutex poisoned")
            .insert_file(path, fingerprint, object_id, stored)
        {
            self.cache_dirty.store(true, AtomicOrdering::Relaxed);
        }
    }

    fn cached_directory(&self, path: &Path) -> Option<CachedDirectoryTree> {
        self.cache
            .lock()
            .expect("capture cache mutex poisoned")
            .get_directory(path)
    }

    fn store_cached_directory(
        &self,
        path: &Path,
        tree_id: ObjectId,
        entries: Vec<TreeEntry>,
        stored: bool,
    ) {
        if self
            .cache
            .lock()
            .expect("capture cache mutex poisoned")
            .insert_directory(path, tree_id, entries, stored)
        {
            self.cache_dirty.store(true, AtomicOrdering::Relaxed);
        }
    }

    fn store_cached_directory_entries(&self, path: &Path, entries: Vec<TreeEntry>) {
        if self
            .cache
            .lock()
            .expect("capture cache mutex poisoned")
            .update_directory_entries(path, entries)
        {
            self.cache_dirty.store(true, AtomicOrdering::Relaxed);
        }
    }

    fn materialize_directory<'a>(
        &'a self,
        tree_id: ObjectId,
        directory_path: PathBuf,
        relative_path: PathBuf,
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

            let removed_paths = prune_directory(
                &directory_path,
                &relative_path,
                &target_names,
                &self.options,
                stats,
            )
            .await?;
            if !removed_paths.is_empty() {
                self.invalidate_paths(removed_paths);
            }

            for entry in tree.entries() {
                let path = directory_path.join(entry.name());
                match entry.kind() {
                    EntryKind::Blob => {
                        let relative_child = join_relative(&relative_path, entry.name());
                        self.materialize_blob(
                            &path,
                            &relative_child,
                            entry.id(),
                            object_store,
                            stats,
                        )
                        .await?;
                    }
                    EntryKind::Tree => {
                        let relative_child = join_relative(&relative_path, entry.name());
                        prepare_directory(&path, stats).await?;
                        self.materialize_directory(
                            entry.id(),
                            path,
                            relative_child,
                            object_store,
                            stats,
                        )
                        .await?;
                    }
                }
            }

            self.store_cached_directory(&relative_path, tree_id, tree.entries().to_vec(), true);
            Ok(())
        })
    }

    async fn materialize_blob(
        &self,
        path: &Path,
        relative_path: &Path,
        id: ObjectId,
        object_store: &dyn ObjectStore,
        stats: &mut MaterializeStats,
    ) -> Result<(), MaterializationError> {
        if let Ok(metadata) = fs::symlink_metadata(path).await
            && metadata.file_type().is_file()
        {
            let fingerprint = FileFingerprint::from_metadata(&metadata);
            if let Some(cached) = self.cached_file_id(relative_path, &fingerprint)
                && cached.object_id == id
            {
                self.store_cached_file_id(relative_path, fingerprint, id, true);
                trace!(path = %path.display(), %id, "materialized file already matches from cache");
                return Ok(());
            }

            let bytes = fs::read(path)
                .await
                .map_err(|source| io_error(path.to_path_buf(), source))?;
            if ObjectId::from_content(&bytes) == id {
                self.store_cached_file_id(relative_path, fingerprint, id, true);
                trace!(path = %path.display(), %id, bytes = bytes.len(), "materialized file already matches");
                return Ok(());
            }
        }

        self.invalidate_paths([relative_path.to_path_buf()]);
        prepare_file_path(path, stats).await?;
        let bytes = object_store.get_blob(&id).await?;
        fs::write(path, &bytes)
            .await
            .map_err(|source| io_error(path.to_path_buf(), source))?;

        let metadata = fs::metadata(path)
            .await
            .map_err(|source| io_error(path.to_path_buf(), source))?;
        self.store_cached_file_id(
            relative_path,
            FileFingerprint::from_metadata(&metadata),
            id,
            true,
        );
        stats.files_written += 1;
        stats.bytes_written += bytes.len() as u64;
        trace!(path = %path.display(), %id, bytes = bytes.len(), "materialized file blob");
        Ok(())
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

    async fn capture_tree_with_hints(
        &self,
        working_directory: &WorkingDirectory,
        object_store: &dyn ObjectStore,
        hints: &[PathBuf],
    ) -> Result<CaptureResult, MaterializationError> {
        FilesystemMaterializer::capture_tree_with_hints(
            self,
            working_directory,
            object_store,
            hints,
        )
        .await
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
    relative_path: &Path,
    target_names: &BTreeSet<String>,
    options: &CaptureOptions,
    stats: &mut MaterializeStats,
) -> Result<Vec<PathBuf>, MaterializationError> {
    let entries = read_directory_entries(directory_path, relative_path).await?;
    let mut removed_paths = Vec::new();

    for entry in entries {
        if target_names.contains(&entry.name) {
            continue;
        }

        if should_skip_entry(options, &entry) {
            trace!(path = %entry.path.display(), "preserving excluded entry");
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

        let relative_path = entry.relative_path.clone();
        remove_path(&entry.path, entry.file_type, stats).await?;
        removed_paths.push(relative_path);
    }

    Ok(removed_paths)
}

fn should_skip_entry(options: &CaptureOptions, entry: &DirectoryEntry) -> bool {
    entry.name == ".era"
        || (entry.file_type.is_dir() && options.excludes_directory_name(&entry.name))
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

fn normalize_hints(options: &CaptureOptions, hints: &[PathBuf]) -> Vec<PathBuf> {
    let mut normalized = hints
        .iter()
        .filter(|path| !options.excludes_path(path))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let mut compacted: Vec<PathBuf> = Vec::new();
    for path in normalized.drain(..) {
        if compacted
            .iter()
            .any(|ancestor| !ancestor.as_os_str().is_empty() && path.starts_with(ancestor))
        {
            continue;
        }
        compacted.push(path);
    }
    compacted
}

fn replace_cached_entry(
    mut entries: Vec<TreeEntry>,
    name: &str,
    replacement: Option<TreeEntry>,
) -> Vec<TreeEntry> {
    entries.retain(|entry| entry.name() != name);
    if let Some(entry) = replacement {
        entries.push(entry);
    }
    entries.sort_by(|left, right| left.name().as_bytes().cmp(right.name().as_bytes()));
    entries
}

fn relative_parent(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn relative_file_name(path: &Path) -> Result<String, MaterializationError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| MaterializationError::PathNotUtf8 {
            path: path.to_path_buf(),
        })
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
