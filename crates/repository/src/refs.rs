use crate::{BranchName, RepositoryError, error::io_error};
use era_core::ObjectId;
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::{fs, io::AsyncWriteExt};

pub(crate) const ERA_DIR_NAME: &str = ".era";
pub(crate) const OBJECTS_DIR_NAME: &str = "objects";
const HEAD_FILE_NAME: &str = "HEAD";
const REFS_DIR_NAME: &str = "refs";
const HEADS_DIR_NAME: &str = "heads";
const HEAD_PREFIX: &str = "ref: ";
const REFS_HEADS_PREFIX: &str = "refs/heads/";

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

pub(crate) async fn create_ref_layout(metadata_dir: &Path) -> Result<(), RepositoryError> {
    let heads_dir = branch_refs_dir(metadata_dir);
    fs::create_dir_all(&heads_dir)
        .await
        .map_err(|source| io_error(heads_dir, source))
}

pub(crate) async fn write_head(
    metadata_dir: &Path,
    branch: &BranchName,
) -> Result<(), RepositoryError> {
    let path = head_path(metadata_dir);
    let contents = format!("{HEAD_PREFIX}{REFS_HEADS_PREFIX}{}\n", branch.as_str());
    write_text_file(&path, &contents).await
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|source| io_error(parent.to_path_buf(), source))?;
    }

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(|source| match source.kind() {
            io::ErrorKind::AlreadyExists => RepositoryError::BranchAlreadyExists {
                name: branch.clone(),
            },
            _ => io_error(path.clone(), source),
        })?;
    file.write_all(format!("{snapshot_id}\n").as_bytes())
        .await
        .map_err(|source| io_error(path.clone(), source))
}

pub(crate) async fn write_branch_ref(
    metadata_dir: &Path,
    branch: &BranchName,
    snapshot_id: ObjectId,
) -> Result<(), RepositoryError> {
    let path = branch_ref_path(metadata_dir, branch);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|source| io_error(parent.to_path_buf(), source))?;
    }
    write_text_file(&path, &format!("{snapshot_id}\n")).await
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

async fn write_text_file(path: &Path, contents: &str) -> Result<(), RepositoryError> {
    fs::write(path, contents)
        .await
        .map_err(|source| io_error(path.to_path_buf(), source))
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

    #[tokio::test(flavor = "current_thread")]
    async fn branch_ref_round_trips_snapshot_id() {
        let temp = TempDir::new().unwrap();
        let metadata = temp.path().join(ERA_DIR_NAME);
        create_ref_layout(&metadata).await.unwrap();
        let branch = BranchName::main();
        let id = ObjectId::from_content(b"snapshot");

        write_branch_ref(&metadata, &branch, id).await.unwrap();

        assert_eq!(read_branch_ref(&metadata, &branch).await.unwrap(), id);
    }
}
