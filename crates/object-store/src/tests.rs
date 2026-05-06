use crate::local::object_dir;
use crate::{LocalObjectStore, ObjectStore, ObjectStoreError};
use era_core::{
    EntryKind, ObjectId, ObjectKind, Snapshot, SnapshotError, SnapshotProvenance, Tree, TreeEntry,
    TreeError,
};
use std::{io, path::Path};
use tempfile::TempDir;
use tokio::fs;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[tokio::test(flavor = "current_thread")]
async fn open_creates_store_directories() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("objects");

    let store = LocalObjectStore::open(&root).await.unwrap();

    assert_eq!(store.root(), root);
    assert!(fs::metadata(root.join("blobs")).await.unwrap().is_dir());
    assert!(fs::metadata(root.join("trees")).await.unwrap().is_dir());
    assert!(fs::metadata(root.join("snapshots")).await.unwrap().is_dir());
}

#[tokio::test(flavor = "current_thread")]
async fn put_blob_round_trips_bytes() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();

    let id = store.put_blob(b"hello").await.unwrap();

    assert_eq!(store.get_blob(&id).await.unwrap(), b"hello");
}

#[tokio::test(flavor = "current_thread")]
async fn empty_blob_round_trips() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();

    let id = store.put_blob([]).await.unwrap();

    assert_eq!(id, ObjectId::from_content([]));
    assert_eq!(store.get_blob(&id).await.unwrap(), b"");
}

#[tokio::test(flavor = "current_thread")]
async fn identical_blobs_share_an_object_file() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();

    let first = store.put_blob(b"same").await.unwrap();
    let second = store.put_blob(b"same").await.unwrap();

    assert_eq!(first, second);
    assert_eq!(object_file_count(store.root(), ObjectKind::Blob).await, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn different_blobs_have_different_ids() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();

    let first = store.put_blob(b"first").await.unwrap();
    let second = store.put_blob(b"second").await.unwrap();

    assert_ne!(first, second);
}

#[tokio::test(flavor = "current_thread")]
async fn missing_blob_returns_clear_error() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = ObjectId::from_content(b"missing");

    let error = store.get_blob(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::MissingObject {
            kind: ObjectKind::Blob,
            id: found,
            ..
        } if found == id
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn corrupted_blob_fails_integrity_check() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = store.put_blob(b"hello").await.unwrap();
    fs::write(store.blob_path(&id), b"corrupt").await.unwrap();

    let error = store.get_blob(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::HashMismatch {
            kind: ObjectKind::Blob,
            expected,
            actual,
            ..
        } if expected == id && actual == ObjectId::from_content(b"corrupt")
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn put_blob_refuses_to_overwrite_corrupt_existing_blob() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = store.put_blob(b"hello").await.unwrap();
    fs::write(store.blob_path(&id), b"corrupt").await.unwrap();

    let error = store.put_blob(b"hello").await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::HashMismatch {
            kind: ObjectKind::Blob,
            expected,
            ..
        } if expected == id
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn contains_blob_checks_integrity() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = store.put_blob(b"hello").await.unwrap();

    assert!(store.contains(ObjectKind::Blob, &id).await.unwrap());

    fs::write(store.blob_path(&id), b"corrupt").await.unwrap();
    assert!(matches!(
        store.contains(ObjectKind::Blob, &id).await.unwrap_err(),
        ObjectStoreError::HashMismatch {
            kind: ObjectKind::Blob,
            ..
        }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn blob_path_is_sharded_by_first_hex_byte() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = ObjectId::from_content(b"hello");
    let hex = id.to_string();

    assert_eq!(
        store.blob_path(&id),
        store.root().join("blobs").join(&hex[..2]).join(hex)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn put_tree_round_trips_tree() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let tree = sample_tree();

    let id = store.put_tree(&tree).await.unwrap();

    assert_eq!(id, tree.id());
    assert_eq!(store.get_tree(&id).await.unwrap(), tree);
}

#[tokio::test(flavor = "current_thread")]
async fn identical_trees_share_an_object_file() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let tree = sample_tree();

    let first = store.put_tree(&tree).await.unwrap();
    let second = store.put_tree(&tree).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(object_file_count(store.root(), ObjectKind::Tree).await, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn different_trees_have_different_ids() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();

    let first = store.put_tree(&sample_tree()).await.unwrap();
    let second = store.put_tree(&Tree::empty()).await.unwrap();

    assert_ne!(first, second);
}

#[tokio::test(flavor = "current_thread")]
async fn missing_tree_returns_clear_error() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = Tree::empty().id();

    let error = store.get_tree(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::MissingObject {
            kind: ObjectKind::Tree,
            id: found,
            ..
        } if found == id
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn corrupted_tree_fails_integrity_check() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let tree = sample_tree();
    let id = store.put_tree(&tree).await.unwrap();
    fs::write(store.tree_path(&id), b"corrupt").await.unwrap();

    let error = store.get_tree(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::HashMismatch {
            kind: ObjectKind::Tree,
            expected,
            actual,
            ..
        } if expected == id && actual == ObjectId::from_content(b"corrupt")
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn non_canonical_tree_object_is_rejected() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let bytes = encode_tree_entries_unchecked(&[
        (EntryKind::Blob, "z.txt", ObjectId::from_content(b"z")),
        (EntryKind::Blob, "a.txt", ObjectId::from_content(b"a")),
    ]);
    let id = ObjectId::from_content(&bytes);
    let path = store.tree_path(&id);
    fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    fs::write(&path, bytes).await.unwrap();

    let error = store.get_tree(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::InvalidTreeObject {
            id: found,
            source: TreeError::NonCanonicalOrder { .. },
            ..
        } if found == id
    ));
    assert!(matches!(
        store.contains(ObjectKind::Tree, &id).await.unwrap_err(),
        ObjectStoreError::InvalidTreeObject { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn tree_path_is_sharded_by_first_hex_byte() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = sample_tree().id();
    let hex = id.to_string();

    assert_eq!(
        store.tree_path(&id),
        store.root().join("trees").join(&hex[..2]).join(hex)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn put_snapshot_round_trips_snapshot() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let snapshot = sample_snapshot();

    let id = store.put_snapshot(&snapshot).await.unwrap();

    assert_eq!(id, snapshot.id());
    assert_eq!(store.get_snapshot(&id).await.unwrap(), snapshot);
    assert!(store.contains(ObjectKind::Snapshot, &id).await.unwrap());
}

#[tokio::test(flavor = "current_thread")]
async fn identical_snapshots_share_an_object_file() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let snapshot = sample_snapshot();

    let first = store.put_snapshot(&snapshot).await.unwrap();
    let second = store.put_snapshot(&snapshot).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(
        object_file_count(store.root(), ObjectKind::Snapshot).await,
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn list_snapshot_ids_returns_stored_snapshot_objects() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let first_snapshot = sample_snapshot();
    let first = store.put_snapshot(&first_snapshot).await.unwrap();
    let second_snapshot = Snapshot::new(
        sample_tree().id(),
        vec![first],
        2,
        None,
        Some("second".to_owned()),
        SnapshotProvenance::manual(),
    );
    let second = store.put_snapshot(&second_snapshot).await.unwrap();
    let temp_path = store
        .root()
        .join("snapshots")
        .join(first.shard_prefix())
        .join(format!(".{first}.tmp"));
    fs::write(temp_path, b"temporary write").await.unwrap();

    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(store.list_snapshot_ids().await.unwrap(), expected);
}

#[tokio::test(flavor = "current_thread")]
async fn missing_snapshot_returns_clear_error() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = sample_snapshot().id();

    let error = store.get_snapshot(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::MissingObject {
            kind: ObjectKind::Snapshot,
            id: found,
            ..
        } if found == id
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn corrupted_snapshot_fails_integrity_check() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let snapshot = sample_snapshot();
    let id = store.put_snapshot(&snapshot).await.unwrap();
    fs::write(store.snapshot_path(&id), b"corrupt")
        .await
        .unwrap();

    let error = store.get_snapshot(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::HashMismatch {
            kind: ObjectKind::Snapshot,
            expected,
            actual,
            ..
        } if expected == id && actual == ObjectId::from_content(b"corrupt")
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn non_canonical_snapshot_object_is_rejected() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let bytes = encode_snapshot_attrs_unchecked(&[("z", "last"), ("a", "first")]);
    let id = ObjectId::from_content(&bytes);
    let path = store.snapshot_path(&id);
    fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    fs::write(&path, bytes).await.unwrap();

    let error = store.get_snapshot(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::InvalidSnapshotObject {
            id: found,
            source: SnapshotError::NonCanonicalProvenanceAttributeOrder { .. },
            ..
        } if found == id
    ));
    assert!(matches!(
        store.contains(ObjectKind::Snapshot, &id).await.unwrap_err(),
        ObjectStoreError::InvalidSnapshotObject { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_path_is_sharded_by_first_hex_byte() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = sample_snapshot().id();
    let hex = id.to_string();

    assert_eq!(
        store.snapshot_path(&id),
        store.root().join("snapshots").join(&hex[..2]).join(hex)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn object_store_trait_object_round_trips_all_object_kinds() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let store: &dyn ObjectStore = &store;
    let tree = sample_tree();
    let snapshot = sample_snapshot();

    let blob_id = store.put_blob(b"via trait").await.unwrap();
    let tree_id = store.put_tree(&tree).await.unwrap();
    let snapshot_id = store.put_snapshot(&snapshot).await.unwrap();

    assert_eq!(store.get_blob(&blob_id).await.unwrap(), b"via trait");
    assert_eq!(store.get_tree(&tree_id).await.unwrap(), tree);
    assert_eq!(store.get_snapshot(&snapshot_id).await.unwrap(), snapshot);
    assert!(store.contains(ObjectKind::Blob, &blob_id).await.unwrap());
    assert!(store.contains(ObjectKind::Tree, &tree_id).await.unwrap());
    assert!(
        store
            .contains(ObjectKind::Snapshot, &snapshot_id)
            .await
            .unwrap()
    );
    assert_eq!(store.list_snapshot_ids().await.unwrap(), vec![snapshot_id]);
}

#[tokio::test(flavor = "current_thread")]
async fn large_blob_round_trips() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let bytes: Vec<u8> = (0..(4 * 1024 * 1024))
        .map(|index| (index % 251) as u8)
        .collect();

    let id = store.put_blob(&bytes).await.unwrap();

    assert_eq!(id, ObjectId::from_content(&bytes));
    assert_eq!(store.get_blob(&id).await.unwrap(), bytes);
    assert_eq!(object_file_count(store.root(), ObjectKind::Blob).await, 1);
    assert_eq!(temp_file_count(store.root(), ObjectKind::Blob).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_puts_of_same_blob_dedupe_and_cleanup_temps() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let expected = ObjectId::from_content(b"concurrent");

    let mut handles = Vec::new();
    for _ in 0..64 {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            store.put_blob(b"concurrent").await.unwrap()
        }));
    }

    for handle in handles {
        assert_eq!(handle.await.unwrap(), expected);
    }

    assert_eq!(store.get_blob(&expected).await.unwrap(), b"concurrent");
    assert_eq!(object_file_count(store.root(), ObjectKind::Blob).await, 1);
    assert_eq!(temp_file_count(store.root(), ObjectKind::Blob).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_puts_of_distinct_blobs_all_round_trip() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let payloads: Vec<Vec<u8>> = (0..32)
        .map(|index| format!("distinct blob {index}").into_bytes())
        .collect();

    let mut handles = Vec::new();
    for payload in payloads.clone() {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            let id = store.put_blob(&payload).await.unwrap();
            (id, payload)
        }));
    }

    for handle in handles {
        let (id, payload) = handle.await.unwrap();
        assert_eq!(id, ObjectId::from_content(&payload));
        assert_eq!(store.get_blob(&id).await.unwrap(), payload);
    }

    assert_eq!(
        object_file_count(store.root(), ObjectKind::Blob).await,
        payloads.len()
    );
    assert_eq!(temp_file_count(store.root(), ObjectKind::Blob).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_puts_of_same_tree_dedupe_and_cleanup_temps() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let tree = sample_tree();
    let expected = tree.id();

    let mut handles = Vec::new();
    for _ in 0..64 {
        let store = store.clone();
        let tree = tree.clone();
        handles.push(tokio::spawn(
            async move { store.put_tree(&tree).await.unwrap() },
        ));
    }

    for handle in handles {
        assert_eq!(handle.await.unwrap(), expected);
    }

    assert_eq!(store.get_tree(&expected).await.unwrap(), tree);
    assert_eq!(object_file_count(store.root(), ObjectKind::Tree).await, 1);
    assert_eq!(temp_file_count(store.root(), ObjectKind::Tree).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_puts_of_same_snapshot_dedupe_and_cleanup_temps() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let snapshot = sample_snapshot();
    let expected = snapshot.id();

    let mut handles = Vec::new();
    for _ in 0..64 {
        let store = store.clone();
        let snapshot = snapshot.clone();
        handles.push(tokio::spawn(async move {
            store.put_snapshot(&snapshot).await.unwrap()
        }));
    }

    for handle in handles {
        assert_eq!(handle.await.unwrap(), expected);
    }

    assert_eq!(store.get_snapshot(&expected).await.unwrap(), snapshot);
    assert_eq!(
        object_file_count(store.root(), ObjectKind::Snapshot).await,
        1
    );
    assert_eq!(temp_file_count(store.root(), ObjectKind::Snapshot).await, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn open_fails_when_root_path_is_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("objects");
    fs::write(&root, b"not a directory").await.unwrap();

    let error = LocalObjectStore::open(&root).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::Io { path, source }
            if path == root.join("blobs")
                && matches!(
                    source.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::NotADirectory
                )
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn open_fails_when_blobs_path_is_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("objects");
    fs::create_dir(&root).await.unwrap();
    let blobs = root.join("blobs");
    fs::write(&blobs, b"not a directory").await.unwrap();

    let error = LocalObjectStore::open(&root).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::Io { path, source }
            if path == blobs
                && matches!(
                    source.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::NotADirectory
                )
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn open_fails_when_trees_path_is_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("objects");
    fs::create_dir(&root).await.unwrap();
    fs::create_dir(root.join("blobs")).await.unwrap();
    let trees = root.join("trees");
    fs::write(&trees, b"not a directory").await.unwrap();

    let error = LocalObjectStore::open(&root).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::Io { path, source }
            if path == trees
                && matches!(
                    source.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::NotADirectory
                )
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn open_fails_when_snapshots_path_is_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("objects");
    fs::create_dir(&root).await.unwrap();
    fs::create_dir(root.join("blobs")).await.unwrap();
    fs::create_dir(root.join("trees")).await.unwrap();
    let snapshots = root.join("snapshots");
    fs::write(&snapshots, b"not a directory").await.unwrap();

    let error = LocalObjectStore::open(&root).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::Io { path, source }
            if path == snapshots
                && matches!(
                    source.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::NotADirectory
                )
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn put_blob_fails_when_shard_path_is_file() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = ObjectId::from_content(b"hello");
    let shard_path = store.root().join("blobs").join(id.shard_prefix());
    fs::write(&shard_path, b"not a directory").await.unwrap();

    let error = store.put_blob(b"hello").await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::Io { source, .. }
            if matches!(
                source.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotADirectory
            )
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn put_tree_fails_when_shard_path_is_file() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let tree = sample_tree();
    let id = tree.id();
    let shard_path = store.root().join("trees").join(id.shard_prefix());
    fs::write(&shard_path, b"not a directory").await.unwrap();

    let error = store.put_tree(&tree).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::Io { source, .. }
            if matches!(
                source.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotADirectory
            )
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn put_snapshot_fails_when_shard_path_is_file() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let snapshot = sample_snapshot();
    let id = snapshot.id();
    let shard_path = store.root().join("snapshots").join(id.shard_prefix());
    fs::write(&shard_path, b"not a directory").await.unwrap();

    let error = store.put_snapshot(&snapshot).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::Io { source, .. }
            if matches!(
                source.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotADirectory
            )
    ));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn get_blob_reports_io_error_when_blob_path_is_directory() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = ObjectId::from_content(b"directory instead of file");
    fs::create_dir_all(store.blob_path(&id)).await.unwrap();

    let error = store.get_blob(&id).await.unwrap_err();

    assert!(matches!(
        error,
        ObjectStoreError::Io { source, .. }
            if matches!(
                source.kind(),
                io::ErrorKind::IsADirectory | io::ErrorKind::PermissionDenied
            )
    ));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn open_reports_permission_denied_when_parent_is_not_writable() {
    let temp = TempDir::new().unwrap();
    let parent = temp.path().join("readonly");
    fs::create_dir(&parent).await.unwrap();
    set_mode(&parent, 0o500);

    let result = LocalObjectStore::open(parent.join("objects")).await;

    set_mode(&parent, 0o700);
    match result {
        Err(ObjectStoreError::Io { source, .. }) => {
            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
        }
        Ok(_) => {
            eprintln!(
                "permission assertion skipped: process can create files in a non-writable directory"
            );
        }
        Err(error) => panic!("unexpected error: {error}"),
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn get_blob_reports_permission_denied_when_blob_is_not_readable() {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let id = store.put_blob(b"secret").await.unwrap();
    let path = store.blob_path(&id);
    set_mode(&path, 0o000);

    let result = store.get_blob(&id).await;

    set_mode(&path, 0o600);
    match result {
        Err(ObjectStoreError::Io { source, .. }) => {
            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
        }
        Ok(bytes) => {
            assert_eq!(bytes, b"secret");
            eprintln!("permission assertion skipped: process can read a file with no read bits");
        }
        Err(error) => panic!("unexpected error: {error}"),
    }
}

fn sample_tree() -> Tree {
    Tree::new([
        TreeEntry::blob("✨.txt", ObjectId::from_content(b"sparkles")).unwrap(),
        TreeEntry::blob("café.txt", ObjectId::from_content(b"coffee")).unwrap(),
        TreeEntry::tree("src", ObjectId::from_content(b"child tree")).unwrap(),
    ])
    .unwrap()
}

fn sample_snapshot() -> Snapshot {
    Snapshot::new(
        sample_tree().id(),
        vec![ObjectId::from_content(b"parent")],
        1_700_000_000_123,
        Some("agent@example".to_owned()),
        Some("checkpoint".to_owned()),
        SnapshotProvenance::manual().with_attribute("model", "test-model"),
    )
}

fn encode_tree_entries_unchecked(entries: &[(EntryKind, &str, ObjectId)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"ERA_TREE_V1\0");
    bytes.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (kind, name, id) in entries {
        bytes.push(match kind {
            EntryKind::Blob => b'b',
            EntryKind::Tree => b't',
        });
        bytes.extend_from_slice(&(name.len() as u32).to_be_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(id.as_bytes());
    }
    bytes
}

fn encode_snapshot_attrs_unchecked(attrs: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"ERA_SNAPSHOT_V1\0");
    bytes.extend_from_slice(sample_tree().id().as_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&1_u64.to_be_bytes());
    bytes.push(0);
    bytes.push(0);
    encode_string(&mut bytes, "manual-snapshot");
    bytes.extend_from_slice(&(attrs.len() as u32).to_be_bytes());
    for (key, value) in attrs {
        encode_string(&mut bytes, key);
        encode_string(&mut bytes, value);
    }
    bytes
}

fn encode_string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

async fn object_file_count(root: &Path, kind: ObjectKind) -> usize {
    count_files_matching(root, kind, |name| !name.ends_with(".tmp")).await
}

async fn temp_file_count(root: &Path, kind: ObjectKind) -> usize {
    count_files_matching(root, kind, |name| name.ends_with(".tmp")).await
}

async fn count_files_matching(
    root: &Path,
    kind: ObjectKind,
    matches_name: fn(&str) -> bool,
) -> usize {
    let directory = object_dir(root, kind);
    let mut count = 0;
    let mut shards = fs::read_dir(directory).await.unwrap();

    while let Some(shard) = shards.next_entry().await.unwrap() {
        if !shard.file_type().await.unwrap().is_dir() {
            continue;
        }

        let mut files = fs::read_dir(shard.path()).await.unwrap();
        while let Some(file) = files.next_entry().await.unwrap() {
            if !file.file_type().await.unwrap().is_file() {
                continue;
            }

            let name = file.file_name();
            if matches_name(&name.to_string_lossy()) {
                count += 1;
            }
        }
    }

    count
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    let permissions = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(path, permissions).unwrap();
}
