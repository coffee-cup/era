use era_core::{OBJECT_ID_BYTES, ObjectId, Tree, TreeEntry};
use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const FILES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("files");
const DIRECTORIES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("directories");
const FILE_RECORD_VERSION: u8 = 2;
const DIRECTORY_RECORD_VERSION: u8 = 1;
static DATABASE_REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<Database>>>> = OnceLock::new();

/// Workspace-local cache of file fingerprints and stored tree structure.
#[derive(Debug, Clone, Default)]
pub(crate) struct CaptureCache {
    files: HashMap<PathBuf, CachedFile>,
    directories: HashMap<PathBuf, CachedDirectory>,
    dirty_files: HashSet<PathBuf>,
    dirty_directories: HashSet<PathBuf>,
    invalidations: Vec<PathBuf>,
    backend: Option<Arc<Database>>,
    loaded_all: bool,
}

impl CaptureCache {
    pub(crate) async fn load(path: &Path) -> Self {
        match open_database(path) {
            Ok(backend) => Self {
                backend: Some(backend),
                ..Self::default()
            },
            Err(message) => {
                warn!(path = %path.display(), %message, "capture cache database could not be opened; using in-memory cache");
                Self::default()
            }
        }
    }

    pub(crate) fn load_all(&mut self) -> Result<(), String> {
        if self.loaded_all {
            return Ok(());
        }
        let Some(backend) = self.backend.clone() else {
            self.loaded_all = true;
            return Ok(());
        };

        let read = backend.begin_read().map_err(error_to_string)?;
        {
            let table = read.open_table(FILES_TABLE).map_err(error_to_string)?;
            let entries = table.iter().map_err(error_to_string)?;
            for entry in entries {
                let (key, value) = entry.map_err(error_to_string)?;
                let path = decode_key(key.value())?;
                if self.files.contains_key(&path) || self.is_invalidated_path(&path) {
                    continue;
                }
                let file = decode_file(value.value())?;
                self.files.insert(path, file);
            }
        }
        {
            let table = read
                .open_table(DIRECTORIES_TABLE)
                .map_err(error_to_string)?;
            let entries = table.iter().map_err(error_to_string)?;
            for entry in entries {
                let (key, value) = entry.map_err(error_to_string)?;
                let path = decode_key(key.value())?;
                if self.directories.contains_key(&path) || self.is_invalidated_path(&path) {
                    continue;
                }
                let directory = decode_directory(value.value())?;
                self.directories.insert(path, directory);
            }
        }

        self.loaded_all = true;
        Ok(())
    }

    pub(crate) fn drop_bulk_loaded_records(&mut self) {
        if self.backend.is_none() || !self.loaded_all {
            return;
        }

        self.files.retain(|path, _| self.dirty_files.contains(path));
        self.directories
            .retain(|path, _| self.dirty_directories.contains(path));
        self.loaded_all = false;
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        let Some(backend) = self.backend.clone() else {
            self.invalidations.clear();
            self.dirty_files.clear();
            self.dirty_directories.clear();
            return Ok(());
        };
        if self.invalidations.is_empty()
            && self.dirty_files.is_empty()
            && self.dirty_directories.is_empty()
        {
            return Ok(());
        }

        let invalidations = self.invalidations.clone();
        let dirty_files = self.dirty_files.iter().cloned().collect::<Vec<_>>();
        let dirty_directories = self.dirty_directories.iter().cloned().collect::<Vec<_>>();

        let mut write = backend.begin_write().map_err(redb_io_error)?;
        write
            .set_durability(Durability::None)
            .map_err(redb_io_error)?;
        {
            let mut table = write.open_table(FILES_TABLE).map_err(redb_io_error)?;
            for path in &invalidations {
                remove_path_from_table(&mut table, path).map_err(io::Error::other)?;
            }
            for path in &dirty_files {
                let key = encode_key(path).map_err(io::Error::other)?;
                match self.files.get(path) {
                    Some(file) => {
                        let value = encode_file(file);
                        table
                            .insert(key.as_str(), value.as_slice())
                            .map_err(redb_io_error)?;
                    }
                    None => {
                        table.remove(key.as_str()).map_err(redb_io_error)?;
                    }
                }
            }
        }
        {
            let mut table = write.open_table(DIRECTORIES_TABLE).map_err(redb_io_error)?;
            for path in &invalidations {
                remove_path_from_table(&mut table, path).map_err(io::Error::other)?;
            }
            for path in &dirty_directories {
                let key = encode_key(path).map_err(io::Error::other)?;
                match self.directories.get(path) {
                    Some(directory) => {
                        let value = encode_directory(directory).map_err(io::Error::other)?;
                        table
                            .insert(key.as_str(), value.as_slice())
                            .map_err(redb_io_error)?;
                    }
                    None => {
                        table.remove(key.as_str()).map_err(redb_io_error)?;
                    }
                }
            }
        }
        write.commit().map_err(redb_io_error)?;

        self.invalidations.clear();
        self.dirty_files.clear();
        self.dirty_directories.clear();
        Ok(())
    }

    pub(crate) fn get_file(
        &mut self,
        path: &Path,
        fingerprint: &FileFingerprint,
    ) -> Option<CachedFileHash> {
        self.file_lookup(path, fingerprint).matching
    }

    pub(crate) fn file_lookup(
        &mut self,
        path: &Path,
        fingerprint: &FileFingerprint,
    ) -> CachedFileLookup {
        let Some(entry) = self.current_file(path) else {
            return CachedFileLookup::default();
        };
        CachedFileLookup {
            matching: (entry.fingerprint == *fingerprint).then_some(CachedFileHash {
                object_id: entry.object_id,
                stored: entry.stored,
            }),
            stored_base_id: if entry.stored {
                Some(entry.object_id)
            } else {
                entry.stored_base_id
            },
        }
    }

    pub(crate) fn insert_file(
        &mut self,
        path: impl Into<PathBuf>,
        fingerprint: FileFingerprint,
        object_id: ObjectId,
        stored: bool,
    ) -> bool {
        let path = path.into();
        let previous = self.current_file(&path);
        let stored = stored
            || previous
                .as_ref()
                .is_some_and(|entry| entry.object_id == object_id && entry.stored);
        let stored_base_id = if stored {
            None
        } else {
            previous.as_ref().and_then(|entry| {
                if entry.stored {
                    Some(entry.object_id)
                } else {
                    entry.stored_base_id
                }
            })
        };
        let next = CachedFile {
            fingerprint,
            object_id,
            stored,
            stored_base_id,
        };
        if previous.as_ref() == Some(&next) {
            self.files.insert(path, next);
            return false;
        }
        self.files.insert(path.clone(), next);
        self.dirty_files.insert(path);
        true
    }

    pub(crate) fn get_directory(&mut self, path: &Path) -> Option<CachedDirectoryTree> {
        self.current_directory(path)
            .map(|directory| CachedDirectoryTree {
                tree_id: directory.tree_id,
                entries: directory.entries,
                stored: directory.stored,
            })
    }

    pub(crate) fn insert_directory(
        &mut self,
        path: impl Into<PathBuf>,
        tree_id: ObjectId,
        entries: Vec<TreeEntry>,
        stored: bool,
    ) -> bool {
        let path = path.into();
        let previous = self.current_directory(&path);
        let stored = stored
            || previous
                .as_ref()
                .is_some_and(|directory| directory.tree_id == tree_id && directory.stored);
        let next = CachedDirectory {
            tree_id,
            entries,
            stored,
        };
        if previous.as_ref() == Some(&next) {
            self.directories.insert(path, next);
            return false;
        }
        self.directories.insert(path.clone(), next);
        self.dirty_directories.insert(path);
        true
    }

    pub(crate) fn update_directory_entries(
        &mut self,
        path: &Path,
        entries: Vec<TreeEntry>,
    ) -> bool {
        let Some(previous) = self.current_directory(path) else {
            return false;
        };
        if previous.entries == entries && !previous.stored {
            self.directories.insert(path.to_path_buf(), previous);
            return false;
        }

        let Ok(tree) = Tree::new(entries) else {
            return false;
        };
        let next = CachedDirectory {
            tree_id: tree.id(),
            entries: tree.entries().to_vec(),
            stored: false,
        };
        self.directories.insert(path.to_path_buf(), next);
        self.dirty_directories.insert(path.to_path_buf());
        true
    }

    pub(crate) fn invalidate_path(&mut self, path: &Path) -> bool {
        if self.backend.is_none() {
            return self.invalidate_in_memory(path);
        }

        let path = path.to_path_buf();
        self.files
            .retain(|cached_path, _| !path_matches(cached_path, &path));
        self.directories
            .retain(|cached_path, _| !path_matches(cached_path, &path));
        self.dirty_files
            .retain(|cached_path| !path_matches(cached_path, &path));
        self.dirty_directories
            .retain(|cached_path| !path_matches(cached_path, &path));
        self.invalidations.push(path);
        true
    }

    fn invalidate_in_memory(&mut self, path: &Path) -> bool {
        if path.as_os_str().is_empty() {
            let changed = !self.files.is_empty() || !self.directories.is_empty();
            self.files.clear();
            self.directories.clear();
            return changed;
        }

        let file_count = self.files.len();
        let directory_count = self.directories.len();
        self.files
            .retain(|cached_path, _| !path_matches(cached_path, path));
        self.directories
            .retain(|cached_path, _| !path_matches(cached_path, path));
        self.files.len() != file_count || self.directories.len() != directory_count
    }

    fn current_file(&mut self, path: &Path) -> Option<CachedFile> {
        if let Some(file) = self.files.get(path) {
            return Some(file.clone());
        }
        if self.is_invalidated_path(path) {
            return None;
        }
        let file = self.read_file(path);
        if let Some(file) = &file {
            self.files.insert(path.to_path_buf(), file.clone());
        }
        file
    }

    fn current_directory(&mut self, path: &Path) -> Option<CachedDirectory> {
        if let Some(directory) = self.directories.get(path) {
            return Some(directory.clone());
        }
        if self.is_invalidated_path(path) {
            return None;
        }
        let directory = self.read_directory(path);
        if let Some(directory) = &directory {
            self.directories
                .insert(path.to_path_buf(), directory.clone());
        }
        directory
    }

    fn read_file(&self, path: &Path) -> Option<CachedFile> {
        let Some(backend) = &self.backend else {
            return None;
        };
        let key = match encode_key(path) {
            Ok(key) => key,
            Err(message) => {
                warn!(path = %path.display(), %message, "capture cache path could not be encoded");
                return None;
            }
        };
        let read = match backend.begin_read() {
            Ok(read) => read,
            Err(error) => {
                warn!(path = %path.display(), error = %error, "capture cache read transaction failed");
                return None;
            }
        };
        let table = match read.open_table(FILES_TABLE) {
            Ok(table) => table,
            Err(error) => {
                warn!(path = %path.display(), error = %error, "capture cache file table could not be opened");
                return None;
            }
        };
        let value = match table.get(key.as_str()) {
            Ok(value) => value,
            Err(error) => {
                warn!(path = %path.display(), error = %error, "capture cache file record could not be read");
                return None;
            }
        };
        value.and_then(|value| match decode_file(value.value()) {
            Ok(file) => Some(file),
            Err(message) => {
                warn!(path = %path.display(), %message, "capture cache file record could not be decoded");
                None
            }
        })
    }

    fn read_directory(&self, path: &Path) -> Option<CachedDirectory> {
        let Some(backend) = &self.backend else {
            return None;
        };
        let key = match encode_key(path) {
            Ok(key) => key,
            Err(message) => {
                warn!(path = %path.display(), %message, "capture cache path could not be encoded");
                return None;
            }
        };
        let read = match backend.begin_read() {
            Ok(read) => read,
            Err(error) => {
                warn!(path = %path.display(), error = %error, "capture cache read transaction failed");
                return None;
            }
        };
        let table = match read.open_table(DIRECTORIES_TABLE) {
            Ok(table) => table,
            Err(error) => {
                warn!(path = %path.display(), error = %error, "capture cache directory table could not be opened");
                return None;
            }
        };
        let value = match table.get(key.as_str()) {
            Ok(value) => value,
            Err(error) => {
                warn!(path = %path.display(), error = %error, "capture cache directory record could not be read");
                return None;
            }
        };
        value.and_then(|value| match decode_directory(value.value()) {
            Ok(directory) => Some(directory),
            Err(message) => {
                warn!(path = %path.display(), %message, "capture cache directory record could not be decoded");
                None
            }
        })
    }

    fn is_invalidated_path(&self, path: &Path) -> bool {
        self.invalidations
            .iter()
            .any(|invalidated| path_matches(path, invalidated))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedFile {
    fingerprint: FileFingerprint,
    object_id: ObjectId,
    stored: bool,
    stored_base_id: Option<ObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedDirectory {
    tree_id: ObjectId,
    entries: Vec<TreeEntry>,
    stored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedDirectoryTree {
    pub(crate) tree_id: ObjectId,
    pub(crate) entries: Vec<TreeEntry>,
    pub(crate) stored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CachedFileHash {
    pub(crate) object_id: ObjectId,
    pub(crate) stored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct CachedFileLookup {
    pub(crate) matching: Option<CachedFileHash>,
    pub(crate) stored_base_id: Option<ObjectId>,
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

    fn from_parts(
        len: u64,
        modified: Option<SystemTime>,
        dev: Option<u64>,
        ino: Option<u64>,
    ) -> Result<Self, String> {
        #[cfg(unix)]
        {
            Ok(Self {
                len,
                modified,
                dev: dev.ok_or_else(|| "missing unix dev fingerprint".to_owned())?,
                ino: ino.ok_or_else(|| "missing unix ino fingerprint".to_owned())?,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (dev, ino);
            Ok(Self { len, modified })
        }
    }

    fn len(&self) -> u64 {
        self.len
    }

    fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    fn dev(&self) -> Option<u64> {
        #[cfg(unix)]
        {
            Some(self.dev)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    fn ino(&self) -> Option<u64> {
        #[cfg(unix)]
        {
            Some(self.ino)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

fn open_database(path: &Path) -> Result<Arc<Database>, String> {
    let path = path.to_path_buf();
    let registry = DATABASE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(database) = registry
        .lock()
        .expect("capture cache database registry mutex poisoned")
        .get(&path)
        .and_then(Weak::upgrade)
    {
        return Ok(database);
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let database = match Database::create(&path) {
        Ok(database) => initialize_database(database).map(Arc::new),
        Err(first_error) => {
            warn!(path = %path.display(), error = %first_error, "capture cache database could not be opened; rebuilding");
            let _ = std::fs::remove_file(&path);
            let database = Database::create(&path)
                .map_err(|second_error| format!("{first_error}; rebuild failed: {second_error}"))?;
            initialize_database(database).map(Arc::new)
        }
    }?;

    registry
        .lock()
        .expect("capture cache database registry mutex poisoned")
        .insert(path, Arc::downgrade(&database));
    Ok(database)
}

fn initialize_database(database: Database) -> Result<Database, String> {
    let mut write = database.begin_write().map_err(error_to_string)?;
    write
        .set_durability(Durability::None)
        .map_err(error_to_string)?;
    {
        write.open_table(FILES_TABLE).map_err(error_to_string)?;
        write
            .open_table(DIRECTORIES_TABLE)
            .map_err(error_to_string)?;
    }
    write.commit().map_err(error_to_string)?;
    Ok(database)
}

fn remove_path_from_table(
    table: &mut redb::Table<'_, &str, &[u8]>,
    path: &Path,
) -> Result<(), String> {
    let key = encode_key(path)?;
    table.remove(key.as_str()).map_err(error_to_string)?;

    if !key.is_empty() {
        let child_prefix = format!("{key}/");
        let mut keys = Vec::new();
        let entries = table
            .range(child_prefix.as_str()..)
            .map_err(error_to_string)?;
        for entry in entries {
            let (entry_key, _) = entry.map_err(error_to_string)?;
            let entry_key = entry_key.value();
            if !entry_key.starts_with(&child_prefix) {
                break;
            }
            keys.push(entry_key.to_owned());
        }
        for key in keys {
            table.remove(key.as_str()).map_err(error_to_string)?;
        }
    } else {
        let keys = table
            .iter()
            .map_err(error_to_string)?
            .map(|entry| {
                entry
                    .map(|(key, _)| key.value().to_owned())
                    .map_err(error_to_string)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for key in keys {
            table.remove(key.as_str()).map_err(error_to_string)?;
        }
    }

    Ok(())
}

fn path_matches(path: &Path, invalidated: &Path) -> bool {
    invalidated.as_os_str().is_empty() || path == invalidated || path.starts_with(invalidated)
}

fn encode_key(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let Some(part) = part.to_str() else {
                    return Err(format!("path is not UTF-8: {}", path.display()));
                };
                parts.push(part);
            }
            Component::CurDir => {}
            _ => return Err(format!("path is not relative: {}", path.display())),
        }
    }
    Ok(parts.join("/"))
}

fn decode_key(value: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Ok(PathBuf::new());
    }
    let mut path = PathBuf::new();
    for part in value.split('/') {
        if part.is_empty() {
            return Err("cache key contains empty path segment".to_owned());
        }
        path.push(part);
    }
    Ok(path)
}

fn encode_file(file: &CachedFile) -> Vec<u8> {
    let mut output = Vec::with_capacity(
        1 + 1 + 8 + 1 + 8 + 4 + 1 + 8 + 1 + 8 + OBJECT_ID_BYTES + 1 + OBJECT_ID_BYTES,
    );
    output.push(FILE_RECORD_VERSION);
    output.push(u8::from(file.stored));
    write_u64(&mut output, file.fingerprint.len());
    let (sign, secs, nanos) = format_time(file.fingerprint.modified());
    output.push(sign);
    write_u64(&mut output, secs);
    write_u32(&mut output, nanos);
    write_optional_u64(&mut output, file.fingerprint.dev());
    write_optional_u64(&mut output, file.fingerprint.ino());
    output.extend_from_slice(file.object_id.as_bytes());
    write_optional_object_id(&mut output, file.stored_base_id.as_ref());
    output
}

fn decode_file(bytes: &[u8]) -> Result<CachedFile, String> {
    let mut cursor = BytesCursor::new(bytes);
    let version = cursor.read_u8()?;
    if !matches!(version, 1 | FILE_RECORD_VERSION) {
        return Err(format!("unsupported cache record version: {version}"));
    }
    let stored = cursor.read_bool()?;
    let len = cursor.read_u64()?;
    let sign = cursor.read_u8()?;
    let secs = cursor.read_u64()?;
    let nanos = cursor.read_u32()?;
    let modified = parse_time(sign, secs, nanos)?;
    let dev = cursor.read_optional_u64()?;
    let ino = cursor.read_optional_u64()?;
    let object_id = cursor.read_object_id()?;
    let stored_base_id = if version >= 2 {
        cursor.read_optional_object_id()?
    } else {
        None
    };
    cursor.finish()?;
    Ok(CachedFile {
        fingerprint: FileFingerprint::from_parts(len, modified, dev, ino)?,
        object_id,
        stored,
        stored_base_id,
    })
}

fn encode_directory(directory: &CachedDirectory) -> Result<Vec<u8>, String> {
    let tree = Tree::new(directory.entries.clone()).map_err(|error| error.to_string())?;
    let tree_id = tree.id();
    if tree_id != directory.tree_id {
        return Err("directory cache tree ID does not match entries".to_owned());
    }

    let tree_bytes = tree.to_canonical_bytes();
    let mut output = Vec::with_capacity(2 + tree_bytes.len());
    output.push(DIRECTORY_RECORD_VERSION);
    output.push(u8::from(directory.stored));
    output.extend_from_slice(&tree_bytes);
    Ok(output)
}

fn decode_directory(bytes: &[u8]) -> Result<CachedDirectory, String> {
    let mut cursor = BytesCursor::new(bytes);
    cursor.expect_version(DIRECTORY_RECORD_VERSION)?;
    let stored = cursor.read_bool()?;
    let tree = Tree::from_canonical_bytes(cursor.remaining()).map_err(|error| error.to_string())?;
    Ok(CachedDirectory {
        tree_id: tree.id(),
        entries: tree.entries().to_vec(),
        stored,
    })
}

fn write_optional_u64(output: &mut Vec<u8>, value: Option<u64>) {
    match value {
        Some(value) => {
            output.push(1);
            write_u64(output, value);
        }
        None => {
            output.push(0);
            write_u64(output, 0);
        }
    }
}

fn write_optional_object_id(output: &mut Vec<u8>, value: Option<&ObjectId>) {
    match value {
        Some(value) => {
            output.push(1);
            output.extend_from_slice(value.as_bytes());
        }
        None => {
            output.push(0);
            output.extend_from_slice(&[0; OBJECT_ID_BYTES]);
        }
    }
}

fn write_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn write_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn parse_time(sign: u8, secs: u64, nanos: u32) -> Result<Option<SystemTime>, String> {
    let duration = Duration::new(secs, nanos);
    match sign {
        0 => Ok(None),
        1 => Ok(Some(UNIX_EPOCH + duration)),
        2 => Ok(Some(UNIX_EPOCH - duration)),
        _ => Err(format!("invalid time sign: {sign}")),
    }
}

fn format_time(time: Option<SystemTime>) -> (u8, u64, u32) {
    let Some(time) = time else {
        return (0, 0, 0);
    };

    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => (1, duration.as_secs(), duration.subsec_nanos()),
        Err(error) => {
            let duration = error.duration();
            (2, duration.as_secs(), duration.subsec_nanos())
        }
    }
}

#[derive(Debug)]
struct BytesCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> BytesCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn expect_version(&mut self, expected: u8) -> Result<(), String> {
        let actual = self.read_u8()?;
        if actual == expected {
            Ok(())
        } else {
            Err(format!("unsupported cache record version: {actual}"))
        }
    }

    fn read_bool(&mut self) -> Result<bool, String> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(format!("invalid bool: {value}")),
        }
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        let bytes = self.read_exact(1)?;
        Ok(bytes[0])
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let bytes = self.read_exact(4)?;
        let mut value = [0_u8; 4];
        value.copy_from_slice(bytes);
        Ok(u32::from_be_bytes(value))
    }

    fn read_u64(&mut self) -> Result<u64, String> {
        let bytes = self.read_exact(8)?;
        let mut value = [0_u8; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(value))
    }

    fn read_optional_u64(&mut self) -> Result<Option<u64>, String> {
        let present = self.read_bool()?;
        let value = self.read_u64()?;
        Ok(present.then_some(value))
    }

    fn read_object_id(&mut self) -> Result<ObjectId, String> {
        let bytes = self.read_exact(OBJECT_ID_BYTES)?;
        let mut id = [0_u8; OBJECT_ID_BYTES];
        id.copy_from_slice(bytes);
        Ok(ObjectId::from_bytes(id))
    }

    fn read_optional_object_id(&mut self) -> Result<Option<ObjectId>, String> {
        let present = self.read_bool()?;
        let value = self.read_object_id()?;
        Ok(present.then_some(value))
    }

    fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.offset..]
    }

    fn finish(&self) -> Result<(), String> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err("cache record has trailing bytes".to_owned())
        }
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| "cache record length overflow".to_owned())?;
        if end > self.bytes.len() {
            return Err("cache record ended early".to_owned());
        }
        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
}

fn redb_io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn error_to_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}
