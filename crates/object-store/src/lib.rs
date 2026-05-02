//! Content-addressed object storage abstractions.

use async_trait::async_trait;
use era_core::ObjectId;
use std::{
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{fs, fs::OpenOptions, io::AsyncWriteExt};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Async blob storage interface used by higher layers.
///
/// The local implementation stores blobs on disk, but callers depend on this
/// capability rather than on a concrete filesystem layout. Future object stores
/// can back the same interface with remote storage or another database.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Stores blob bytes and returns their content-addressed object ID.
    async fn put_blob(&self, bytes: &[u8]) -> Result<ObjectId, ObjectStoreError>;

    /// Reads blob bytes by object ID and verifies their integrity.
    async fn get_blob(&self, id: &ObjectId) -> Result<Vec<u8>, ObjectStoreError>;

    /// Returns `true` when a valid blob exists in the store.
    async fn contains_blob(&self, id: &ObjectId) -> Result<bool, ObjectStoreError>;
}

/// Local filesystem-backed content-addressed object store.
///
/// The v0 store currently supports blobs only. Objects are addressed by the
/// BLAKE3 hash of their bytes and stored under `blobs/<prefix>/<object-id>`.
#[derive(Debug, Clone)]
pub struct LocalObjectStore {
    root: PathBuf,
}

impl LocalObjectStore {
    /// Opens or creates a local object store at `root`.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, ObjectStoreError> {
        let root = root.into();
        let blobs = root.join("blobs");
        fs::create_dir_all(&blobs)
            .await
            .map_err(|source| ObjectStoreError::Io {
                path: blobs,
                source,
            })?;
        Ok(Self { root })
    }

    /// Returns the object store root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stores blob bytes and returns their content-addressed object ID.
    ///
    /// If the same valid blob already exists, no new object file is written.
    /// If the destination path exists with the wrong contents, the corruption is
    /// reported instead of silently overwriting it.
    pub async fn put_blob(
        &self,
        bytes: impl AsRef<[u8]> + Send,
    ) -> Result<ObjectId, ObjectStoreError> {
        self.put_blob_bytes(bytes.as_ref()).await
    }

    /// Reads blob bytes by object ID and verifies their integrity.
    pub async fn get_blob(&self, id: &ObjectId) -> Result<Vec<u8>, ObjectStoreError> {
        self.get_blob_bytes(id).await
    }

    /// Returns `true` when a valid blob exists in the store.
    pub async fn contains_blob(&self, id: &ObjectId) -> Result<bool, ObjectStoreError> {
        self.contains_blob_bytes(id).await
    }

    /// Returns the local path used to store a blob.
    pub fn blob_path(&self, id: &ObjectId) -> PathBuf {
        let hex = id.to_string();
        self.root.join("blobs").join(id.shard_prefix()).join(hex)
    }

    async fn put_blob_bytes(&self, bytes: &[u8]) -> Result<ObjectId, ObjectStoreError> {
        let id = ObjectId::from_content(bytes);
        let path = self.blob_path(&id);

        if fs::try_exists(&path)
            .await
            .map_err(|source| ObjectStoreError::Io {
                path: path.clone(),
                source,
            })?
        {
            match self.verify_blob_file(&path, id).await {
                Ok(()) => return Ok(id),
                Err(ObjectStoreError::MissingBlob { .. }) => {}
                Err(error) => return Err(error),
            }
        }

        let parent = path.parent().expect("blob path always has a parent");
        fs::create_dir_all(parent)
            .await
            .map_err(|source| ObjectStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;

        let temp_path = self.write_temp_blob(parent, id, bytes).await?;
        match fs::hard_link(&temp_path, &path).await {
            Ok(()) => {
                remove_temp_file(&temp_path).await?;
                Ok(id)
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                remove_temp_file(&temp_path).await?;
                self.verify_blob_file(&path, id).await?;
                Ok(id)
            }
            Err(source) => {
                let _ = fs::remove_file(&temp_path).await;
                Err(ObjectStoreError::Io { path, source })
            }
        }
    }

    async fn get_blob_bytes(&self, id: &ObjectId) -> Result<Vec<u8>, ObjectStoreError> {
        let path = self.blob_path(id);
        let bytes = read_blob_file(&path, *id).await?;
        let actual = ObjectId::from_content(&bytes);
        if actual != *id {
            return Err(ObjectStoreError::HashMismatch {
                path,
                expected: *id,
                actual,
            });
        }
        Ok(bytes)
    }

    async fn contains_blob_bytes(&self, id: &ObjectId) -> Result<bool, ObjectStoreError> {
        let path = self.blob_path(id);
        match self.verify_blob_file(&path, *id).await {
            Ok(()) => Ok(true),
            Err(ObjectStoreError::MissingBlob { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn write_temp_blob(
        &self,
        parent: &Path,
        id: ObjectId,
        bytes: &[u8],
    ) -> Result<PathBuf, ObjectStoreError> {
        for _ in 0..16 {
            let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let temp_path = parent.join(format!(".{id}.{}.{counter}.tmp", std::process::id()));

            let mut file = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .await
            {
                Ok(file) => file,
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(ObjectStoreError::Io {
                        path: temp_path,
                        source,
                    });
                }
            };

            if let Err(source) = file.write_all(bytes).await {
                let _ = fs::remove_file(&temp_path).await;
                return Err(ObjectStoreError::Io {
                    path: temp_path,
                    source,
                });
            }

            if let Err(source) = file.flush().await {
                let _ = fs::remove_file(&temp_path).await;
                return Err(ObjectStoreError::Io {
                    path: temp_path,
                    source,
                });
            }

            drop(file);
            return Ok(temp_path);
        }

        Err(ObjectStoreError::TempFileExhausted {
            directory: parent.to_path_buf(),
            id,
        })
    }

    async fn verify_blob_file(
        &self,
        path: &Path,
        expected: ObjectId,
    ) -> Result<(), ObjectStoreError> {
        let bytes = read_blob_file(path, expected).await?;
        let actual = ObjectId::from_content(bytes);
        if actual != expected {
            return Err(ObjectStoreError::HashMismatch {
                path: path.to_path_buf(),
                expected,
                actual,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl BlobStore for LocalObjectStore {
    async fn put_blob(&self, bytes: &[u8]) -> Result<ObjectId, ObjectStoreError> {
        self.put_blob_bytes(bytes).await
    }

    async fn get_blob(&self, id: &ObjectId) -> Result<Vec<u8>, ObjectStoreError> {
        self.get_blob_bytes(id).await
    }

    async fn contains_blob(&self, id: &ObjectId) -> Result<bool, ObjectStoreError> {
        self.contains_blob_bytes(id).await
    }
}

/// Errors returned by the local object store.
#[derive(Debug)]
pub enum ObjectStoreError {
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
    /// A requested blob was not present.
    MissingBlob { id: ObjectId, path: PathBuf },
    /// A blob path existed, but its contents did not hash to the expected ID.
    HashMismatch {
        path: PathBuf,
        expected: ObjectId,
        actual: ObjectId,
    },
    /// The store could not allocate a collision-free temporary path.
    TempFileExhausted { directory: PathBuf, id: ObjectId },
}

impl fmt::Display for ObjectStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(
                formatter,
                "object store filesystem error at {}: {source}",
                path.display()
            ),
            Self::MissingBlob { id, path } => {
                write!(formatter, "missing blob {id} at {}", path.display())
            }
            Self::HashMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "blob integrity check failed at {}: expected {expected}, got {actual}",
                path.display()
            ),
            Self::TempFileExhausted { directory, id } => write!(
                formatter,
                "could not allocate temporary file for blob {id} in {}",
                directory.display()
            ),
        }
    }
}

impl Error for ObjectStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::MissingBlob { .. }
            | Self::HashMismatch { .. }
            | Self::TempFileExhausted { .. } => None,
        }
    }
}

async fn read_blob_file(path: &Path, id: ObjectId) -> Result<Vec<u8>, ObjectStoreError> {
    fs::read(path).await.map_err(|source| match source.kind() {
        io::ErrorKind::NotFound => ObjectStoreError::MissingBlob {
            id,
            path: path.to_path_buf(),
        },
        _ => ObjectStoreError::Io {
            path: path.to_path_buf(),
            source,
        },
    })
}

async fn remove_temp_file(path: &Path) -> Result<(), ObjectStoreError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ObjectStoreError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test(flavor = "current_thread")]
    async fn open_creates_store_directories() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("objects");

        let store = LocalObjectStore::open(&root).await.unwrap();

        assert_eq!(store.root(), root);
        assert!(fs::metadata(root.join("blobs")).await.unwrap().is_dir());
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
        assert_eq!(blob_file_count(store.root()).await, 1);
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

        assert!(matches!(error, ObjectStoreError::MissingBlob { id: found, .. } if found == id));
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
            ObjectStoreError::HashMismatch { expected, .. } if expected == id
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn contains_blob_checks_integrity() {
        let temp = TempDir::new().unwrap();
        let store = LocalObjectStore::open(temp.path().join("objects"))
            .await
            .unwrap();
        let id = store.put_blob(b"hello").await.unwrap();

        assert!(store.contains_blob(&id).await.unwrap());

        fs::write(store.blob_path(&id), b"corrupt").await.unwrap();
        assert!(matches!(
            store.contains_blob(&id).await.unwrap_err(),
            ObjectStoreError::HashMismatch { .. }
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

    async fn blob_file_count(root: &Path) -> usize {
        let blobs = root.join("blobs");
        let mut count = 0;
        let mut shards = fs::read_dir(blobs).await.unwrap();

        while let Some(shard) = shards.next_entry().await.unwrap() {
            if !shard.file_type().await.unwrap().is_dir() {
                continue;
            }

            let mut files = fs::read_dir(shard.path()).await.unwrap();
            while let Some(file) = files.next_entry().await.unwrap() {
                if file.file_type().await.unwrap().is_file() {
                    count += 1;
                }
            }
        }

        count
    }
}
