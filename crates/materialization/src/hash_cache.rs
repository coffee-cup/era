use era_core::ObjectId;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// In-memory file hash cache for a single materialized workspace.
#[derive(Debug, Default)]
pub(crate) struct HashCache {
    entries: HashMap<PathBuf, CachedFile>,
}

impl HashCache {
    pub(crate) fn get(&self, path: &Path, fingerprint: &FileFingerprint) -> Option<CachedFileHash> {
        self.entries
            .get(path)
            .filter(|entry| entry.fingerprint == *fingerprint)
            .map(|entry| CachedFileHash {
                object_id: entry.object_id,
                stored: entry.stored,
            })
    }

    pub(crate) fn insert(
        &mut self,
        path: impl Into<PathBuf>,
        fingerprint: FileFingerprint,
        object_id: ObjectId,
        stored: bool,
    ) {
        self.entries.insert(
            path.into(),
            CachedFile {
                fingerprint,
                object_id,
                stored,
            },
        );
    }

    pub(crate) fn invalidate_path(&mut self, path: &Path) {
        if path.as_os_str().is_empty() {
            self.entries.clear();
            return;
        }

        self.entries
            .retain(|cached_path, _| cached_path != path && !cached_path.starts_with(path));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedFile {
    fingerprint: FileFingerprint,
    object_id: ObjectId,
    stored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CachedFileHash {
    pub(crate) object_id: ObjectId,
    pub(crate) stored: bool,
}

/// Filesystem metadata used to decide whether a cached file hash is reusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

impl FileFingerprint {
    pub(crate) fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            dev: metadata.dev(),
            #[cfg(unix)]
            ino: metadata.ino(),
        }
    }
}
