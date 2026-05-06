use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use era_materialization::FilesystemMaterializer;
use era_repository::{Repository, SnapshotRequest};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{fs, runtime::Runtime};

struct LargeRepo {
    _temp: TempDir,
    work: PathBuf,
    repo: Repository,
    counter: AtomicUsize,
}

fn snapshot_large_benches(criterion: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let fixture = runtime.block_on(setup_large_repo(100, 100));
    let hot_materializer =
        FilesystemMaterializer::with_cache_path(fixture.repo.capture_cache_path());
    let mut group = criterion.benchmark_group("snapshot_large");

    group.bench_function("warm_noop_snapshot_10000", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                let materializer =
                    FilesystemMaterializer::with_cache_path(fixture.repo.capture_cache_path());
                fixture
                    .repo
                    .snapshot_if_changed(&materializer, SnapshotRequest::manual_unlabeled())
                    .await
                    .unwrap();
            });
        });
    });

    group.bench_function("hinted_one_file_snapshot_10000", |bench| {
        bench.iter_batched(
            || runtime.block_on(fixture.write_changed_file("dir-050/file-050.txt")),
            |changed| {
                runtime.block_on(async {
                    let materializer =
                        FilesystemMaterializer::with_cache_path(fixture.repo.capture_cache_path());
                    fixture
                        .repo
                        .snapshot_if_changed_with_hints(
                            &materializer,
                            SnapshotRequest::manual_unlabeled(),
                            &[changed],
                        )
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("hot_hinted_one_file_snapshot_10000", |bench| {
        bench.iter_batched(
            || runtime.block_on(fixture.write_changed_file("dir-050/file-050.txt")),
            |changed| {
                runtime.block_on(async {
                    fixture
                        .repo
                        .snapshot_if_changed_with_hints(
                            &hot_materializer,
                            SnapshotRequest::manual_unlabeled(),
                            &[changed],
                        )
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("cold_one_file_snapshot_10000", |bench| {
        bench.iter_batched(
            || runtime.block_on(fixture.write_changed_file("dir-050/file-050.txt")),
            |_| {
                runtime.block_on(async {
                    let materializer = FilesystemMaterializer::new();
                    fixture
                        .repo
                        .snapshot_if_changed(&materializer, SnapshotRequest::manual_unlabeled())
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("warm_noop_status_10000", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                let materializer =
                    FilesystemMaterializer::with_cache_path(fixture.repo.capture_cache_path());
                fixture
                    .repo
                    .working_tree_status(&materializer)
                    .await
                    .unwrap();
            });
        });
    });

    group.finish();
}

async fn setup_large_repo(directories: usize, files_per_directory: usize) -> LargeRepo {
    let temp = TempDir::new().unwrap();
    let work = temp.path().join("work");
    fs::create_dir(&work).await.unwrap();
    write_tree(&work, directories, files_per_directory).await;

    let init_materializer = FilesystemMaterializer::new();
    let init = Repository::init(&work, &init_materializer, SnapshotRequest::initial())
        .await
        .unwrap();
    let repo = init.repository;
    let materializer = FilesystemMaterializer::with_cache_path(repo.capture_cache_path());
    repo.snapshot_if_changed(&materializer, SnapshotRequest::manual_unlabeled())
        .await
        .unwrap();

    LargeRepo {
        _temp: temp,
        work,
        repo,
        counter: AtomicUsize::new(0),
    }
}

impl LargeRepo {
    async fn write_changed_file(&self, relative_path: &str) -> PathBuf {
        let iteration = self.counter.fetch_add(1, Ordering::Relaxed);
        fs::write(
            self.work.join(relative_path),
            format!("changed {iteration}\n"),
        )
        .await
        .unwrap();
        PathBuf::from(relative_path)
    }
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
    targets = snapshot_large_benches
}
criterion_main!(benches);
