use super::*;
use era_core::{EntryKind, ObjectKind, SnapshotProvenance, Tree};
use era_materialization::FilesystemMaterializer;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs;

#[test]
fn branch_name_exposes_inner_value() {
    let branch = BranchName::new("main").unwrap();

    assert_eq!(branch.as_str(), "main");
    assert_eq!(branch.to_string(), "main");
    assert_eq!(branch.as_ref(), "main");
    assert_eq!("main".parse::<BranchName>().unwrap(), branch);
}

#[test]
fn branch_name_rejects_unsafe_ref_segments() {
    for name in ["", ".", "..", "feature/x", "feature\\x", "bad\0name"] {
        assert!(BranchName::new(name).is_err(), "{name:?} should be invalid");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn init_creates_metadata_layout_and_initial_snapshot() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"hello").await.unwrap();
    fs::create_dir(work.join("src")).await.unwrap();
    fs::write(work.join("src/main.rs"), b"fn main() {}\n")
        .await
        .unwrap();
    fs::create_dir(work.join(".git")).await.unwrap();
    fs::write(work.join(".git/config"), b"ignored")
        .await
        .unwrap();
    let materializer = FilesystemMaterializer::new();

    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(100),
    )
    .await
    .unwrap();
    let repo = init.repository;

    assert_eq!(repo.root(), work);
    assert!(fs::metadata(work.join(".era")).await.unwrap().is_dir());
    assert!(
        fs::metadata(work.join(".era/objects/blobs"))
            .await
            .unwrap()
            .is_dir()
    );
    assert!(
        fs::metadata(work.join(".era/objects/trees"))
            .await
            .unwrap()
            .is_dir()
    );
    assert!(
        fs::metadata(work.join(".era/objects/snapshots"))
            .await
            .unwrap()
            .is_dir()
    );
    assert_eq!(
        read_to_string(work.join(".era/HEAD")).await,
        "ref: refs/heads/main\n"
    );
    assert_eq!(
        read_to_string(work.join(".era/refs/heads/main")).await,
        format!("{}\n", init.snapshot.snapshot_id)
    );
    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        init.snapshot.snapshot_id
    );

    let snapshot = repo
        .object_store()
        .get_snapshot(&init.snapshot.snapshot_id)
        .await
        .unwrap();
    assert_eq!(snapshot, init.snapshot.snapshot);
    assert_eq!(snapshot.parents(), &[]);
    assert_eq!(snapshot.timestamp_millis(), 100);
    assert_eq!(snapshot.provenance().source(), "repository-init");

    let root_tree = repo
        .object_store()
        .get_tree(&snapshot.root_tree_id())
        .await
        .unwrap();
    assert_eq!(entry_names(&root_tree), vec!["README.md", "src"]);
    assert!(
        repo.object_store()
            .contains(ObjectKind::Snapshot, &init.snapshot.snapshot_id)
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn init_fails_when_repository_already_exists() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    let materializer = FilesystemMaterializer::new();
    Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();

    let error = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(2),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, RepositoryError::AlreadyInitialized { path } if path == work.join(".era"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn open_fails_outside_repository() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;

    let error = Repository::open(&work).await.unwrap_err();

    assert!(matches!(error, RepositoryError::NotRepository { path } if path == work.join(".era")));
}

#[tokio::test(flavor = "current_thread")]
async fn open_existing_repository_reads_current_snapshot() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"hello").await.unwrap();
    let materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();

    let repo = Repository::open(&work).await.unwrap();

    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        init.snapshot.snapshot_id
    );
    assert_eq!(repo.metadata_dir(), work.join(".era"));
}

#[tokio::test(flavor = "current_thread")]
async fn manual_snapshot_advances_current_branch_and_records_parent() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"one").await.unwrap();
    let materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = init.repository;
    fs::write(work.join("README.md"), b"two").await.unwrap();

    let second = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("manual checkpoint")
                .with_timestamp_millis(2)
                .with_author("human@example"),
        )
        .await
        .unwrap();

    assert_ne!(second.snapshot_id, init.snapshot.snapshot_id);
    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        second.snapshot_id
    );
    assert_eq!(second.snapshot.parents(), &[init.snapshot.snapshot_id]);
    assert_eq!(second.snapshot.timestamp_millis(), 2);
    assert_eq!(second.snapshot.author(), Some("human@example"));
    assert_eq!(second.snapshot.message(), Some("manual checkpoint"));
    assert_eq!(second.snapshot.provenance().source(), "manual-snapshot");

    let root_tree = repo
        .object_store()
        .get_tree(&second.snapshot.root_tree_id())
        .await
        .unwrap();
    let readme_entry = root_tree
        .entries()
        .iter()
        .find(|entry| entry.name() == "README.md")
        .unwrap();
    assert_eq!(readme_entry.kind(), EntryKind::Blob);
    assert_eq!(
        repo.object_store()
            .get_blob(&readme_entry.id())
            .await
            .unwrap(),
        b"two"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn timeline_walks_first_parent_newest_to_oldest() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("file.txt"), b"one").await.unwrap();
    let materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = init.repository;
    fs::write(work.join("file.txt"), b"two").await.unwrap();
    let second = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("second").with_timestamp_millis(2),
        )
        .await
        .unwrap();
    fs::write(work.join("file.txt"), b"three").await.unwrap();
    let third = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("third").with_timestamp_millis(3),
        )
        .await
        .unwrap();

    let timeline = repo.timeline().await.unwrap();

    assert_eq!(
        timeline
            .iter()
            .map(|entry| entry.snapshot_id)
            .collect::<Vec<_>>(),
        vec![
            third.snapshot_id,
            second.snapshot_id,
            init.snapshot.snapshot_id
        ]
    );
    assert_eq!(timeline[0].snapshot.message(), Some("third"));
    assert_eq!(timeline[1].snapshot.message(), Some("second"));
    assert_eq!(timeline[2].snapshot.parents(), &[]);
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_head_returns_clear_error() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = init.repository;
    fs::write(repo.head_path(), b"not a ref\n").await.unwrap();

    let error = repo.current_branch().await.unwrap_err();

    assert!(matches!(error, RepositoryError::InvalidHead { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_ref_returns_clear_error() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = init.repository;
    let ref_path = repo.current_branch_ref_path().await.unwrap();
    fs::write(&ref_path, b"not-an-object-id\n").await.unwrap();

    let error = repo.current_snapshot_id().await.unwrap_err();

    assert!(matches!(error, RepositoryError::InvalidRef { path, .. } if path == ref_path));
}

#[tokio::test(flavor = "current_thread")]
async fn missing_ref_returns_clear_error() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = init.repository;
    let ref_path = repo.current_branch_ref_path().await.unwrap();
    fs::remove_file(&ref_path).await.unwrap();

    let error = repo.current_snapshot_id().await.unwrap_err();

    assert!(matches!(error, RepositoryError::RefMissing { path } if path == ref_path));
}

#[tokio::test(flavor = "current_thread")]
async fn init_rejects_missing_or_file_root() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing");
    let file = temp.path().join("file");
    fs::write(&file, b"not a directory").await.unwrap();
    let materializer = FilesystemMaterializer::new();

    let missing_error = Repository::init(
        &missing,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap_err();
    let file_error = Repository::init(
        &file,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap_err();

    assert!(matches!(missing_error, RepositoryError::RootMissing { path } if path == missing));
    assert!(matches!(file_error, RepositoryError::RootNotDirectory { path } if path == file));
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_request_accepts_structured_provenance() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = init.repository;

    let result = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("agent checkpoint")
                .with_timestamp_millis(2)
                .with_provenance(
                    SnapshotProvenance::new("agent")
                        .with_attribute("model", "test-model")
                        .with_attribute("task", "T-123"),
                ),
        )
        .await
        .unwrap();

    assert_eq!(result.snapshot.provenance().source(), "agent");
    assert_eq!(
        result
            .snapshot
            .provenance()
            .attributes()
            .get("model")
            .map(String::as_str),
        Some("test-model")
    );
}

async fn create_workdir(temp: &TempDir) -> PathBuf {
    let path = temp.path().join("work");
    fs::create_dir(&path).await.unwrap();
    path
}

async fn read_to_string(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).await.unwrap()
}

fn entry_names(tree: &Tree) -> Vec<&str> {
    tree.entries().iter().map(|entry| entry.name()).collect()
}
