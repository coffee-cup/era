use super::*;
use era_core::{EntryKind, ObjectKind, Tree, TreeEntry};
use era_object_store::LocalObjectStore;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs;

#[cfg(unix)]
use std::os::unix::fs as unix_fs;

#[tokio::test(flavor = "current_thread")]
async fn working_directory_exposes_root() {
    let directory = WorkingDirectory::new("/tmp/project");

    assert_eq!(directory.root(), Path::new("/tmp/project"));
}

#[test]
fn capture_options_are_configurable() {
    let options = CaptureOptions::default();

    assert!(options.excludes_directory_name(".era"));
    assert!(options.excludes_directory_name("target"));
    assert_eq!(options.symlink_policy(), SymlinkPolicy::Skip);

    let options = options
        .without_excluded_directory("target")
        .with_excluded_directory("vendor")
        .with_symlink_policy(SymlinkPolicy::Error);

    assert!(!options.excludes_directory_name("target"));
    assert!(options.excludes_directory_name("vendor"));
    assert_eq!(options.symlink_policy(), SymlinkPolicy::Error);
}

#[tokio::test(flavor = "current_thread")]
async fn empty_directory_captures_as_empty_tree() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let result = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert_eq!(result.root_tree_id, Tree::empty().id());
    assert_eq!(
        store.get_tree(&result.root_tree_id).await.unwrap(),
        Tree::empty()
    );
    assert_eq!(
        result.stats,
        CaptureStats {
            directories_seen: 1,
            trees_stored: 1,
            ..CaptureStats::default()
        }
    );
    assert!(result.issues.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn captures_nested_files_and_empty_directories() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    let readme = b"hello";
    let main_rs = b"fn main() {}\n";
    let lib_rs = b"";
    fs::write(work.join("README.md"), readme).await.unwrap();
    fs::create_dir(work.join("src")).await.unwrap();
    fs::write(work.join("src/main.rs"), main_rs).await.unwrap();
    fs::write(work.join("src/lib.rs"), lib_rs).await.unwrap();
    fs::create_dir(work.join("empty")).await.unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let result = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert!(
        store
            .contains(ObjectKind::Tree, &result.root_tree_id)
            .await
            .unwrap()
    );
    assert_eq!(
        result.stats,
        CaptureStats {
            files_seen: 3,
            directories_seen: 3,
            bytes_read: (readme.len() + main_rs.len() + lib_rs.len()) as u64,
            blobs_stored: 3,
            trees_stored: 3,
            hash_cache_misses: 3,
            ..CaptureStats::default()
        }
    );

    let root_tree = store.get_tree(&result.root_tree_id).await.unwrap();
    assert_eq!(entry_names(&root_tree), vec!["README.md", "empty", "src"]);

    let readme_entry = entry(&root_tree, "README.md");
    assert_eq!(readme_entry.kind(), EntryKind::Blob);
    assert_eq!(store.get_blob(&readme_entry.id()).await.unwrap(), readme);

    let empty_entry = entry(&root_tree, "empty");
    assert_eq!(empty_entry.kind(), EntryKind::Tree);
    assert_eq!(
        store.get_tree(&empty_entry.id()).await.unwrap(),
        Tree::empty()
    );

    let src_entry = entry(&root_tree, "src");
    assert_eq!(src_entry.kind(), EntryKind::Tree);
    let src_tree = store.get_tree(&src_entry.id()).await.unwrap();
    assert_eq!(entry_names(&src_tree), vec!["lib.rs", "main.rs"]);
    assert_eq!(
        store
            .get_blob(&entry(&src_tree, "main.rs").id())
            .await
            .unwrap(),
        main_rs
    );
    assert_eq!(
        store
            .get_blob(&entry(&src_tree, "lib.rs").id())
            .await
            .unwrap(),
        lib_rs
    );
}

#[tokio::test(flavor = "current_thread")]
async fn default_exclusions_skip_transient_directories() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    for directory in [".era", ".git", "target", "node_modules", ".next"] {
        fs::create_dir(work.join(directory)).await.unwrap();
        fs::write(work.join(directory).join("generated.txt"), b"ignored")
            .await
            .unwrap();
    }
    fs::create_dir(work.join("src")).await.unwrap();
    fs::write(work.join("src/main.rs"), b"tracked")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let result = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    let root_tree = store.get_tree(&result.root_tree_id).await.unwrap();
    assert_eq!(entry_names(&root_tree), vec!["src"]);
    assert_eq!(result.stats.files_seen, 1);
    assert_eq!(result.stats.directories_seen, 2);
    assert_eq!(result.stats.ignored_entries, 5);
    assert!(result.issues.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn exclusion_list_can_be_overridden() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::create_dir(work.join("target")).await.unwrap();
    fs::write(work.join("target/out.txt"), b"captured")
        .await
        .unwrap();
    fs::create_dir(work.join("vendor")).await.unwrap();
    fs::write(work.join("vendor/lib.rs"), b"ignored")
        .await
        .unwrap();
    fs::write(work.join("keep.txt"), b"keep").await.unwrap();
    let store = open_store(&temp).await;
    let options = CaptureOptions::default()
        .without_excluded_directory("target")
        .with_excluded_directory("vendor");
    let materializer = FilesystemMaterializer::with_options(options);

    let result = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    let root_tree = store.get_tree(&result.root_tree_id).await.unwrap();
    assert_eq!(entry_names(&root_tree), vec!["keep.txt", "target"]);
    assert_eq!(result.stats.ignored_entries, 1);
    assert_eq!(
        store
            .get_blob(&entry(&root_tree, "keep.txt").id())
            .await
            .unwrap(),
        b"keep"
    );

    let target_tree = store
        .get_tree(&entry(&root_tree, "target").id())
        .await
        .unwrap();
    assert_eq!(entry_names(&target_tree), vec!["out.txt"]);
}

#[tokio::test(flavor = "current_thread")]
async fn scan_tree_matches_capture_without_storing_objects() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"hello").await.unwrap();
    fs::create_dir(work.join("src")).await.unwrap();
    fs::write(work.join("src/main.rs"), b"fn main() {}\n")
        .await
        .unwrap();
    fs::create_dir(work.join("target")).await.unwrap();
    fs::write(work.join("target/generated.txt"), b"ignored")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let scan = materializer
        .scan_tree(&WorkingDirectory::new(&work))
        .await
        .unwrap();
    let capture = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert_eq!(scan.root_tree_id, capture.root_tree_id);
    assert_eq!(scan.stats.files_seen, capture.stats.files_seen);
    assert_eq!(scan.stats.directories_seen, capture.stats.directories_seen);
    assert_eq!(scan.stats.bytes_read, capture.stats.bytes_read);
    assert_eq!(scan.stats.ignored_entries, capture.stats.ignored_entries);
    assert_eq!(scan.stats.symlinks_skipped, capture.stats.symlinks_skipped);
    assert_eq!(scan.issues, capture.issues);
}

#[tokio::test(flavor = "current_thread")]
async fn hash_cache_reuses_unchanged_files_and_can_be_invalidated() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"hello").await.unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let first = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    let second = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert_eq!(second.root_tree_id, first.root_tree_id);
    assert_eq!(first.stats.hash_cache_misses, 1);
    assert_eq!(first.stats.hash_cache_hits, 0);
    assert_eq!(second.stats.hash_cache_hits, 1);
    assert_eq!(second.stats.hash_cache_misses, 0);
    assert_eq!(second.stats.bytes_read, 0);
    assert_eq!(second.stats.blobs_stored, 0);

    materializer.invalidate_paths([PathBuf::from("README.md")]);
    let third = materializer
        .scan_tree(&WorkingDirectory::new(&work))
        .await
        .unwrap();

    assert_eq!(third.root_tree_id, first.root_tree_id);
    assert_eq!(third.stats.hash_cache_hits, 0);
    assert_eq!(third.stats.hash_cache_misses, 1);
    assert_eq!(third.stats.bytes_read, 5);
}

#[tokio::test(flavor = "current_thread")]
async fn compare_tree_reports_path_level_changes() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"one").await.unwrap();
    fs::create_dir(work.join("dir-to-file")).await.unwrap();
    fs::write(work.join("dir-to-file/nested.txt"), b"nested")
        .await
        .unwrap();
    fs::create_dir(work.join("empty")).await.unwrap();
    fs::write(work.join("file-to-dir"), b"file").await.unwrap();
    fs::create_dir(work.join("src")).await.unwrap();
    fs::write(work.join("src/main.rs"), b"fn main() {}")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let captured = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    fs::write(work.join("README.md"), b"two").await.unwrap();
    fs::remove_dir_all(work.join("dir-to-file")).await.unwrap();
    fs::write(work.join("dir-to-file"), b"now a file")
        .await
        .unwrap();
    fs::remove_dir(work.join("empty")).await.unwrap();
    fs::write(work.join("extra.txt"), b"extra").await.unwrap();
    fs::remove_file(work.join("file-to-dir")).await.unwrap();
    fs::create_dir(work.join("file-to-dir")).await.unwrap();
    fs::remove_file(work.join("src/main.rs")).await.unwrap();

    let comparison = materializer
        .compare_tree(captured.root_tree_id, &WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    let scan = materializer
        .scan_tree(&WorkingDirectory::new(&work))
        .await
        .unwrap();

    assert_eq!(comparison.current_root_tree_id, scan.root_tree_id);
    assert!(!comparison.is_clean());
    assert_eq!(
        comparison.changes,
        vec![
            TreeChange::modified("README.md"),
            TreeChange::type_changed("dir-to-file"),
            TreeChange::deleted("empty"),
            TreeChange::added("extra.txt"),
            TreeChange::type_changed("file-to-dir"),
            TreeChange::deleted("src/main.rs"),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn compare_tree_ignores_excluded_added_directories() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"one").await.unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let captured = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    fs::create_dir(work.join("target")).await.unwrap();
    fs::write(work.join("target/generated.txt"), b"ignored")
        .await
        .unwrap();

    let comparison = materializer
        .compare_tree(captured.root_tree_id, &WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert!(comparison.is_clean());
    assert!(comparison.changes.is_empty());
    assert_eq!(comparison.stats.ignored_entries, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn materialize_tree_reconciles_working_directory_and_preserves_excluded_dirs() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"one").await.unwrap();
    fs::create_dir(work.join("src")).await.unwrap();
    fs::write(work.join("src/main.rs"), b"fn main() {}\n")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let captured = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    fs::write(work.join("README.md"), b"two").await.unwrap();
    fs::remove_dir_all(work.join("src")).await.unwrap();
    fs::write(work.join("extra.txt"), b"remove me")
        .await
        .unwrap();
    fs::create_dir(work.join("target")).await.unwrap();
    fs::write(work.join("target/generated.txt"), b"preserve me")
        .await
        .unwrap();

    let result = materializer
        .materialize_tree(captured.root_tree_id, &WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert_eq!(fs::read(work.join("README.md")).await.unwrap(), b"one");
    assert_eq!(
        fs::read(work.join("src/main.rs")).await.unwrap(),
        b"fn main() {}\n"
    );
    assert!(fs::metadata(work.join("extra.txt")).await.is_err());
    assert_eq!(
        fs::read(work.join("target/generated.txt")).await.unwrap(),
        b"preserve me"
    );
    assert!(result.stats.files_written >= 2);
    assert!(result.stats.entries_removed >= 1);
}

#[tokio::test(flavor = "current_thread")]
async fn second_capture_reflects_deleted_files() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("delete-me.txt"), b"temporary")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let first = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    fs::remove_file(work.join("delete-me.txt")).await.unwrap();
    let second = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert_ne!(first.root_tree_id, second.root_tree_id);
    assert_eq!(
        store.get_tree(&second.root_tree_id).await.unwrap(),
        Tree::empty()
    );
    assert_eq!(second.stats.files_seen, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn captures_emoji_and_non_english_filenames() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("✨.txt"), b"sparkles").await.unwrap();
    fs::create_dir(work.join("日本語")).await.unwrap();
    fs::write(work.join("日本語/café.txt"), b"coffee")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let result = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    let root_tree = store.get_tree(&result.root_tree_id).await.unwrap();
    assert_eq!(entry_names(&root_tree), vec!["✨.txt", "日本語"]);
    assert_eq!(
        store
            .get_blob(&entry(&root_tree, "✨.txt").id())
            .await
            .unwrap(),
        b"sparkles"
    );
    let japanese_tree = store
        .get_tree(&entry(&root_tree, "日本語").id())
        .await
        .unwrap();
    assert_eq!(entry_names(&japanese_tree), vec!["café.txt"]);
}

#[tokio::test(flavor = "current_thread")]
async fn capture_rejects_missing_root() {
    let temp = TempDir::new().unwrap();
    let store = open_store(&temp).await;
    let missing = temp.path().join("missing");
    let materializer = FilesystemMaterializer::new();

    let error = materializer
        .capture_tree(&WorkingDirectory::new(&missing), &store)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        MaterializationError::RootMissing { path } if path == missing
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn capture_rejects_root_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("file.txt");
    fs::write(&root, b"not a directory").await.unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let error = materializer
        .capture_tree(&WorkingDirectory::new(&root), &store)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        MaterializationError::RootNotDirectory { path } if path == root
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn materializer_trait_object_captures_tree() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("file.txt"), b"via trait")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let materializer: &dyn Materializer = &materializer;

    let result = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    let root_tree = store.get_tree(&result.root_tree_id).await.unwrap();
    assert_eq!(
        store
            .get_blob(&entry(&root_tree, "file.txt").id())
            .await
            .unwrap(),
        b"via trait"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn symlinks_are_skipped_by_default_without_following() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("real.txt"), b"real").await.unwrap();
    unix_fs::symlink("real.txt", work.join("link.txt")).unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let result = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    let root_tree = store.get_tree(&result.root_tree_id).await.unwrap();
    assert_eq!(entry_names(&root_tree), vec!["real.txt"]);
    assert_eq!(result.stats.symlinks_skipped, 1);
    assert_eq!(
        result.issues,
        vec![CaptureIssue::new(
            PathBuf::from("link.txt"),
            CaptureIssueKind::SkippedSymlink,
        )]
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn compare_tree_skips_added_symlinks_without_reporting_changes() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("real.txt"), b"real").await.unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let captured = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    unix_fs::symlink("real.txt", work.join("link.txt")).unwrap();

    let comparison = materializer
        .compare_tree(captured.root_tree_id, &WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert!(comparison.is_clean());
    assert!(comparison.changes.is_empty());
    assert_eq!(comparison.stats.symlinks_skipped, 1);
    assert_eq!(
        comparison.issues,
        vec![CaptureIssue::new(
            PathBuf::from("link.txt"),
            CaptureIssueKind::SkippedSymlink,
        )]
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn materialize_tree_preserves_skipped_symlinks() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("real.txt"), b"real").await.unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let captured = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    unix_fs::symlink("real.txt", work.join("link.txt")).unwrap();

    materializer
        .materialize_tree(captured.root_tree_id, &WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert!(
        fs::symlink_metadata(work.join("link.txt"))
            .await
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn symlink_policy_can_error() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("real.txt"), b"real").await.unwrap();
    let link = work.join("link.txt");
    unix_fs::symlink("real.txt", &link).unwrap();
    let store = open_store(&temp).await;
    let options = CaptureOptions::default().with_symlink_policy(SymlinkPolicy::Error);
    let materializer = FilesystemMaterializer::with_options(options);

    let error = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        MaterializationError::SymlinkUnsupported { path } if path == link
    ));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn capture_rejects_non_utf8_path_segment() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    let invalid_name = OsString::from_vec(b"bad-\xff".to_vec());
    let invalid_path = work.join(&invalid_name);
    if let Err(error) = fs::write(&invalid_path, b"bad").await {
        eprintln!("non-UTF-8 path assertion skipped: filesystem rejected test path: {error}");
        return;
    }
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();

    let error = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        MaterializationError::PathNotUtf8 { path } if path == invalid_path
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn persistent_capture_cache_reuses_hashes_across_materializers() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("a.txt"), b"a").await.unwrap();
    fs::create_dir(work.join("src")).await.unwrap();
    fs::write(work.join("src/b.txt"), b"b").await.unwrap();
    let store = open_store(&temp).await;
    let cache_path = temp.path().join("cache/capture-v2.redb");

    let first_materializer = FilesystemMaterializer::with_cache_path(&cache_path);
    let first = first_materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    let second_materializer = FilesystemMaterializer::with_cache_path(&cache_path);
    let second = second_materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert_eq!(second.root_tree_id, first.root_tree_id);
    assert_eq!(second.stats.files_seen, 2);
    assert_eq!(second.stats.hash_cache_hits, 2);
    assert_eq!(second.stats.hash_cache_misses, 0);
    assert_eq!(second.stats.bytes_read, 0);
    assert_eq!(second.stats.blobs_stored, 0);
    assert_eq!(second.stats.trees_stored, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn corrupt_persistent_capture_cache_is_ignored_and_rebuilt() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"hello").await.unwrap();
    let store = open_store(&temp).await;
    let cache_path = temp.path().join("cache/capture-v2.redb");
    fs::create_dir_all(cache_path.parent().unwrap())
        .await
        .unwrap();
    fs::write(&cache_path, b"not a capture cache")
        .await
        .unwrap();

    let materializer = FilesystemMaterializer::with_cache_path(&cache_path);
    let result = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    let second = FilesystemMaterializer::with_cache_path(&cache_path)
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert_eq!(result.stats.hash_cache_misses, 1);
    assert_eq!(second.root_tree_id, result.root_tree_id);
    assert_eq!(second.stats.hash_cache_hits, 1);
    assert_eq!(second.stats.bytes_read, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn hinted_capture_rehashes_dirty_file_and_rebuilds_ancestors_only() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("README.md"), b"unchanged")
        .await
        .unwrap();
    fs::create_dir(work.join("src")).await.unwrap();
    fs::write(work.join("src/lib.rs"), b"old").await.unwrap();
    fs::write(work.join("src/main.rs"), b"unchanged")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let cache_path = temp.path().join("cache/capture-v2.redb");

    FilesystemMaterializer::with_cache_path(&cache_path)
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    fs::write(work.join("src/lib.rs"), b"new contents")
        .await
        .unwrap();

    let materializer = FilesystemMaterializer::with_cache_path(&cache_path);
    let hinted = materializer
        .capture_tree_with_hints(
            &WorkingDirectory::new(&work),
            &store,
            &[PathBuf::from("src/lib.rs")],
        )
        .await
        .unwrap();
    let scan = materializer
        .scan_tree(&WorkingDirectory::new(&work))
        .await
        .unwrap();

    assert_eq!(hinted.root_tree_id, scan.root_tree_id);
    assert_eq!(hinted.stats.files_seen, 1);
    assert_eq!(hinted.stats.bytes_read, b"new contents".len() as u64);
    assert_eq!(hinted.stats.blobs_stored, 1);
    assert_eq!(hinted.stats.trees_stored, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn hinted_capture_handles_add_delete_and_type_changes() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("delete.txt"), b"delete").await.unwrap();
    fs::write(work.join("file-to-dir"), b"file").await.unwrap();
    fs::create_dir(work.join("dir-to-file")).await.unwrap();
    fs::write(work.join("dir-to-file/nested.txt"), b"nested")
        .await
        .unwrap();
    let store = open_store(&temp).await;
    let cache_path = temp.path().join("cache/capture-v2.redb");

    FilesystemMaterializer::with_cache_path(&cache_path)
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    fs::remove_file(work.join("delete.txt")).await.unwrap();
    fs::write(work.join("added.txt"), b"added").await.unwrap();
    fs::remove_file(work.join("file-to-dir")).await.unwrap();
    fs::create_dir(work.join("file-to-dir")).await.unwrap();
    fs::write(work.join("file-to-dir/file.txt"), b"child")
        .await
        .unwrap();
    fs::remove_dir_all(work.join("dir-to-file")).await.unwrap();
    fs::write(work.join("dir-to-file"), b"now file")
        .await
        .unwrap();

    let materializer = FilesystemMaterializer::with_cache_path(&cache_path);
    let hinted = materializer
        .capture_tree_with_hints(
            &WorkingDirectory::new(&work),
            &store,
            &[
                PathBuf::from("delete.txt"),
                PathBuf::from("added.txt"),
                PathBuf::from("file-to-dir"),
                PathBuf::from("dir-to-file"),
            ],
        )
        .await
        .unwrap();
    let scan = materializer
        .scan_tree(&WorkingDirectory::new(&work))
        .await
        .unwrap();

    assert_eq!(hinted.root_tree_id, scan.root_tree_id);
    assert_eq!(hinted.stats.files_seen, 3);
    assert_eq!(
        hinted.stats.bytes_read,
        (b"added".len() + b"child".len() + b"now file".len()) as u64
    );
}

#[tokio::test(flavor = "current_thread")]
async fn materialize_tree_seeds_cache_for_hinted_agent_snapshot() {
    let temp = TempDir::new().unwrap();
    let source = create_workdir(&temp).await;
    fs::write(source.join("a.txt"), b"unchanged").await.unwrap();
    fs::write(source.join("b.txt"), b"old").await.unwrap();
    let target = temp.path().join("agent");
    fs::create_dir(&target).await.unwrap();
    let store = open_store(&temp).await;
    let source_capture = FilesystemMaterializer::new()
        .capture_tree(&WorkingDirectory::new(&source), &store)
        .await
        .unwrap();
    let cache_path = temp.path().join("agent-cache/capture-v2.redb");

    FilesystemMaterializer::with_cache_path(&cache_path)
        .materialize_tree(
            source_capture.root_tree_id,
            &WorkingDirectory::new(&target),
            &store,
        )
        .await
        .unwrap();
    fs::write(target.join("b.txt"), b"new").await.unwrap();

    let materializer = FilesystemMaterializer::with_cache_path(&cache_path);
    let hinted = materializer
        .capture_tree_with_hints(
            &WorkingDirectory::new(&target),
            &store,
            &[PathBuf::from("b.txt")],
        )
        .await
        .unwrap();
    let scan = materializer
        .scan_tree(&WorkingDirectory::new(&target))
        .await
        .unwrap();

    assert_eq!(hinted.root_tree_id, scan.root_tree_id);
    assert_eq!(hinted.stats.files_seen, 1);
    assert_eq!(hinted.stats.bytes_read, 3);
}

#[tokio::test(flavor = "current_thread")]
async fn materialize_tree_invalidates_pruned_cached_subtrees() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("a.txt"), b"a").await.unwrap();
    let store = open_store(&temp).await;
    let cache_path = temp.path().join("cache/capture-v2.redb");
    let materializer = FilesystemMaterializer::with_cache_path(&cache_path);
    let without_dir = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    fs::create_dir(work.join("dir")).await.unwrap();
    fs::write(work.join("dir/file.txt"), b"file").await.unwrap();
    materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    materializer
        .materialize_tree(
            without_dir.root_tree_id,
            &WorkingDirectory::new(&work),
            &store,
        )
        .await
        .unwrap();
    let hinted = materializer
        .capture_tree_with_hints(
            &WorkingDirectory::new(&work),
            &store,
            &[PathBuf::from("dir/file.txt")],
        )
        .await
        .unwrap();

    assert_eq!(hinted.root_tree_id, without_dir.root_tree_id);
    assert!(fs::metadata(work.join("dir")).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn materialize_tree_skips_files_that_already_match() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    fs::write(work.join("a.txt"), b"a").await.unwrap();
    fs::write(work.join("b.txt"), b"b").await.unwrap();
    let store = open_store(&temp).await;
    let materializer = FilesystemMaterializer::new();
    let captured = materializer
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    let result = materializer
        .materialize_tree(captured.root_tree_id, &WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    assert_eq!(result.stats.files_written, 0);
    assert_eq!(result.stats.bytes_written, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn large_capture_cache_limits_hinted_work() {
    let temp = TempDir::new().unwrap();
    let work = create_workdir(&temp).await;
    write_numbered_tree(&work, 32, 32).await;
    let store = open_store(&temp).await;
    let cache_path = temp.path().join("cache/capture-v2.redb");

    FilesystemMaterializer::with_cache_path(&cache_path)
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();

    let warm = FilesystemMaterializer::with_cache_path(&cache_path)
        .capture_tree(&WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    assert_eq!(warm.stats.files_seen, 1024);
    assert_eq!(warm.stats.hash_cache_hits, 1024);
    assert_eq!(warm.stats.hash_cache_misses, 0);
    assert_eq!(warm.stats.bytes_read, 0);
    assert_eq!(warm.stats.blobs_stored, 0);
    assert_eq!(warm.stats.trees_stored, 0);

    let dirty_path = PathBuf::from("dir-07/file-07.txt");
    fs::write(work.join(&dirty_path), b"changed").await.unwrap();
    let materializer = FilesystemMaterializer::with_cache_path(&cache_path);
    let hinted = materializer
        .capture_tree_with_hints(&WorkingDirectory::new(&work), &store, &[dirty_path])
        .await
        .unwrap();
    assert_eq!(hinted.stats.files_seen, 1);
    assert_eq!(hinted.stats.hash_cache_hits, 0);
    assert_eq!(hinted.stats.hash_cache_misses, 1);
    assert_eq!(hinted.stats.bytes_read, b"changed".len() as u64);
    assert_eq!(hinted.stats.blobs_stored, 1);
    assert_eq!(hinted.stats.trees_stored, 2);

    let restored = materializer
        .materialize_tree(hinted.root_tree_id, &WorkingDirectory::new(&work), &store)
        .await
        .unwrap();
    assert_eq!(restored.stats.files_written, 0);
    assert_eq!(restored.stats.bytes_written, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn separate_workspace_cache_paths_do_not_share_file_fingerprints() {
    let temp = TempDir::new().unwrap();
    let first_work = temp.path().join("first");
    let second_work = temp.path().join("second");
    fs::create_dir(&first_work).await.unwrap();
    fs::create_dir(&second_work).await.unwrap();
    fs::write(first_work.join("same.txt"), b"same")
        .await
        .unwrap();
    fs::write(second_work.join("same.txt"), b"same")
        .await
        .unwrap();
    let store = open_store(&temp).await;

    FilesystemMaterializer::with_cache_path(temp.path().join("first-cache/capture-v2.redb"))
        .capture_tree(&WorkingDirectory::new(&first_work), &store)
        .await
        .unwrap();
    let second =
        FilesystemMaterializer::with_cache_path(temp.path().join("second-cache/capture-v2.redb"))
            .capture_tree(&WorkingDirectory::new(&second_work), &store)
            .await
            .unwrap();

    assert_eq!(second.stats.hash_cache_hits, 0);
    assert_eq!(second.stats.hash_cache_misses, 1);
}

async fn create_workdir(temp: &TempDir) -> PathBuf {
    let work = temp.path().join("work");
    fs::create_dir(&work).await.unwrap();
    work
}

async fn open_store(temp: &TempDir) -> LocalObjectStore {
    LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap()
}

async fn write_numbered_tree(root: &Path, directories: usize, files_per_directory: usize) {
    for directory_index in 0..directories {
        let directory = root.join(format!("dir-{directory_index:02}"));
        fs::create_dir(&directory).await.unwrap();
        for file_index in 0..files_per_directory {
            fs::write(
                directory.join(format!("file-{file_index:02}.txt")),
                format!("content {directory_index} {file_index}\n"),
            )
            .await
            .unwrap();
        }
    }
}

fn entry_names(tree: &Tree) -> Vec<&str> {
    tree.entries().iter().map(TreeEntry::name).collect()
}

fn entry<'a>(tree: &'a Tree, name: &str) -> &'a TreeEntry {
    tree.entries()
        .iter()
        .find(|entry| entry.name() == name)
        .unwrap_or_else(|| panic!("missing tree entry {name:?}"))
}
