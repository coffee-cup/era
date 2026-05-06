use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use era_materialization::FilesystemMaterializer;
use era_repository::{Repository, SnapshotRequest};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{fs, runtime::Runtime};

struct SnapshotBench {
    _temp: TempDir,
    work: PathBuf,
    repo: Repository,
}

fn snapshot_benches(criterion: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = criterion.benchmark_group("snapshot");

    group.bench_function("warm_noop_snapshot", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_repo(true)),
            |fixture| {
                runtime.block_on(async move {
                    let materializer =
                        FilesystemMaterializer::with_cache_path(fixture.repo.capture_cache_path());
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

    group.bench_function("hinted_one_file_snapshot", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_dirty_repo()),
            |fixture| {
                runtime.block_on(async move {
                    let materializer =
                        FilesystemMaterializer::with_cache_path(fixture.repo.capture_cache_path());
                    fixture
                        .repo
                        .snapshot_if_changed_with_hints(
                            &materializer,
                            SnapshotRequest::manual_unlabeled(),
                            &[PathBuf::from("dir-5/file-5.txt")],
                        )
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("cold_one_file_snapshot", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_dirty_repo_without_cache()),
            |fixture| {
                runtime.block_on(async move {
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

    group.finish();
}

async fn setup_dirty_repo() -> SnapshotBench {
    let fixture = setup_repo(true).await;
    fs::write(fixture.work.join("dir-5/file-5.txt"), b"changed")
        .await
        .unwrap();
    fixture
}

async fn setup_dirty_repo_without_cache() -> SnapshotBench {
    let fixture = setup_repo(false).await;
    fs::write(fixture.work.join("dir-5/file-5.txt"), b"changed")
        .await
        .unwrap();
    fixture
}

async fn setup_repo(warm_cache: bool) -> SnapshotBench {
    let temp = TempDir::new().unwrap();
    let work = temp.path().join("work");
    fs::create_dir(&work).await.unwrap();
    write_tree(&work, 10, 10).await;
    let init_materializer = FilesystemMaterializer::new();
    let init = Repository::init(&work, &init_materializer, SnapshotRequest::initial())
        .await
        .unwrap();
    let repo = init.repository;

    if warm_cache {
        let materializer = FilesystemMaterializer::with_cache_path(repo.capture_cache_path());
        repo.snapshot_if_changed(&materializer, SnapshotRequest::manual_unlabeled())
            .await
            .unwrap();
    }

    SnapshotBench {
        _temp: temp,
        work,
        repo,
    }
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
    targets = snapshot_benches
}
criterion_main!(benches);
