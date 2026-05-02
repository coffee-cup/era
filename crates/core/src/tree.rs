use crate::{OBJECT_ID_BYTES, ObjectId};
use std::{error::Error, fmt};

const TREE_MAGIC: &[u8] = b"ERA_TREE_V1\0";

/// Kind of entry in a tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// Entry points at a blob object.
    Blob,
    /// Entry points at another tree object.
    Tree,
}

impl EntryKind {
    fn tag(self) -> u8 {
        match self {
            Self::Blob => b'b',
            Self::Tree => b't',
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            b'b' => Some(Self::Blob),
            b't' => Some(Self::Tree),
            _ => None,
        }
    }
}

impl fmt::Display for EntryKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob => formatter.write_str("blob"),
            Self::Tree => formatter.write_str("tree"),
        }
    }
}

/// A single named entry in a directory tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TreeEntry {
    name: String,
    kind: EntryKind,
    id: ObjectId,
}

impl TreeEntry {
    /// Creates a tree entry after validating that `name` is one path segment.
    pub fn new(name: impl Into<String>, kind: EntryKind, id: ObjectId) -> Result<Self, TreeError> {
        let name = name.into();
        validate_entry_name(&name)?;
        Ok(Self { name, kind, id })
    }

    /// Creates a blob tree entry.
    pub fn blob(name: impl Into<String>, id: ObjectId) -> Result<Self, TreeError> {
        Self::new(name, EntryKind::Blob, id)
    }

    /// Creates a nested-tree entry.
    pub fn tree(name: impl Into<String>, id: ObjectId) -> Result<Self, TreeError> {
        Self::new(name, EntryKind::Tree, id)
    }

    /// Returns the entry name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the entry kind.
    #[must_use]
    pub fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Returns the object ID the entry points at.
    #[must_use]
    pub fn id(&self) -> ObjectId {
        self.id
    }
}

/// A deterministic directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tree {
    entries: Vec<TreeEntry>,
}

impl Tree {
    /// Creates a tree from entries, sorting by the exact UTF-8 bytes of each
    /// name and rejecting duplicate names.
    pub fn new(entries: impl IntoIterator<Item = TreeEntry>) -> Result<Self, TreeError> {
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
        reject_duplicate_names(&entries)?;
        Ok(Self { entries })
    }

    /// Creates an empty tree.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Returns sorted tree entries.
    #[must_use]
    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    /// Returns canonical serialized tree bytes.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(TREE_MAGIC);
        write_u32(&mut output, self.entries.len());

        for entry in &self.entries {
            let name = entry.name.as_bytes();
            output.push(entry.kind.tag());
            write_u32(&mut output, name.len());
            output.extend_from_slice(name);
            output.extend_from_slice(entry.id.as_bytes());
        }

        output
    }

    /// Parses a canonical serialized tree.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, TreeError> {
        let mut cursor = TreeCursor::new(bytes);
        cursor.read_magic()?;
        let entry_count = cursor.read_u32()? as usize;
        let mut entries = Vec::new();
        let mut previous_name: Option<String> = None;

        for index in 0..entry_count {
            let kind_tag = cursor.read_byte()?;
            let kind = EntryKind::from_tag(kind_tag).ok_or(TreeError::InvalidEntryKind {
                index,
                value: kind_tag,
            })?;
            let name_length = cursor.read_u32()? as usize;
            let name_bytes = cursor.read_exact(name_length)?;
            let name = String::from_utf8(name_bytes.to_vec())
                .map_err(|_| TreeError::NameNotUtf8 { index })?;
            validate_entry_name(&name)?;

            if let Some(previous) = &previous_name {
                match previous.as_bytes().cmp(name.as_bytes()) {
                    std::cmp::Ordering::Less => {}
                    std::cmp::Ordering::Equal => {
                        return Err(TreeError::DuplicateName { name });
                    }
                    std::cmp::Ordering::Greater => {
                        return Err(TreeError::NonCanonicalOrder {
                            previous: previous.clone(),
                            current: name,
                        });
                    }
                }
            }

            let id_bytes = cursor.read_exact(OBJECT_ID_BYTES)?;
            let mut id = [0_u8; OBJECT_ID_BYTES];
            id.copy_from_slice(id_bytes);
            previous_name = Some(name.clone());
            entries.push(TreeEntry {
                name,
                kind,
                id: ObjectId::from_bytes(id),
            });
        }

        if !cursor.is_finished() {
            return Err(TreeError::TrailingBytes {
                offset: cursor.offset,
                len: bytes.len(),
            });
        }

        Ok(Self { entries })
    }

    /// Returns the content-addressed ID of this tree's canonical bytes.
    #[must_use]
    pub fn id(&self) -> ObjectId {
        ObjectId::from_content(self.to_canonical_bytes())
    }
}

impl Default for Tree {
    fn default() -> Self {
        Self::empty()
    }
}

/// Reason a tree entry name is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidTreeEntryName {
    /// Entry names cannot be empty.
    Empty,
    /// `.` is not a valid stored tree entry.
    CurrentDirectory,
    /// `..` is not a valid stored tree entry.
    ParentDirectory,
    /// Entry names are one path segment and cannot contain `/`.
    ContainsSlash,
    /// Entry names cannot contain NUL.
    ContainsNul,
}

impl fmt::Display for InvalidTreeEntryName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("entry name is empty"),
            Self::CurrentDirectory => formatter.write_str("entry name cannot be '.'"),
            Self::ParentDirectory => formatter.write_str("entry name cannot be '..'"),
            Self::ContainsSlash => formatter.write_str("entry name contains '/'"),
            Self::ContainsNul => formatter.write_str("entry name contains NUL"),
        }
    }
}

/// Errors returned while constructing or decoding trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    /// An entry name was not a valid single path segment.
    InvalidName {
        name: String,
        reason: InvalidTreeEntryName,
    },
    /// A tree contained two entries with the same exact name.
    DuplicateName { name: String },
    /// The serialized tree did not start with Era's tree magic bytes.
    InvalidMagic,
    /// The serialized tree ended before a complete field could be read.
    UnexpectedEof {
        offset: usize,
        needed: usize,
        len: usize,
    },
    /// The serialized tree contained an unknown entry-kind tag.
    InvalidEntryKind { index: usize, value: u8 },
    /// A serialized entry name was not UTF-8.
    NameNotUtf8 { index: usize },
    /// Serialized entries were not in canonical byte order.
    NonCanonicalOrder { previous: String, current: String },
    /// Bytes remained after the declared entries were decoded.
    TrailingBytes { offset: usize, len: usize },
}

impl fmt::Display for TreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name, reason } => {
                write!(formatter, "invalid tree entry name {name:?}: {reason}")
            }
            Self::DuplicateName { name } => write!(formatter, "duplicate tree entry name {name:?}"),
            Self::InvalidMagic => formatter.write_str("invalid tree object magic"),
            Self::UnexpectedEof {
                offset,
                needed,
                len,
            } => write!(
                formatter,
                "unexpected end of tree object at byte {offset}: needed {needed} bytes, len is {len}"
            ),
            Self::InvalidEntryKind { index, value } => write!(
                formatter,
                "invalid tree entry kind 0x{value:02x} at entry index {index}"
            ),
            Self::NameNotUtf8 { index } => {
                write!(formatter, "tree entry name at index {index} is not UTF-8")
            }
            Self::NonCanonicalOrder { previous, current } => write!(
                formatter,
                "tree entries are not in canonical order: {previous:?} appeared before {current:?}"
            ),
            Self::TrailingBytes { offset, len } => write!(
                formatter,
                "tree object has trailing bytes starting at {offset}; len is {len}"
            ),
        }
    }
}

impl Error for TreeError {}

struct TreeCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> TreeCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_magic(&mut self) -> Result<(), TreeError> {
        let magic = self.read_exact(TREE_MAGIC.len())?;
        if magic != TREE_MAGIC {
            return Err(TreeError::InvalidMagic);
        }
        Ok(())
    }

    fn read_byte(&mut self) -> Result<u8, TreeError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32, TreeError> {
        let bytes = self.read_exact(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], TreeError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(TreeError::UnexpectedEof {
                offset: self.offset,
                needed: len,
                len: self.bytes.len(),
            })?;
        if end > self.bytes.len() {
            return Err(TreeError::UnexpectedEof {
                offset: self.offset,
                needed: len,
                len: self.bytes.len(),
            });
        }

        let output = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(output)
    }

    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn validate_entry_name(name: &str) -> Result<(), TreeError> {
    let reason = if name.is_empty() {
        Some(InvalidTreeEntryName::Empty)
    } else if name == "." {
        Some(InvalidTreeEntryName::CurrentDirectory)
    } else if name == ".." {
        Some(InvalidTreeEntryName::ParentDirectory)
    } else if name.contains('/') {
        Some(InvalidTreeEntryName::ContainsSlash)
    } else if name.contains('\0') {
        Some(InvalidTreeEntryName::ContainsNul)
    } else {
        None
    };

    if let Some(reason) = reason {
        return Err(TreeError::InvalidName {
            name: name.to_owned(),
            reason,
        });
    }

    Ok(())
}

fn reject_duplicate_names(entries: &[TreeEntry]) -> Result<(), TreeError> {
    for pair in entries.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(TreeError::DuplicateName {
                name: pair[0].name.clone(),
            });
        }
    }
    Ok(())
}

fn write_u32(output: &mut Vec<u8>, value: usize) {
    let value = u32::try_from(value).expect("tree field length exceeds u32::MAX");
    output.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_sorts_entries_by_exact_name_bytes() {
        let id = ObjectId::from_content(b"blob");

        let tree = Tree::new([
            TreeEntry::blob("z.txt", id).unwrap(),
            TreeEntry::blob("a.txt", id).unwrap(),
            TreeEntry::blob("m.txt", id).unwrap(),
        ])
        .unwrap();

        assert_eq!(entry_names(&tree), vec!["a.txt", "m.txt", "z.txt"]);
    }

    #[test]
    fn tree_rejects_duplicate_names() {
        let id = ObjectId::from_content(b"blob");

        let error = Tree::new([
            TreeEntry::blob("same.txt", id).unwrap(),
            TreeEntry::tree("same.txt", id).unwrap(),
        ])
        .unwrap_err();

        assert_eq!(
            error,
            TreeError::DuplicateName {
                name: "same.txt".to_owned()
            }
        );
    }

    #[test]
    fn tree_entry_rejects_invalid_names() {
        let id = ObjectId::from_content(b"blob");
        let cases = [
            ("", InvalidTreeEntryName::Empty),
            (".", InvalidTreeEntryName::CurrentDirectory),
            ("..", InvalidTreeEntryName::ParentDirectory),
            ("nested/name", InvalidTreeEntryName::ContainsSlash),
            ("nul\0name", InvalidTreeEntryName::ContainsNul),
        ];

        for (name, reason) in cases {
            let error = TreeEntry::blob(name, id).unwrap_err();
            assert_eq!(
                error,
                TreeError::InvalidName {
                    name: name.to_owned(),
                    reason
                }
            );
        }
    }

    #[test]
    fn tree_supports_emoji_and_non_english_names() {
        let id = ObjectId::from_content(b"blob");
        let tree = Tree::new([
            TreeEntry::blob("✨.txt", id).unwrap(),
            TreeEntry::blob("日本語.md", id).unwrap(),
            TreeEntry::blob("مرحبا.txt", id).unwrap(),
            TreeEntry::blob("café.txt", id).unwrap(),
            TreeEntry::blob("данные.json", id).unwrap(),
        ])
        .unwrap();

        let roundtrip = Tree::from_canonical_bytes(&tree.to_canonical_bytes()).unwrap();

        assert_eq!(roundtrip, tree);
    }

    #[test]
    fn tree_does_not_normalize_unicode_names() {
        let id = ObjectId::from_content(b"blob");
        let composed = "é.txt";
        let decomposed = "e\u{301}.txt";

        let tree = Tree::new([
            TreeEntry::blob(composed, id).unwrap(),
            TreeEntry::blob(decomposed, id).unwrap(),
        ])
        .unwrap();

        assert_eq!(tree.entries().len(), 2);
        assert!(tree.entries().iter().any(|entry| entry.name() == composed));
        assert!(
            tree.entries()
                .iter()
                .any(|entry| entry.name() == decomposed)
        );
    }

    #[test]
    fn tree_hash_is_stable_independent_of_construction_order() {
        let first_id = ObjectId::from_content(b"first");
        let second_id = ObjectId::from_content(b"second");

        let left = Tree::new([
            TreeEntry::blob("b.txt", second_id).unwrap(),
            TreeEntry::blob("a.txt", first_id).unwrap(),
        ])
        .unwrap();
        let right = Tree::new([
            TreeEntry::blob("a.txt", first_id).unwrap(),
            TreeEntry::blob("b.txt", second_id).unwrap(),
        ])
        .unwrap();

        assert_eq!(left.to_canonical_bytes(), right.to_canonical_bytes());
        assert_eq!(left.id(), right.id());
    }

    #[test]
    fn entry_kind_affects_tree_hash() {
        let id = ObjectId::from_content(b"same pointed object id");
        let blob_tree = Tree::new([TreeEntry::blob("entry", id).unwrap()]).unwrap();
        let nested_tree = Tree::new([TreeEntry::tree("entry", id).unwrap()]).unwrap();

        assert_ne!(
            blob_tree.to_canonical_bytes(),
            nested_tree.to_canonical_bytes()
        );
        assert_ne!(blob_tree.id(), nested_tree.id());
    }

    #[test]
    fn empty_tree_has_canonical_bytes_and_id() {
        let tree = Tree::empty();

        assert_eq!(
            Tree::from_canonical_bytes(&tree.to_canonical_bytes()).unwrap(),
            tree
        );
        assert_eq!(tree.id(), ObjectId::from_content(tree.to_canonical_bytes()));
    }

    #[test]
    fn tree_round_trips_through_canonical_bytes() {
        let blob_id = ObjectId::from_content(b"blob");
        let child_id = ObjectId::from_content(b"child tree");
        let tree = Tree::new([
            TreeEntry::blob("file.txt", blob_id).unwrap(),
            TreeEntry::tree("src", child_id).unwrap(),
        ])
        .unwrap();

        let parsed = Tree::from_canonical_bytes(&tree.to_canonical_bytes()).unwrap();

        assert_eq!(parsed, tree);
    }

    #[test]
    fn tree_decode_rejects_invalid_magic() {
        let error = Tree::from_canonical_bytes(b"bad").unwrap_err();

        assert_eq!(
            error,
            TreeError::UnexpectedEof {
                offset: 0,
                needed: TREE_MAGIC.len(),
                len: 3
            }
        );

        let mut bytes = Tree::empty().to_canonical_bytes();
        bytes[0] = b'X';
        assert_eq!(
            Tree::from_canonical_bytes(&bytes).unwrap_err(),
            TreeError::InvalidMagic
        );
    }

    #[test]
    fn tree_decode_rejects_invalid_entry_kind() {
        let id = ObjectId::from_content(b"blob");
        let bytes = encode_tree_entries_unchecked(&[(b'x', "file.txt", id)]);

        let error = Tree::from_canonical_bytes(&bytes).unwrap_err();

        assert_eq!(
            error,
            TreeError::InvalidEntryKind {
                index: 0,
                value: b'x'
            }
        );
    }

    #[test]
    fn tree_decode_rejects_non_utf8_name() {
        let id = ObjectId::from_content(b"blob");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(TREE_MAGIC);
        write_u32(&mut bytes, 1);
        bytes.push(b'b');
        write_u32(&mut bytes, 1);
        bytes.push(0xff);
        bytes.extend_from_slice(id.as_bytes());

        let error = Tree::from_canonical_bytes(&bytes).unwrap_err();

        assert_eq!(error, TreeError::NameNotUtf8 { index: 0 });
    }

    #[test]
    fn tree_decode_rejects_non_canonical_order() {
        let id = ObjectId::from_content(b"blob");
        let bytes = encode_tree_entries_unchecked(&[(b'b', "z.txt", id), (b'b', "a.txt", id)]);

        let error = Tree::from_canonical_bytes(&bytes).unwrap_err();

        assert_eq!(
            error,
            TreeError::NonCanonicalOrder {
                previous: "z.txt".to_owned(),
                current: "a.txt".to_owned()
            }
        );
    }

    #[test]
    fn tree_decode_rejects_duplicate_names() {
        let id = ObjectId::from_content(b"blob");
        let bytes =
            encode_tree_entries_unchecked(&[(b'b', "same.txt", id), (b't', "same.txt", id)]);

        let error = Tree::from_canonical_bytes(&bytes).unwrap_err();

        assert_eq!(
            error,
            TreeError::DuplicateName {
                name: "same.txt".to_owned()
            }
        );
    }

    #[test]
    fn tree_decode_rejects_trailing_bytes() {
        let mut bytes = Tree::empty().to_canonical_bytes();
        let offset = bytes.len();
        bytes.push(0);

        let error = Tree::from_canonical_bytes(&bytes).unwrap_err();

        assert_eq!(
            error,
            TreeError::TrailingBytes {
                offset,
                len: offset + 1
            }
        );
    }

    fn entry_names(tree: &Tree) -> Vec<&str> {
        tree.entries().iter().map(TreeEntry::name).collect()
    }

    fn encode_tree_entries_unchecked(entries: &[(u8, &str, ObjectId)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(TREE_MAGIC);
        write_u32(&mut bytes, entries.len());
        for (kind, name, id) in entries {
            bytes.push(*kind);
            write_u32(&mut bytes, name.len());
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(id.as_bytes());
        }
        bytes
    }
}
