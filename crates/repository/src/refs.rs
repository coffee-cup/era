use crate::{
    BranchName, RepositoryError, WorkspaceId, error::io_error, workspace::WorkspacePointer,
};
use era_core::ObjectId;
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};
use tokio::{
    fs,
    fs::OpenOptions,
    io::AsyncWriteExt,
    time::{Instant, sleep},
};

pub(crate) const ERA_DIR_NAME: &str = ".era";
pub(crate) const OBJECTS_DIR_NAME: &str = "objects";
const HEAD_FILE_NAME: &str = "HEAD";
const REFS_DIR_NAME: &str = "refs";
const HEADS_DIR_NAME: &str = "heads";
const WORKSPACE_REFS_DIR_NAME: &str = "workspaces";
const WORKSPACES_DIR_NAME: &str = "workspaces";
const WORKSPACE_PATH_FILE_NAME: &str = "path";
const LOCKS_DIR_NAME: &str = "locks";
const HEAD_PREFIX: &str = "ref: ";
const REFS_HEADS_PREFIX: &str = "refs/heads/";
const WORKSPACE_POINTER_MAGIC: &str = "era-workspace-v1";
const POINTER_METADATA_PREFIX: &str = "metadata: ";
const POINTER_WORKSPACE_PREFIX: &str = "workspace: ";
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const STALE_LOCK_AGE: Duration = Duration::from_secs(60);

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn metadata_dir(root: &Path) -> PathBuf {
    root.join(ERA_DIR_NAME)
}

pub(crate) fn objects_dir(metadata_dir: &Path) -> PathBuf {
    metadata_dir.join(OBJECTS_DIR_NAME)
}

pub(crate) fn head_path(metadata_dir: &Path) -> PathBuf {
    metadata_dir.join(HEAD_FILE_NAME)
}

pub(crate) fn branch_ref_path(metadata_dir: &Path, branch: &BranchName) -> PathBuf {
    branch_refs_dir(metadata_dir).join(branch.as_str())
}

pub(crate) fn branch_refs_dir(metadata_dir: &Path) -> PathBuf {
    metadata_dir.join(REFS_DIR_NAME).join(HEADS_DIR_NAME)
}

pub(crate) fn workspace_ref_path(metadata_dir: &Path, workspace: &WorkspaceId) -> PathBuf {
    workspace_refs_dir(metadata_dir).join(workspace.as_str())
}

pub(crate) fn workspace_refs_dir(metadata_dir: &Path) -> PathBuf {
    metadata_dir
        .join(REFS_DIR_NAME)
        .join(WORKSPACE_REFS_DIR_NAME)
}

pub(crate) fn workspaces_dir(metadata_dir: &Path) -> PathBuf {
    metadata_dir.join(WORKSPACES_DIR_NAME)
}

pub(crate) fn workspace_record_dir(metadata_dir: &Path, workspace: &WorkspaceId) -> PathBuf {
    workspaces_dir(metadata_dir).join(workspace.as_str())
}

pub(crate) fn workspace_path_file(metadata_dir: &Path, workspace: &WorkspaceId) -> PathBuf {
    workspace_record_dir(metadata_dir, workspace).join(WORKSPACE_PATH_FILE_NAME)
}

pub(crate) fn workspace_pointer_path(root: &Path) -> PathBuf {
    root.join(ERA_DIR_NAME)
}

pub(crate) fn branch_lock_path(metadata_dir: &Path, branch: &BranchName) -> PathBuf {
    metadata_dir
        .join(LOCKS_DIR_NAME)
        .join(REFS_DIR_NAME)
        .join(HEADS_DIR_NAME)
        .join(format!("{}.lock", branch.as_str()))
}

pub(crate) fn workspace_ref_lock_path(metadata_dir: &Path, workspace: &WorkspaceId) -> PathBuf {
    metadata_dir
        .join(LOCKS_DIR_NAME)
        .join(REFS_DIR_NAME)
        .join(WORKSPACE_REFS_DIR_NAME)
        .join(format!("{}.lock", workspace.as_str()))
}

pub(crate) fn workspace_record_lock_path(metadata_dir: &Path, workspace: &WorkspaceId) -> PathBuf {
    metadata_dir
        .join(LOCKS_DIR_NAME)
        .join(WORKSPACES_DIR_NAME)
        .join(format!("{}.lock", workspace.as_str()))
}

pub(crate) async fn create_ref_layout(metadata_dir: &Path) -> Result<(), RepositoryError> {
    for directory in [
        branch_refs_dir(metadata_dir),
        workspace_refs_dir(metadata_dir),
        workspaces_dir(metadata_dir),
        metadata_dir.join(LOCKS_DIR_NAME),
    ] {
        fs::create_dir_all(&directory)
            .await
            .map_err(|source| io_error(directory, source))?;
    }

    Ok(())
}

pub(crate) async fn write_head(
    metadata_dir: &Path,
    branch: &BranchName,
) -> Result<(), RepositoryError> {
    let path = head_path(metadata_dir);
    let contents = format!("{HEAD_PREFIX}{REFS_HEADS_PREFIX}{}\n", branch.as_str());
    write_text_file_atomic(&path, &contents).await
}

pub(crate) async fn read_head_branch(metadata_dir: &Path) -> Result<BranchName, RepositoryError> {
    let path = head_path(metadata_dir);
    let contents = read_text_file(&path, || RepositoryError::HeadMissing {
        path: path.clone(),
    })
    .await?;
    parse_head(&path, &contents)
}

pub(crate) async fn create_branch_ref(
    metadata_dir: &Path,
    branch: &BranchName,
    snapshot_id: ObjectId,
) -> Result<(), RepositoryError> {
    let path = branch_ref_path(metadata_dir, branch);
    create_text_file_new(&path, &format!("{snapshot_id}\n"), || {
        RepositoryError::BranchAlreadyExists {
            name: branch.clone(),
        }
    })
    .await
}

pub(crate) async fn write_branch_ref(
    metadata_dir: &Path,
    branch: &BranchName,
    snapshot_id: ObjectId,
) -> Result<(), RepositoryError> {
    let path = branch_ref_path(metadata_dir, branch);
    write_text_file_atomic(&path, &format!("{snapshot_id}\n")).await
}

pub(crate) async fn branch_ref_exists(
    metadata_dir: &Path,
    branch: &BranchName,
) -> Result<bool, RepositoryError> {
    let path = branch_ref_path(metadata_dir, branch);
    fs::try_exists(&path)
        .await
        .map_err(|source| io_error(path, source))
}

pub(crate) async fn read_branch_ref(
    metadata_dir: &Path,
    branch: &BranchName,
) -> Result<ObjectId, RepositoryError> {
    let path = branch_ref_path(metadata_dir, branch);
    let contents =
        read_text_file(&path, || RepositoryError::RefMissing { path: path.clone() }).await?;
    parse_ref(&path, &contents)
}

pub(crate) async fn list_branch_refs(
    metadata_dir: &Path,
) -> Result<Vec<(BranchName, ObjectId)>, RepositoryError> {
    let heads_dir = branch_refs_dir(metadata_dir);
    let mut reader = fs::read_dir(&heads_dir)
        .await
        .map_err(|source| io_error(heads_dir.clone(), source))?;
    let mut branches = Vec::new();

    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|source| io_error(heads_dir.clone(), source))?
    {
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| RepositoryError::InvalidBranchRefName { path: path.clone() })?;
        let branch = BranchName::new(name)?;
        let snapshot_id = read_branch_ref(metadata_dir, &branch).await?;
        branches.push((branch, snapshot_id));
    }

    branches.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    Ok(branches)
}

pub(crate) async fn create_workspace_ref(
    metadata_dir: &Path,
    workspace: &WorkspaceId,
    snapshot_id: ObjectId,
) -> Result<(), RepositoryError> {
    let path = workspace_ref_path(metadata_dir, workspace);
    create_text_file_new(&path, &format!("{snapshot_id}\n"), || {
        RepositoryError::WorkspaceAlreadyExists {
            id: workspace.clone(),
            existing_path: path.clone(),
            requested_path: path.clone(),
        }
    })
    .await
}

pub(crate) async fn write_workspace_ref(
    metadata_dir: &Path,
    workspace: &WorkspaceId,
    snapshot_id: ObjectId,
) -> Result<(), RepositoryError> {
    let path = workspace_ref_path(metadata_dir, workspace);
    write_text_file_atomic(&path, &format!("{snapshot_id}\n")).await
}

pub(crate) async fn workspace_ref_exists(
    metadata_dir: &Path,
    workspace: &WorkspaceId,
) -> Result<bool, RepositoryError> {
    let path = workspace_ref_path(metadata_dir, workspace);
    fs::try_exists(&path)
        .await
        .map_err(|source| io_error(path, source))
}

pub(crate) async fn read_workspace_ref(
    metadata_dir: &Path,
    workspace: &WorkspaceId,
) -> Result<ObjectId, RepositoryError> {
    let path = workspace_ref_path(metadata_dir, workspace);
    let contents =
        read_text_file(&path, || RepositoryError::RefMissing { path: path.clone() }).await?;
    parse_ref(&path, &contents)
}

pub(crate) async fn list_workspace_refs(
    metadata_dir: &Path,
) -> Result<Vec<(WorkspaceId, ObjectId)>, RepositoryError> {
    let refs_dir = workspace_refs_dir(metadata_dir);
    let mut reader = fs::read_dir(&refs_dir)
        .await
        .map_err(|source| io_error(refs_dir.clone(), source))?;
    let mut workspaces = Vec::new();

    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|source| io_error(refs_dir.clone(), source))?
    {
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| RepositoryError::InvalidWorkspaceRefName { path: path.clone() })?;
        let workspace = WorkspaceId::new(name)?;
        let snapshot_id = read_workspace_ref(metadata_dir, &workspace).await?;
        workspaces.push((workspace, snapshot_id));
    }

    workspaces.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    Ok(workspaces)
}

pub(crate) async fn write_workspace_path(
    metadata_dir: &Path,
    workspace: &WorkspaceId,
    path: &Path,
) -> Result<(), RepositoryError> {
    let path_file = workspace_path_file(metadata_dir, workspace);
    write_text_file_atomic(&path_file, &format!("{}\n", path.display())).await
}

pub(crate) async fn read_workspace_path(
    metadata_dir: &Path,
    workspace: &WorkspaceId,
) -> Result<PathBuf, RepositoryError> {
    let path = workspace_path_file(metadata_dir, workspace);
    let contents = read_text_file(&path, || RepositoryError::WorkspaceNotFound {
        id: workspace.clone(),
    })
    .await?;
    let line = single_line(&contents).ok_or_else(|| RepositoryError::InvalidRef {
        path: path.clone(),
        contents: contents.clone(),
    })?;
    Ok(PathBuf::from(line))
}

pub(crate) async fn workspace_record_exists(
    metadata_dir: &Path,
    workspace: &WorkspaceId,
) -> Result<bool, RepositoryError> {
    let path = workspace_path_file(metadata_dir, workspace);
    fs::try_exists(&path)
        .await
        .map_err(|source| io_error(path, source))
}

pub(crate) async fn list_workspace_paths(
    metadata_dir: &Path,
) -> Result<Vec<(WorkspaceId, Option<PathBuf>)>, RepositoryError> {
    let directory = workspaces_dir(metadata_dir);
    let mut reader = fs::read_dir(&directory)
        .await
        .map_err(|source| io_error(directory.clone(), source))?;
    let mut workspaces = Vec::new();

    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|source| io_error(directory.clone(), source))?
    {
        let path = entry.path();
        if !entry
            .file_type()
            .await
            .map_err(|source| io_error(path.clone(), source))?
            .is_dir()
        {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| RepositoryError::InvalidWorkspaceRefName { path: path.clone() })?;
        let workspace = WorkspaceId::new(name)?;
        let workspace_path = if workspace_record_exists(metadata_dir, &workspace).await? {
            Some(read_workspace_path(metadata_dir, &workspace).await?)
        } else {
            None
        };
        workspaces.push((workspace, workspace_path));
    }

    workspaces.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    Ok(workspaces)
}

pub(crate) async fn write_workspace_pointer(
    root: &Path,
    pointer: &WorkspacePointer,
) -> Result<(), RepositoryError> {
    let path = workspace_pointer_path(root);
    write_text_file_atomic(&path, &pointer.to_pointer_file()).await
}

pub(crate) async fn read_workspace_pointer(
    root: &Path,
) -> Result<WorkspacePointer, RepositoryError> {
    let path = workspace_pointer_path(root);
    let contents = read_text_file(&path, || RepositoryError::NotRepository {
        path: path.clone(),
    })
    .await?;
    parse_workspace_pointer(&path, &contents)
}

pub(crate) async fn acquire_metadata_lock(path: PathBuf) -> Result<MetadataLock, RepositoryError> {
    let start = Instant::now();

    loop {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|source| io_error(parent.to_path_buf(), source))?;
        }

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                let contents = format!(
                    "pid: {}\ncreated_millis: {}\n",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|duration| duration.as_millis())
                        .unwrap_or_default()
                );
                file.write_all(contents.as_bytes())
                    .await
                    .map_err(|source| io_error(path.clone(), source))?;
                file.flush()
                    .await
                    .map_err(|source| io_error(path.clone(), source))?;
                drop(file);
                return Ok(MetadataLock { path });
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                maybe_remove_stale_lock(&path).await?;
                if start.elapsed() >= LOCK_WAIT_TIMEOUT {
                    return Err(RepositoryError::LockTimeout { path });
                }
                sleep(LOCK_POLL_INTERVAL).await;
            }
            Err(source) => return Err(io_error(path, source)),
        }
    }
}

pub(crate) struct MetadataLock {
    path: PathBuf,
}

#[cfg(test)]
impl MetadataLock {
    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MetadataLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn parse_head(path: &Path, contents: &str) -> Result<BranchName, RepositoryError> {
    let line = single_line(contents).ok_or_else(|| RepositoryError::InvalidHead {
        path: path.to_path_buf(),
        contents: contents.to_owned(),
    })?;
    let reference = line
        .strip_prefix(HEAD_PREFIX)
        .ok_or_else(|| RepositoryError::InvalidHead {
            path: path.to_path_buf(),
            contents: contents.to_owned(),
        })?;
    let branch =
        reference
            .strip_prefix(REFS_HEADS_PREFIX)
            .ok_or_else(|| RepositoryError::InvalidHead {
                path: path.to_path_buf(),
                contents: contents.to_owned(),
            })?;

    BranchName::new(branch).map_err(RepositoryError::from)
}

fn parse_ref(path: &Path, contents: &str) -> Result<ObjectId, RepositoryError> {
    let line = single_line(contents).ok_or_else(|| RepositoryError::InvalidRef {
        path: path.to_path_buf(),
        contents: contents.to_owned(),
    })?;

    line.parse::<ObjectId>()
        .map_err(|_| RepositoryError::InvalidRef {
            path: path.to_path_buf(),
            contents: contents.to_owned(),
        })
}

fn parse_workspace_pointer(
    path: &Path,
    contents: &str,
) -> Result<WorkspacePointer, RepositoryError> {
    let lines = contents.strip_suffix('\n').unwrap_or(contents);
    let mut lines = lines.lines();
    let Some(magic) = lines.next() else {
        return Err(RepositoryError::InvalidWorkspacePointer {
            path: path.to_path_buf(),
            contents: contents.to_owned(),
        });
    };
    let Some(metadata) = lines.next() else {
        return Err(RepositoryError::InvalidWorkspacePointer {
            path: path.to_path_buf(),
            contents: contents.to_owned(),
        });
    };
    let Some(workspace) = lines.next() else {
        return Err(RepositoryError::InvalidWorkspacePointer {
            path: path.to_path_buf(),
            contents: contents.to_owned(),
        });
    };
    if lines.next().is_some() || magic != WORKSPACE_POINTER_MAGIC {
        return Err(RepositoryError::InvalidWorkspacePointer {
            path: path.to_path_buf(),
            contents: contents.to_owned(),
        });
    }

    let metadata_dir = metadata
        .strip_prefix(POINTER_METADATA_PREFIX)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RepositoryError::InvalidWorkspacePointer {
            path: path.to_path_buf(),
            contents: contents.to_owned(),
        })?;
    let workspace_id = workspace
        .strip_prefix(POINTER_WORKSPACE_PREFIX)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RepositoryError::InvalidWorkspacePointer {
            path: path.to_path_buf(),
            contents: contents.to_owned(),
        })?;

    Ok(WorkspacePointer::new(
        PathBuf::from(metadata_dir),
        WorkspaceId::new(workspace_id)?,
    ))
}

fn single_line(contents: &str) -> Option<&str> {
    let line = contents.strip_suffix('\n').unwrap_or(contents);
    if line.contains('\n') || line.is_empty() {
        None
    } else {
        Some(line)
    }
}

async fn read_text_file(
    path: &Path,
    missing: impl FnOnce() -> RepositoryError,
) -> Result<String, RepositoryError> {
    fs::read_to_string(path)
        .await
        .map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => missing(),
            _ => io_error(path.to_path_buf(), source),
        })
}

async fn create_text_file_new(
    path: &Path,
    contents: &str,
    already_exists: impl FnOnce() -> RepositoryError,
) -> Result<(), RepositoryError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|source| io_error(parent.to_path_buf(), source))?;
    }

    let temp_path = write_temp_text_file(path, contents).await?;
    match fs::hard_link(&temp_path, path).await {
        Ok(()) => {
            remove_temp_file(&temp_path).await?;
            Ok(())
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            remove_temp_file(&temp_path).await?;
            Err(already_exists())
        }
        Err(source) => {
            let _ = fs::remove_file(&temp_path).await;
            Err(io_error(path.to_path_buf(), source))
        }
    }
}

async fn write_text_file_atomic(path: &Path, contents: &str) -> Result<(), RepositoryError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|source| io_error(parent.to_path_buf(), source))?;
    }

    let temp_path = write_temp_text_file(path, contents).await?;
    match fs::rename(&temp_path, path).await {
        Ok(()) => Ok(()),
        Err(source) => {
            let _ = fs::remove_file(&temp_path).await;
            Err(io_error(path.to_path_buf(), source))
        }
    }
}

async fn write_temp_text_file(path: &Path, contents: &str) -> Result<PathBuf, RepositoryError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for _ in 0..16 {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("ref");
        let temp_path = parent.join(format!(".{name}.{}.{counter}.tmp", std::process::id()));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .await
        {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(io_error(temp_path, source)),
        };

        if let Err(source) = file.write_all(contents.as_bytes()).await {
            let _ = fs::remove_file(&temp_path).await;
            return Err(io_error(temp_path, source));
        }
        if let Err(source) = file.flush().await {
            let _ = fs::remove_file(&temp_path).await;
            return Err(io_error(temp_path, source));
        }
        if let Err(source) = file.sync_all().await {
            let _ = fs::remove_file(&temp_path).await;
            return Err(io_error(temp_path, source));
        }
        drop(file);
        return Ok(temp_path);
    }

    Err(io_error(
        parent.to_path_buf(),
        io::Error::new(io::ErrorKind::AlreadyExists, "temporary file exhausted"),
    ))
}

async fn remove_temp_file(path: &Path) -> Result<(), RepositoryError> {
    fs::remove_file(path)
        .await
        .map_err(|source| io_error(path.to_path_buf(), source))
}

async fn maybe_remove_stale_lock(path: &Path) -> Result<(), RepositoryError> {
    let Ok(metadata) = fs::metadata(path).await else {
        return Ok(());
    };
    let Ok(modified) = metadata.modified() else {
        return Ok(());
    };
    let Ok(age) = modified.elapsed() else {
        return Ok(());
    };
    if age >= STALE_LOCK_AGE {
        match fs::remove_file(path).await {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(path.to_path_buf(), source)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_head_accepts_main_branch_ref() {
        let branch = parse_head(Path::new("HEAD"), "ref: refs/heads/main\n").unwrap();

        assert_eq!(branch.as_str(), "main");
    }

    #[test]
    fn parse_head_rejects_path_traversal_ref() {
        let error = parse_head(Path::new("HEAD"), "ref: refs/heads/../evil\n").unwrap_err();

        assert!(matches!(error, RepositoryError::InvalidBranchName { .. }));
    }

    #[test]
    fn parse_workspace_pointer_accepts_metadata_and_workspace() {
        let pointer = parse_workspace_pointer(
            Path::new(".era"),
            "era-workspace-v1\nmetadata: /tmp/project/.era\nworkspace: agent-1\n",
        )
        .unwrap();

        assert_eq!(pointer.metadata_dir, PathBuf::from("/tmp/project/.era"));
        assert_eq!(pointer.workspace_id.as_str(), "agent-1");
    }

    #[test]
    fn parse_workspace_pointer_rejects_invalid_workspace_id() {
        let error = parse_workspace_pointer(
            Path::new(".era"),
            "era-workspace-v1\nmetadata: /tmp/project/.era\nworkspace: ../evil\n",
        )
        .unwrap_err();

        assert!(matches!(error, RepositoryError::InvalidWorkspaceId { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn branch_ref_round_trips_snapshot_id() {
        let temp = TempDir::new().unwrap();
        let metadata = temp.path().join(ERA_DIR_NAME);
        create_ref_layout(&metadata).await.unwrap();
        let branch = BranchName::main();
        let id = ObjectId::from_content(b"snapshot");

        create_branch_ref(&metadata, &branch, id).await.unwrap();

        assert_eq!(read_branch_ref(&metadata, &branch).await.unwrap(), id);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn workspace_ref_and_path_round_trip() {
        let temp = TempDir::new().unwrap();
        let metadata = temp.path().join(ERA_DIR_NAME);
        create_ref_layout(&metadata).await.unwrap();
        let workspace = WorkspaceId::new("agent-1").unwrap();
        let id = ObjectId::from_content(b"snapshot");
        let path = temp.path().join("agent-1");

        create_workspace_ref(&metadata, &workspace, id)
            .await
            .unwrap();
        write_workspace_path(&metadata, &workspace, &path)
            .await
            .unwrap();

        assert_eq!(read_workspace_ref(&metadata, &workspace).await.unwrap(), id);
        assert_eq!(
            read_workspace_path(&metadata, &workspace).await.unwrap(),
            path
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn metadata_lock_serializes_create_new_lock_files() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("lock");

        let lock = acquire_metadata_lock(path.clone()).await.unwrap();
        assert!(lock.path().exists());
        drop(lock);

        let second = acquire_metadata_lock(path.clone()).await.unwrap();
        assert!(second.path().exists());
    }
}
