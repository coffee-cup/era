use crate::{
    CaptureIssue, CaptureIssueKind, CaptureOptions, CaptureResult, CaptureStats,
    MaterializationError, Materializer, SymlinkPolicy, WorkingDirectory,
};
use async_trait::async_trait;
use era_core::{ObjectId, Tree, TreeEntry};
use era_object_store::ObjectStore;
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
};
use tokio::fs;
use tracing::{debug, trace};

/// Copy-based filesystem materializer for ordinary working directories.
#[derive(Debug, Clone, Default)]
pub struct FilesystemMaterializer {
    options: CaptureOptions,
}

impl FilesystemMaterializer {
    /// Creates a materializer with default capture options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a materializer with explicit capture options.
    pub fn with_options(options: CaptureOptions) -> Self {
        Self { options }
    }

    /// Returns this materializer's capture options.
    pub fn options(&self) -> &CaptureOptions {
        &self.options
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
                    let bytes = fs::read(&entry.path)
                        .await
                        .map_err(|source| io_error(entry.path.clone(), source))?;
                    let id = object_store.put_blob(&bytes).await?;

                    stats.files_seen += 1;
                    stats.bytes_read += bytes.len() as u64;
                    stats.blobs_stored += 1;
                    trace!(path = %entry.path.display(), %id, bytes = bytes.len(), "captured file blob");

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
}

#[derive(Debug)]
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
