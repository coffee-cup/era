use crate::{ObjectStore, ObjectStoreError};
use async_trait::async_trait;
use era_core::{ObjectId, ObjectKind, Tree};
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{fs, fs::OpenOptions, io::AsyncWriteExt};
use tracing::{debug, warn};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Local filesystem-backed content-addressed object store.
///
/// Objects are addressed by the BLAKE3 hash of their bytes and stored under
/// `<kind>/<prefix>/<object-id>`.
#[derive(Debug, Clone)]
pub struct LocalObjectStore {
    root: PathBuf,
}

impl LocalObjectStore {
    /// Opens or creates a local object store at `root`.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, ObjectStoreError> {
        let root = root.into();
        debug!(root = %root.display(), "opening local object store");

        for kind in [ObjectKind::Blob, ObjectKind::Tree] {
            let directory = object_dir(&root, kind);
            fs::create_dir_all(&directory)
                .await
                .map_err(|source| ObjectStoreError::Io {
                    path: directory,
                    source,
                })?;
        }

        debug!(root = %root.display(), "local object store ready");
        Ok(Self { root })
    }

    /// Returns the object store root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stores blob bytes and returns their content-addressed object ID.
    pub async fn put_blob(
        &self,
        bytes: impl AsRef<[u8]> + Send,
    ) -> Result<ObjectId, ObjectStoreError> {
        let bytes = bytes.as_ref();
        let id = ObjectId::from_content(bytes);
        self.put_object(ObjectKind::Blob, id, bytes).await
    }

    /// Reads blob bytes by object ID and verifies their integrity.
    pub async fn get_blob(&self, id: &ObjectId) -> Result<Vec<u8>, ObjectStoreError> {
        self.get_object_bytes(ObjectKind::Blob, id).await
    }

    /// Stores a tree and returns the ID of its canonical serialized bytes.
    pub async fn put_tree(&self, tree: &Tree) -> Result<ObjectId, ObjectStoreError> {
        let bytes = tree.to_canonical_bytes();
        let id = ObjectId::from_content(&bytes);
        self.put_object(ObjectKind::Tree, id, &bytes).await
    }

    /// Reads a tree by object ID and verifies its integrity and canonical form.
    pub async fn get_tree(&self, id: &ObjectId) -> Result<Tree, ObjectStoreError> {
        let path = self.object_path(ObjectKind::Tree, id);
        let bytes = self.get_object_bytes(ObjectKind::Tree, id).await?;
        Tree::from_canonical_bytes(&bytes).map_err(|source| ObjectStoreError::InvalidTreeObject {
            id: *id,
            path,
            source,
        })
    }

    /// Returns `true` when a valid object exists in the store.
    pub async fn contains(
        &self,
        kind: ObjectKind,
        id: &ObjectId,
    ) -> Result<bool, ObjectStoreError> {
        match kind {
            ObjectKind::Blob => self.contains_object_hash(kind, id).await,
            ObjectKind::Tree => match self.get_tree(id).await {
                Ok(_) => Ok(true),
                Err(ObjectStoreError::MissingObject { .. }) => Ok(false),
                Err(error) => Err(error),
            },
        }
    }

    /// Returns the local path used to store an object.
    pub fn object_path(&self, kind: ObjectKind, id: &ObjectId) -> PathBuf {
        let hex = id.to_string();
        object_dir(&self.root, kind)
            .join(id.shard_prefix())
            .join(hex)
    }

    /// Returns the local path used to store a blob.
    pub fn blob_path(&self, id: &ObjectId) -> PathBuf {
        self.object_path(ObjectKind::Blob, id)
    }

    /// Returns the local path used to store a tree.
    pub fn tree_path(&self, id: &ObjectId) -> PathBuf {
        self.object_path(ObjectKind::Tree, id)
    }

    async fn put_object(
        &self,
        kind: ObjectKind,
        id: ObjectId,
        bytes: &[u8],
    ) -> Result<ObjectId, ObjectStoreError> {
        let path = self.object_path(kind, &id);
        debug!(%kind, %id, bytes = bytes.len(), path = %path.display(), "storing object");

        if fs::try_exists(&path)
            .await
            .map_err(|source| ObjectStoreError::Io {
                path: path.clone(),
                source,
            })?
        {
            match self.verify_object_file(kind, &path, id).await {
                Ok(()) => {
                    debug!(%kind, %id, "object already present; reusing existing object");
                    return Ok(id);
                }
                Err(ObjectStoreError::MissingObject { .. }) => {}
                Err(error) => return Err(error),
            }
        }

        let parent = path.parent().expect("object path always has a parent");
        fs::create_dir_all(parent)
            .await
            .map_err(|source| ObjectStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;

        let temp_path = self.write_temp_object(kind, parent, id, bytes).await?;
        match fs::hard_link(&temp_path, &path).await {
            Ok(()) => {
                remove_temp_file(&temp_path).await?;
                debug!(%kind, %id, path = %path.display(), "stored new object");
                Ok(id)
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                remove_temp_file(&temp_path).await?;
                self.verify_object_file(kind, &path, id).await?;
                debug!(%kind, %id, "object appeared during concurrent write; reusing existing object");
                Ok(id)
            }
            Err(source) => {
                let _ = fs::remove_file(&temp_path).await;
                Err(ObjectStoreError::Io { path, source })
            }
        }
    }

    async fn get_object_bytes(
        &self,
        kind: ObjectKind,
        id: &ObjectId,
    ) -> Result<Vec<u8>, ObjectStoreError> {
        let path = self.object_path(kind, id);
        debug!(%kind, %id, path = %path.display(), "reading object");
        let bytes = read_object_file(kind, &path, *id).await?;
        let actual = ObjectId::from_content(&bytes);
        if actual != *id {
            warn!(%kind, %id, %actual, path = %path.display(), "object failed integrity check on read");
            return Err(ObjectStoreError::HashMismatch {
                kind,
                path,
                expected: *id,
                actual,
            });
        }
        Ok(bytes)
    }

    async fn contains_object_hash(
        &self,
        kind: ObjectKind,
        id: &ObjectId,
    ) -> Result<bool, ObjectStoreError> {
        let path = self.object_path(kind, id);
        debug!(%kind, %id, path = %path.display(), "checking object presence");
        match self.verify_object_file(kind, &path, *id).await {
            Ok(()) => Ok(true),
            Err(ObjectStoreError::MissingObject { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn write_temp_object(
        &self,
        kind: ObjectKind,
        parent: &Path,
        id: ObjectId,
        bytes: &[u8],
    ) -> Result<PathBuf, ObjectStoreError> {
        for _ in 0..16 {
            let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let temp_path = parent.join(format!(".{id}.{}.{counter}.tmp", std::process::id()));

            debug!(%kind, %id, temp_path = %temp_path.display(), "writing temporary object file");
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
            kind,
            directory: parent.to_path_buf(),
            id,
        })
    }

    async fn verify_object_file(
        &self,
        kind: ObjectKind,
        path: &Path,
        expected: ObjectId,
    ) -> Result<(), ObjectStoreError> {
        let bytes = read_object_file(kind, path, expected).await?;
        let actual = ObjectId::from_content(bytes);
        if actual != expected {
            warn!(%kind, %expected, %actual, path = %path.display(), "object failed integrity verification");
            return Err(ObjectStoreError::HashMismatch {
                kind,
                path: path.to_path_buf(),
                expected,
                actual,
            });
        }
        Ok(())
    }
}

#[async_trait]
impl ObjectStore for LocalObjectStore {
    async fn put_blob(&self, bytes: &[u8]) -> Result<ObjectId, ObjectStoreError> {
        LocalObjectStore::put_blob(self, bytes).await
    }

    async fn get_blob(&self, id: &ObjectId) -> Result<Vec<u8>, ObjectStoreError> {
        LocalObjectStore::get_blob(self, id).await
    }

    async fn put_tree(&self, tree: &Tree) -> Result<ObjectId, ObjectStoreError> {
        LocalObjectStore::put_tree(self, tree).await
    }

    async fn get_tree(&self, id: &ObjectId) -> Result<Tree, ObjectStoreError> {
        LocalObjectStore::get_tree(self, id).await
    }

    async fn contains(&self, kind: ObjectKind, id: &ObjectId) -> Result<bool, ObjectStoreError> {
        LocalObjectStore::contains(self, kind, id).await
    }
}

async fn read_object_file(
    kind: ObjectKind,
    path: &Path,
    id: ObjectId,
) -> Result<Vec<u8>, ObjectStoreError> {
    fs::read(path).await.map_err(|source| match source.kind() {
        io::ErrorKind::NotFound => ObjectStoreError::MissingObject {
            kind,
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

pub(crate) fn object_dir(root: &Path, kind: ObjectKind) -> PathBuf {
    root.join(match kind {
        ObjectKind::Blob => "blobs",
        ObjectKind::Tree => "trees",
    })
}
