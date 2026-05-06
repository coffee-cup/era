use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use era_materialization::{FilesystemMaterializer, WorkingDirectory};
use era_object_store::LocalObjectStore;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{fs, runtime::Runtime};

struct LargeWorkTree {
    _temp: TempDir,
    work: PathBuf,
    counter: AtomicUsize,
}

struct WarmCaptureFixture {
    _temp: TempDir,
    store: LocalObjectStore,
    cache_path: PathBuf,
    root_tree_id: era_core::ObjectId,
}

fn capture_large_benches(criterion: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let tree = runtime.block_on(setup_large_work_tree(100, 100));
    let warm = runtime.block_on(setup_warm_capture_fixture(&tree));
    let hot_materializer = FilesystemMaterializer::with_cache_path(&warm.cache_path);
    let flat_tree = runtime.block_on(setup_large_work_tree(1, 10_000));
    let flat_warm = runtime.block_on(setup_warm_capture_fixture(&flat_tree));
    let flat_hot_materializer = FilesystemMaterializer::with_cache_path(&flat_warm.cache_path);
    let mut group = criterion.benchmark_group("capture_large");

    group.bench_function("cold_10000_small_files", |bench| {
        bench.iter_batched(
            || runtime.block_on(tree.fresh_store()),
            |store| {
                runtime.block_on(async {
                    let materializer = FilesystemMaterializer::new();
                    materializer
                        .capture_tree(&WorkingDirectory::new(&tree.work), &store)
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("warm_persistent_noop_10000", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                let materializer = FilesystemMaterializer::with_cache_path(&warm.cache_path);
                materializer
                    .capture_tree(&WorkingDirectory::new(&tree.work), &warm.store)
                    .await
                    .unwrap();
            });
        });
    });

    group.bench_function("hinted_one_file_edit_10000", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                let iteration = tree.counter.fetch_add(1, Ordering::Relaxed);
                let changed = PathBuf::from("dir-050/file-050.txt");
                fs::write(tree.work.join(&changed), format!("changed {iteration}\n"))
                    .await
                    .unwrap();
                let materializer = FilesystemMaterializer::with_cache_path(&warm.cache_path);
                materializer
                    .capture_tree_with_hints(
                        &WorkingDirectory::new(&tree.work),
                        &warm.store,
                        &[changed],
                    )
                    .await
                    .unwrap();
            });
        });
    });

    group.bench_function("hot_hinted_one_file_edit_10000", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                let iteration = tree.counter.fetch_add(1, Ordering::Relaxed);
                let changed = PathBuf::from("dir-050/file-050.txt");
                fs::write(
                    tree.work.join(&changed),
                    format!("hot changed {iteration}\n"),
                )
                .await
                .unwrap();
                hot_materializer
                    .capture_tree_with_hints(
                        &WorkingDirectory::new(&tree.work),
                        &warm.store,
                        &[changed],
                    )
                    .await
                    .unwrap();
            });
        });
    });

    group.bench_function("flat_hinted_one_file_edit_10000", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                let iteration = flat_tree.counter.fetch_add(1, Ordering::Relaxed);
                let changed = PathBuf::from("dir-000/file-5000.txt");
                fs::write(
                    flat_tree.work.join(&changed),
                    format!("flat changed {iteration}\n"),
                )
                .await
                .unwrap();
                let materializer = FilesystemMaterializer::with_cache_path(&flat_warm.cache_path);
                materializer
                    .capture_tree_with_hints(
                        &WorkingDirectory::new(&flat_tree.work),
                        &flat_warm.store,
                        &[changed],
                    )
                    .await
                    .unwrap();
            });
        });
    });

    group.bench_function("flat_hot_hinted_one_file_edit_10000", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                let iteration = flat_tree.counter.fetch_add(1, Ordering::Relaxed);
                let changed = PathBuf::from("dir-000/file-5000.txt");
                fs::write(
                    flat_tree.work.join(&changed),
                    format!("flat hot changed {iteration}\n"),
                )
                .await
                .unwrap();
                flat_hot_materializer
                    .capture_tree_with_hints(
                        &WorkingDirectory::new(&flat_tree.work),
                        &flat_warm.store,
                        &[changed],
                    )
                    .await
                    .unwrap();
            });
        });
    });

    group.bench_function("batched_dirty_hints_10000", |bench| {
        bench.iter_batched(
            || next_dirty_batch(&tree, 20),
            |paths| {
                runtime.block_on(async {
                    let iteration = tree.counter.fetch_add(1, Ordering::Relaxed);
                    for path in &paths {
                        fs::write(tree.work.join(path), format!("batch {iteration}\n"))
                            .await
                            .unwrap();
                    }
                    let materializer = FilesystemMaterializer::with_cache_path(&warm.cache_path);
                    materializer
                        .capture_tree_with_hints(
                            &WorkingDirectory::new(&tree.work),
                            &warm.store,
                            &paths,
                        )
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("restore_noop_10000", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                let materializer = FilesystemMaterializer::with_cache_path(&warm.cache_path);
                materializer
                    .materialize_tree(
                        warm.root_tree_id,
                        &WorkingDirectory::new(&tree.work),
                        &warm.store,
                    )
                    .await
                    .unwrap();
            });
        });
    });

    group.finish();
}

async fn setup_large_work_tree(directories: usize, files_per_directory: usize) -> LargeWorkTree {
    let temp = TempDir::new().unwrap();
    let work = temp.path().join("work");
    fs::create_dir(&work).await.unwrap();
    write_tree(&work, directories, files_per_directory).await;
    LargeWorkTree {
        _temp: temp,
        work,
        counter: AtomicUsize::new(0),
    }
}

async fn setup_warm_capture_fixture(tree: &LargeWorkTree) -> WarmCaptureFixture {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let cache_path = temp.path().join("cache/capture-v2.redb");
    let root_tree_id = FilesystemMaterializer::with_cache_path(&cache_path)
        .capture_tree(&WorkingDirectory::new(&tree.work), &store)
        .await
        .unwrap()
        .root_tree_id;
    WarmCaptureFixture {
        _temp: temp,
        store,
        cache_path,
        root_tree_id,
    }
}

impl LargeWorkTree {
    async fn fresh_store(&self) -> LocalObjectStore {
        let index = self.counter.fetch_add(1, Ordering::Relaxed);
        LocalObjectStore::open(self.work.join(format!("../objects-{index}")))
            .await
            .unwrap()
    }
}

fn next_dirty_batch(tree: &LargeWorkTree, count: usize) -> Vec<PathBuf> {
    let start = tree.counter.fetch_add(count, Ordering::Relaxed);
    (0..count)
        .map(|offset| {
            let index = (start + offset) % 10_000;
            PathBuf::from(format!(
                "dir-{:03}/file-{:03}.txt",
                index / 100,
                index % 100
            ))
        })
        .collect()
}

async fn write_tree(root: &Path, directories: usize, files_per_directory: usize) {
    for directory_index in 0..directories {
        let directory = root.join(format!("dir-{directory_index:03}"));
        fs::create_dir(&directory).await.unwrap();
        for file_index in 0..files_per_directory {
            fs::write(
                directory.join(format!("file-{file_index:03}.txt")),
                format!("content {directory_index} {file_index}\n"),
            )
            .await
            .unwrap();
        }
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(200))
        .measurement_time(Duration::from_secs(1));
    targets = capture_large_benches
}
criterion_main!(benches);
