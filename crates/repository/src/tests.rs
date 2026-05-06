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
    assert!(
        fs::metadata(work.join(".era/index/snapshots"))
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
    assert!(
        fs::metadata(work.join(format!(
            ".era/index/snapshots/{}/{}",
            init.snapshot.snapshot_id.shard_prefix(),
            init.snapshot.snapshot_id
        )))
        .await
        .unwrap()
        .is_file()
    );
    assert!(
        fs::metadata(work.join(".era/index/snapshots/.complete-v1"))
            .await
            .unwrap()
            .is_file()
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
async fn working_tree_status_detects_clean_and_dirty_states() {
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

    let clean = repo.working_tree_status(&materializer).await.unwrap();
    assert!(clean.is_clean());
    assert_eq!(clean.snapshot_id, init.snapshot.snapshot_id);

    fs::write(work.join("README.md"), b"two").await.unwrap();
    let dirty = repo.working_tree_status(&materializer).await.unwrap();
    assert!(!dirty.is_clean());
    assert_ne!(dirty.current_root_tree_id, dirty.snapshot.root_tree_id());
    assert_eq!(dirty.changes(), &[TreeChange::modified("README.md")]);

    repo.snapshot(
        &materializer,
        SnapshotRequest::manual("saved").with_timestamp_millis(2),
    )
    .await
    .unwrap();
    assert!(
        repo.working_tree_status(&materializer)
            .await
            .unwrap()
            .is_clean()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_if_changed_skips_clean_trees_and_records_auto_provenance() {
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

    let clean = repo
        .snapshot_if_changed(
            &materializer,
            SnapshotRequest::automatic_for_trigger(AutoSnapshotTrigger::Reconcile)
                .with_timestamp_millis(2),
        )
        .await
        .unwrap();
    assert!(clean.is_none());
    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        init.snapshot.snapshot_id
    );

    fs::write(work.join("README.md"), b"two").await.unwrap();
    let saved = repo
        .snapshot_if_changed(
            &materializer,
            SnapshotRequest::automatic_for_trigger(AutoSnapshotTrigger::Watch)
                .with_timestamp_millis(3)
                .with_provenance_attribute("workspace", "agent-1")
                .with_provenance_attribute("agent", "claude")
                .with_provenance_attribute("task", "fix-parser"),
        )
        .await
        .unwrap()
        .expect("dirty tree should be saved");

    assert_eq!(repo.current_snapshot_id().await.unwrap(), saved.snapshot_id);
    assert_eq!(saved.snapshot.parents(), &[init.snapshot.snapshot_id]);
    assert_eq!(saved.snapshot.message(), None);
    assert_eq!(saved.snapshot.provenance().source(), "auto-snapshot");
    assert_eq!(
        saved
            .snapshot
            .provenance()
            .attributes()
            .get("trigger")
            .map(String::as_str),
        Some("watch")
    );
    assert_eq!(
        saved
            .snapshot
            .provenance()
            .attributes()
            .get("workspace")
            .map(String::as_str),
        Some("agent-1")
    );
    assert_eq!(
        saved
            .snapshot
            .provenance()
            .attributes()
            .get("agent")
            .map(String::as_str),
        Some("claude")
    );
    assert_eq!(
        saved
            .snapshot
            .provenance()
            .attributes()
            .get("task")
            .map(String::as_str),
        Some("fix-parser")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn branch_create_switch_and_restore_cover_local_workflows() {
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
    let feature = BranchName::new("feature").unwrap();

    let branch = repo
        .create_branch(&materializer, feature.clone())
        .await
        .unwrap();
    assert_eq!(branch.snapshot_id, init.snapshot.snapshot_id);
    assert!(branch.saved_snapshot.is_none());
    assert_eq!(repo.current_branch().await.unwrap(), BranchName::main());
    assert_eq!(
        repo.branches()
            .await
            .unwrap()
            .iter()
            .map(|branch| branch.name.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec!["feature".to_owned(), "main".to_owned()]
    );

    fs::write(work.join("README.md"), b"two").await.unwrap();
    let main_tip = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("main two").with_timestamp_millis(2),
        )
        .await
        .unwrap();

    let switched = repo
        .switch_branch(&materializer, feature.clone())
        .await
        .unwrap();
    assert_eq!(switched.branch, feature);
    assert_eq!(repo.current_branch().await.unwrap().as_str(), "feature");
    assert_eq!(fs::read(work.join("README.md")).await.unwrap(), b"one");

    fs::write(work.join("README.md"), b"feature work")
        .await
        .unwrap();
    let switched = repo
        .switch_branch(&materializer, BranchName::main())
        .await
        .unwrap();
    assert!(switched.saved_snapshot.is_some());
    assert_eq!(switched.snapshot_id, main_tip.snapshot_id);
    assert_eq!(repo.current_branch().await.unwrap(), BranchName::main());
    assert_eq!(fs::read(work.join("README.md")).await.unwrap(), b"two");

    let restored = repo.restore(&materializer, "main two").await.unwrap();
    assert_eq!(restored.snapshot_id, main_tip.snapshot_id);
    assert_eq!(fs::read(work.join("README.md")).await.unwrap(), b"two");
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_targets_resolve_full_ids_prefixes_and_messages() {
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
            SnapshotRequest::manual("second label").with_timestamp_millis(2),
        )
        .await
        .unwrap();
    let prefix = second
        .snapshot_id
        .to_hex()
        .chars()
        .take(12)
        .collect::<String>();

    assert_eq!(
        repo.resolve_snapshot_target(&second.snapshot_id.to_string())
            .await
            .unwrap()
            .snapshot_id,
        second.snapshot_id
    );
    assert_eq!(
        repo.resolve_snapshot_target(&prefix)
            .await
            .unwrap()
            .snapshot_id,
        second.snapshot_id
    );
    assert_eq!(
        repo.resolve_snapshot_target("second label")
            .await
            .unwrap()
            .snapshot_id,
        second.snapshot_id
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restore_auto_saves_dirty_work_before_materializing_target() {
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
    let saved_two = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("two").with_timestamp_millis(2),
        )
        .await
        .unwrap();
    fs::write(work.join("README.md"), b"three").await.unwrap();

    let restored = repo.restore(&materializer, "two").await.unwrap();

    assert_eq!(restored.snapshot_id, saved_two.snapshot_id);
    assert_eq!(restored.cursor, CursorInfo::Branch(BranchName::main()));
    let safety_snapshot = restored.saved_snapshot.as_ref().expect("dirty work saved");
    assert_eq!(safety_snapshot.snapshot.parents(), &[saved_two.snapshot_id]);
    assert_eq!(fs::read(work.join("README.md")).await.unwrap(), b"two");
    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        saved_two.snapshot_id
    );
    assert!(
        repo.working_tree_status(&materializer)
            .await
            .unwrap()
            .is_clean()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restore_moves_cursor_and_next_snapshot_branches_from_restored_target() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"one").await.unwrap();
    let materializer = FilesystemMaterializer::new();
    Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = Repository::open(&work).await.unwrap();

    fs::write(work.join("README.md"), b"two").await.unwrap();
    let second = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("two").with_timestamp_millis(2),
        )
        .await
        .unwrap();
    fs::write(work.join("README.md"), b"three").await.unwrap();
    let third = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("three").with_timestamp_millis(3),
        )
        .await
        .unwrap();

    let restored = repo.restore(&materializer, "two").await.unwrap();
    assert_eq!(restored.snapshot_id, second.snapshot_id);
    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        second.snapshot_id
    );

    fs::write(work.join("README.md"), b"side").await.unwrap();
    let side = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("side").with_timestamp_millis(4),
        )
        .await
        .unwrap();

    assert_eq!(side.snapshot.parents(), &[second.snapshot_id]);
    assert_eq!(repo.current_snapshot_id().await.unwrap(), side.snapshot_id);
    let graph = repo.snapshot_graph().await.unwrap();
    let ids = graph
        .entries
        .iter()
        .map(|entry| entry.snapshot_id)
        .collect::<Vec<_>>();
    assert!(ids.contains(&third.snapshot_id));
    assert!(ids.contains(&side.snapshot_id));
}

#[tokio::test(flavor = "current_thread")]
async fn restore_moves_workspace_cursor() {
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
            SnapshotRequest::manual("two").with_timestamp_millis(2),
        )
        .await
        .unwrap();
    let workspace_path = temp.path().join("agent");
    repo.add_workspace(
        &materializer,
        AddWorkspaceOptions {
            path: workspace_path.clone(),
            workspace_id: WorkspaceId::new("agent").unwrap(),
            from: Some(second.snapshot_id.to_hex()),
        },
    )
    .await
    .unwrap();
    let workspace_repo = Repository::open(&workspace_path).await.unwrap();
    fs::write(workspace_path.join("README.md"), b"agent work")
        .await
        .unwrap();
    let agent = workspace_repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("agent").with_timestamp_millis(3),
        )
        .await
        .unwrap();

    let restored = workspace_repo.restore(&materializer, "two").await.unwrap();

    assert_eq!(restored.snapshot_id, second.snapshot_id);
    assert_eq!(
        restored.cursor,
        CursorInfo::Workspace(WorkspaceId::new("agent").unwrap())
    );
    assert_eq!(
        workspace_repo.current_snapshot_id().await.unwrap(),
        second.snapshot_id
    );
    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        second.snapshot_id
    );
    let graph = repo.snapshot_graph().await.unwrap();
    assert!(
        graph
            .entries
            .iter()
            .any(|entry| entry.snapshot_id == agent.snapshot_id)
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
async fn snapshot_graph_includes_branch_and_workspace_futures() {
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

    fs::write(work.join("file.txt"), b"main").await.unwrap();
    let main = repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("main").with_timestamp_millis(2),
        )
        .await
        .unwrap();

    let workspace_path = temp.path().join("agent");
    repo.add_workspace(
        &materializer,
        AddWorkspaceOptions {
            path: workspace_path.clone(),
            workspace_id: WorkspaceId::new("agent").unwrap(),
            from: Some(init.snapshot.snapshot_id.to_hex()),
        },
    )
    .await
    .unwrap();
    let workspace_repo = Repository::open(&workspace_path).await.unwrap();
    fs::write(workspace_path.join("file.txt"), b"agent")
        .await
        .unwrap();
    let agent = workspace_repo
        .snapshot(
            &materializer,
            SnapshotRequest::manual("agent").with_timestamp_millis(3),
        )
        .await
        .unwrap();

    let graph = repo.snapshot_graph().await.unwrap();
    let ids = graph
        .entries
        .iter()
        .map(|entry| entry.snapshot_id)
        .collect::<Vec<_>>();

    assert!(ids.contains(&init.snapshot.snapshot_id));
    assert!(ids.contains(&main.snapshot_id));
    assert!(ids.contains(&agent.snapshot_id));
    assert_eq!(graph.branches[0].snapshot_id, main.snapshot_id);
    assert_eq!(graph.workspaces[0].snapshot_id, agent.snapshot_id);
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_graph_rebuilds_missing_index_from_snapshot_objects() {
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
    fs::remove_dir_all(work.join(".era/index")).await.unwrap();

    let graph = repo.snapshot_graph().await.unwrap();

    let ids = graph
        .entries
        .iter()
        .map(|entry| entry.snapshot_id)
        .collect::<Vec<_>>();
    assert!(ids.contains(&init.snapshot.snapshot_id));
    assert!(ids.contains(&second.snapshot_id));
    assert!(
        fs::metadata(work.join(".era/index/snapshots/.complete-v1"))
            .await
            .unwrap()
            .is_file()
    );
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
async fn workspace_add_materializes_external_path_and_snapshots_independently() {
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
    let workspace_path = temp.path().join("agent-1");
    let workspace_id = WorkspaceId::new("agent-1").unwrap();

    let added = repo
        .add_workspace(
            &materializer,
            AddWorkspaceOptions {
                path: workspace_path.clone(),
                workspace_id: workspace_id.clone(),
                from: None,
            },
        )
        .await
        .unwrap();

    assert!(added.created);
    assert!(added.materialized);
    assert_eq!(added.snapshot_id, init.snapshot.snapshot_id);
    assert_eq!(
        fs::read(workspace_path.join("README.md")).await.unwrap(),
        b"one"
    );
    assert!(
        fs::metadata(workspace_path.join(".era"))
            .await
            .unwrap()
            .is_file()
    );

    let workspace_repo = Repository::open(&workspace_path).await.unwrap();
    assert_eq!(workspace_repo.workspace_id(), Some(&workspace_id));
    assert_eq!(workspace_repo.metadata_dir(), repo.metadata_dir());
    assert_eq!(
        workspace_repo.current_snapshot_id().await.unwrap(),
        init.snapshot.snapshot_id
    );

    fs::write(workspace_path.join("README.md"), b"agent work")
        .await
        .unwrap();
    let saved = workspace_repo
        .snapshot_if_changed(
            &materializer,
            SnapshotRequest::manual_unlabeled().with_timestamp_millis(2),
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(saved.snapshot.parents(), &[init.snapshot.snapshot_id]);
    assert_eq!(
        workspace_repo.current_snapshot_id().await.unwrap(),
        saved.snapshot_id
    );
    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        init.snapshot.snapshot_id
    );
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_add_adopts_non_empty_directory_without_overwriting() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"base").await.unwrap();
    let materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = init.repository;
    let workspace_path = temp.path().join("agent-existing");
    fs::create_dir(&workspace_path).await.unwrap();
    fs::write(workspace_path.join("README.md"), b"already changed")
        .await
        .unwrap();

    let added = repo
        .add_workspace(
            &materializer,
            AddWorkspaceOptions {
                path: workspace_path.clone(),
                workspace_id: WorkspaceId::new("agent-existing").unwrap(),
                from: None,
            },
        )
        .await
        .unwrap();

    assert!(added.created);
    assert!(!added.materialized);
    assert!(added.materialization.is_none());
    assert_eq!(
        fs::read(workspace_path.join("README.md")).await.unwrap(),
        b"already changed"
    );

    let workspace_repo = Repository::open(&workspace_path).await.unwrap();
    let status = workspace_repo
        .working_tree_status(&materializer)
        .await
        .unwrap();
    assert!(!status.is_clean());
    let saved = workspace_repo
        .snapshot_if_changed(
            &materializer,
            SnapshotRequest::manual_unlabeled().with_timestamp_millis(2),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.snapshot.parents(), &[init.snapshot.snapshot_id]);
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_add_rejects_nested_workspace_paths() {
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
    let nested = work.join("agent-1");

    let error = repo
        .add_workspace(
            &materializer,
            AddWorkspaceOptions {
                path: nested.clone(),
                workspace_id: WorkspaceId::new("agent-1").unwrap(),
                from: None,
            },
        )
        .await
        .unwrap_err();

    let expected_nested = fs::canonicalize(&work).await.unwrap().join("agent-1");
    let expected_root = fs::canonicalize(&work).await.unwrap();
    assert!(matches!(
        error,
        RepositoryError::WorkspaceInsideWorkspace { path, workspace_root }
            if path == expected_nested && workspace_root == expected_root
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_add_rejects_paths_nested_inside_registered_workspaces() {
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
    let workspace_path = temp.path().join("agent-outer");
    repo.add_workspace(
        &materializer,
        AddWorkspaceOptions {
            path: workspace_path.clone(),
            workspace_id: WorkspaceId::new("agent-outer").unwrap(),
            from: None,
        },
    )
    .await
    .unwrap();
    let nested = workspace_path.join("nested");

    let error = repo
        .add_workspace(
            &materializer,
            AddWorkspaceOptions {
                path: nested.clone(),
                workspace_id: WorkspaceId::new("nested").unwrap(),
                from: None,
            },
        )
        .await
        .unwrap_err();

    let expected_root = fs::canonicalize(&workspace_path).await.unwrap();
    assert!(matches!(
        error,
        RepositoryError::WorkspaceInsideWorkspace { path, workspace_root }
            if path == expected_root.join("nested") && workspace_root == expected_root
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_snapshots_on_same_workspace_serialize_and_collapse_duplicates() {
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
    let workspace_path = temp.path().join("agent-race");
    init.repository
        .add_workspace(
            &materializer,
            AddWorkspaceOptions {
                path: workspace_path.clone(),
                workspace_id: WorkspaceId::new("agent-race").unwrap(),
                from: None,
            },
        )
        .await
        .unwrap();
    fs::write(workspace_path.join("README.md"), b"two")
        .await
        .unwrap();

    let mut handles = Vec::new();
    for index in 0..16 {
        let path = workspace_path.clone();
        handles.push(tokio::spawn(async move {
            let repo = Repository::open(path).await.unwrap();
            let materializer = FilesystemMaterializer::new();
            repo.snapshot_if_changed(
                &materializer,
                SnapshotRequest::manual_unlabeled().with_timestamp_millis(10 + index),
            )
            .await
            .unwrap()
        }));
    }

    let mut saved = Vec::new();
    for handle in handles {
        if let Some(snapshot) = handle.await.unwrap() {
            saved.push(snapshot);
        }
    }

    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].snapshot.parents(), &[init.snapshot.snapshot_id]);
    let workspace_repo = Repository::open(&workspace_path).await.unwrap();
    assert_eq!(workspace_repo.timeline().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_snapshots_on_different_workspaces_advance_independent_cursors() {
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
    let mut workspace_paths = Vec::new();
    for index in 0..6 {
        let id = WorkspaceId::new(format!("agent-{index}")).unwrap();
        let path = temp.path().join(format!("agent-{index}"));
        repo.add_workspace(
            &materializer,
            AddWorkspaceOptions {
                path: path.clone(),
                workspace_id: id,
                from: None,
            },
        )
        .await
        .unwrap();
        fs::write(path.join("README.md"), format!("agent {index}"))
            .await
            .unwrap();
        workspace_paths.push(path);
    }

    let mut handles = Vec::new();
    for (index, path) in workspace_paths.iter().cloned().enumerate() {
        handles.push(tokio::spawn(async move {
            let repo = Repository::open(path).await.unwrap();
            let materializer = FilesystemMaterializer::new();
            repo.snapshot_if_changed(
                &materializer,
                SnapshotRequest::manual_unlabeled().with_timestamp_millis(20 + index as u64),
            )
            .await
            .unwrap()
            .unwrap()
        }));
    }

    let mut snapshots = Vec::new();
    for handle in handles {
        snapshots.push(handle.await.unwrap());
    }

    assert_eq!(snapshots.len(), workspace_paths.len());
    assert_eq!(
        repo.current_snapshot_id().await.unwrap(),
        init.snapshot.snapshot_id
    );
    for (snapshot, path) in snapshots.iter().zip(workspace_paths) {
        assert_eq!(snapshot.snapshot.parents(), &[init.snapshot.snapshot_id]);
        let workspace_repo = Repository::open(path).await.unwrap();
        assert_eq!(
            workspace_repo.current_snapshot_id().await.unwrap(),
            snapshot.snapshot_id
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn hinted_snapshot_uses_workspace_capture_cache() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"one").await.unwrap();
    let init_materializer = FilesystemMaterializer::new();
    let init = Repository::init(
        &work,
        &init_materializer,
        SnapshotRequest::initial().with_timestamp_millis(1),
    )
    .await
    .unwrap();
    let repo = init.repository;
    assert_eq!(
        repo.capture_cache_path(),
        work.join(".era/workspaces/default/cache/capture-v2.redb")
    );

    fs::write(work.join("README.md"), b"two").await.unwrap();
    let materializer = FilesystemMaterializer::with_cache_path(repo.capture_cache_path());
    let first = repo
        .snapshot_if_changed(
            &materializer,
            SnapshotRequest::manual_unlabeled().with_timestamp_millis(2),
        )
        .await
        .unwrap()
        .expect("first edit should be saved");

    fs::write(work.join("README.md"), b"three").await.unwrap();
    let materializer = FilesystemMaterializer::with_cache_path(repo.capture_cache_path());
    let second = repo
        .snapshot_if_changed_with_hints(
            &materializer,
            SnapshotRequest::manual_unlabeled().with_timestamp_millis(3),
            &[PathBuf::from("README.md")],
        )
        .await
        .unwrap()
        .expect("hinted edit should be saved");

    assert_eq!(first.snapshot.parents(), &[init.snapshot.snapshot_id]);
    assert_eq!(second.snapshot.parents(), &[first.snapshot_id]);
    assert_eq!(second.capture.stats.files_seen, 1);
    assert_eq!(second.capture.stats.bytes_read, b"three".len() as u64);
    assert_eq!(second.capture.stats.blobs_stored, 1);
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
