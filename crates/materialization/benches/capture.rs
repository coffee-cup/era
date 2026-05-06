use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use era_materialization::{FilesystemMaterializer, WorkingDirectory};
use era_object_store::LocalObjectStore;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{fs, runtime::Runtime};

struct CaptureBench {
    _temp: TempDir,
    work: PathBuf,
    store: LocalObjectStore,
    cache_path: PathBuf,
}

fn capture_benches(criterion: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = criterion.benchmark_group("capture");

    group.bench_function("cold_many_small_files", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_tree(10, 10, false)),
            |fixture| {
                runtime.block_on(async move {
                    let materializer = FilesystemMaterializer::new();
                    materializer
                        .capture_tree(&WorkingDirectory::new(&fixture.work), &fixture.store)
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("warm_persistent_noop", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_tree(10, 10, true)),
            |fixture| {
                runtime.block_on(async move {
                    let materializer = FilesystemMaterializer::with_cache_path(&fixture.cache_path);
                    materializer
                        .capture_tree(&WorkingDirectory::new(&fixture.work), &fixture.store)
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("hinted_one_file_edit", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_hinted_edit()),
            |fixture| {
                runtime.block_on(async move {
                    let materializer = FilesystemMaterializer::with_cache_path(&fixture.cache_path);
                    materializer
                        .capture_tree_with_hints(
                            &WorkingDirectory::new(&fixture.work),
                            &fixture.store,
                            &[PathBuf::from("dir-5/file-5.txt")],
                        )
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("restore_noop", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_tree(10, 10, true)),
            |fixture| {
                runtime.block_on(async move {
                    let materializer = FilesystemMaterializer::with_cache_path(&fixture.cache_path);
                    let root = materializer
                        .scan_tree(&WorkingDirectory::new(&fixture.work))
                        .await
                        .unwrap()
                        .root_tree_id;
                    materializer
                        .materialize_tree(
                            root,
                            &WorkingDirectory::new(&fixture.work),
                            &fixture.store,
                        )
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

async fn setup_tree(
    directories: usize,
    files_per_directory: usize,
    warm_cache: bool,
) -> CaptureBench {
    let temp = TempDir::new().unwrap();
    let work = temp.path().join("work");
    fs::create_dir(&work).await.unwrap();
    write_tree(&work, directories, files_per_directory).await;
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let cache_path = temp.path().join("cache/capture-v2.redb");

    if warm_cache {
        FilesystemMaterializer::with_cache_path(&cache_path)
            .capture_tree(&WorkingDirectory::new(&work), &store)
            .await
            .unwrap();
    }

    CaptureBench {
        _temp: temp,
        work,
        store,
        cache_path,
    }
}

async fn setup_hinted_edit() -> CaptureBench {
    let fixture = setup_tree(10, 10, true).await;
    fs::write(fixture.work.join("dir-5/file-5.txt"), b"changed")
        .await
        .unwrap();
    fixture
}

async fn write_tree(root: &Path, directories: usize, files_per_directory: usize) {
    for directory_index in 0..directories {
        let directory = root.join(format!("dir-{directory_index}"));
        fs::create_dir(&directory).await.unwrap();
        for file_index in 0..files_per_directory {
            fs::write(
                directory.join(format!("file-{file_index}.txt")),
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
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(2));
    targets = capture_benches
}
criterion_main!(benches);
