use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use era_object_store::LocalObjectStore;
use tempfile::TempDir;
use tokio::runtime::Runtime;

struct BaseFixture {
    _temp: TempDir,
    store: LocalObjectStore,
    base_id: era_core::ObjectId,
    changed: Vec<u8>,
}

fn blob_delta_benches(criterion: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let base = large_bytes();
    let changed = one_byte_edit(&base);
    let delta_read = runtime.block_on(setup_base_fixture(&base, &changed));
    let delta_id = runtime.block_on(async {
        delta_read
            .store
            .put_blob_with_base(&delta_read.changed, Some(&delta_read.base_id))
            .await
            .unwrap()
    });
    let mut group = criterion.benchmark_group("blob_delta");

    group.bench_function("put_raw_large_blob", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_empty_store()),
            |fixture| {
                runtime.block_on(async {
                    fixture.store.put_blob(&changed).await.unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("put_delta_one_byte_edit", |bench| {
        bench.iter_batched(
            || runtime.block_on(setup_base_fixture(&base, &changed)),
            |fixture| {
                runtime.block_on(async {
                    fixture
                        .store
                        .put_blob_with_base(&fixture.changed, Some(&fixture.base_id))
                        .await
                        .unwrap();
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("get_delta_one_byte_edit", |bench| {
        bench.iter(|| {
            runtime.block_on(async {
                delta_read.store.get_blob(&delta_id).await.unwrap();
            });
        });
    });

    group.finish();
}

async fn setup_empty_store() -> StoreFixture {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    StoreFixture { _temp: temp, store }
}

async fn setup_base_fixture(base: &[u8], changed: &[u8]) -> BaseFixture {
    let temp = TempDir::new().unwrap();
    let store = LocalObjectStore::open(temp.path().join("objects"))
        .await
        .unwrap();
    let base_id = store.put_blob(base).await.unwrap();
    BaseFixture {
        _temp: temp,
        store,
        base_id,
        changed: changed.to_vec(),
    }
}

struct StoreFixture {
    _temp: TempDir,
    store: LocalObjectStore,
}

fn large_bytes() -> Vec<u8> {
    (0..(8 * 1024 * 1024))
        .map(|index| (index % 251) as u8)
        .collect()
}

fn one_byte_edit(base: &[u8]) -> Vec<u8> {
    let mut changed = base.to_vec();
    let midpoint = changed.len() / 2;
    changed[midpoint] = changed[midpoint].wrapping_add(1);
    changed
}

criterion_group!(benches, blob_delta_benches);
criterion_main!(benches);
