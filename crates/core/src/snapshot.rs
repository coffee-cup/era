use crate::ObjectId;
use std::{collections::BTreeMap, error::Error, fmt};

const SNAPSHOT_MAGIC: &[u8] = b"ERA_SNAPSHOT_V1\0";

/// Structured provenance attached to a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotProvenance {
    source: String,
    attributes: BTreeMap<String, String>,
}

impl SnapshotProvenance {
    /// Creates provenance with a source name and no extra attributes.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            attributes: BTreeMap::new(),
        }
    }

    /// Provenance for repository initialization snapshots.
    pub fn initial() -> Self {
        Self::new("repository-init")
    }

    /// Provenance for explicitly requested manual snapshots.
    pub fn manual() -> Self {
        Self::new("manual-snapshot")
    }

    /// Returns the source that produced the snapshot.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns sorted provenance attributes.
    pub fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }

    /// Adds or replaces a provenance attribute.
    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }
}

/// A complete captured tree state with history and metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    root_tree_id: ObjectId,
    parents: Vec<ObjectId>,
    timestamp_millis: u64,
    author: Option<String>,
    message: Option<String>,
    provenance: SnapshotProvenance,
}

impl Snapshot {
    /// Creates a snapshot value.
    pub fn new(
        root_tree_id: ObjectId,
        parents: impl Into<Vec<ObjectId>>,
        timestamp_millis: u64,
        author: Option<String>,
        message: Option<String>,
        provenance: SnapshotProvenance,
    ) -> Self {
        Self {
            root_tree_id,
            parents: parents.into(),
            timestamp_millis,
            author,
            message,
            provenance,
        }
    }

    /// Returns the root tree captured by this snapshot.
    pub fn root_tree_id(&self) -> ObjectId {
        self.root_tree_id
    }

    /// Returns the parent snapshot IDs in stored order.
    pub fn parents(&self) -> &[ObjectId] {
        &self.parents
    }

    /// Returns the snapshot timestamp as milliseconds since the Unix epoch.
    pub fn timestamp_millis(&self) -> u64 {
        self.timestamp_millis
    }

    /// Returns the optional snapshot author.
    pub fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }

    /// Returns the optional human-facing snapshot message.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Returns structured provenance for the snapshot.
    pub fn provenance(&self) -> &SnapshotProvenance {
        &self.provenance
    }

    /// Returns canonical serialized snapshot bytes.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(SNAPSHOT_MAGIC);
        output.extend_from_slice(self.root_tree_id.as_bytes());
        write_u32(&mut output, self.parents.len());
        for parent in &self.parents {
            output.extend_from_slice(parent.as_bytes());
        }
        write_u64(&mut output, self.timestamp_millis);
        write_optional_string(&mut output, self.author.as_deref());
        write_optional_string(&mut output, self.message.as_deref());
        write_string(&mut output, self.provenance.source());
        write_u32(&mut output, self.provenance.attributes().len());
        for (key, value) in self.provenance.attributes() {
            write_string(&mut output, key);
            write_string(&mut output, value);
        }
        output
    }

    /// Parses canonical serialized snapshot bytes.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, SnapshotError> {
        let mut cursor = SnapshotCursor::new(bytes);
        cursor.read_magic()?;

        let root_tree_id = cursor.read_object_id()?;
        let parent_count = cursor.read_u32()? as usize;
        let mut parents = Vec::new();
        for _ in 0..parent_count {
            parents.push(cursor.read_object_id()?);
        }

        let timestamp_millis = cursor.read_u64()?;
        let author = cursor.read_optional_string("author")?;
        let message = cursor.read_optional_string("message")?;
        let source = cursor.read_string("provenance source")?;
        let attribute_count = cursor.read_u32()? as usize;
        let mut attributes = BTreeMap::new();
        let mut previous_key: Option<String> = None;

        for _ in 0..attribute_count {
            let key = cursor.read_string("provenance attribute key")?;
            if let Some(previous) = &previous_key {
                match previous.as_bytes().cmp(key.as_bytes()) {
                    std::cmp::Ordering::Less => {}
                    std::cmp::Ordering::Equal => {
                        return Err(SnapshotError::DuplicateProvenanceAttribute { key });
                    }
                    std::cmp::Ordering::Greater => {
                        return Err(SnapshotError::NonCanonicalProvenanceAttributeOrder {
                            previous: previous.clone(),
                            current: key,
                        });
                    }
                }
            }
            let value = cursor.read_string("provenance attribute value")?;
            previous_key = Some(key.clone());
            attributes.insert(key, value);
        }

        if !cursor.is_finished() {
            return Err(SnapshotError::TrailingBytes {
                offset: cursor.offset,
                len: bytes.len(),
            });
        }

        Ok(Self {
            root_tree_id,
            parents,
            timestamp_millis,
            author,
            message,
            provenance: SnapshotProvenance { source, attributes },
        })
    }

    /// Returns the content-addressed ID of this snapshot's canonical bytes.
    pub fn id(&self) -> ObjectId {
        ObjectId::from_content(self.to_canonical_bytes())
    }
}

/// Errors returned while decoding snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// The serialized snapshot did not start with Era's snapshot magic bytes.
    InvalidMagic,
    /// The serialized snapshot ended before a complete field could be read.
    UnexpectedEof {
        offset: usize,
        needed: usize,
        len: usize,
    },
    /// An optional string field used an unknown tag.
    InvalidOptionalStringTag { field: &'static str, value: u8 },
    /// A serialized string field was not UTF-8.
    StringNotUtf8 { field: &'static str },
    /// Provenance attributes contained the same key twice.
    DuplicateProvenanceAttribute { key: String },
    /// Provenance attributes were not in canonical byte order.
    NonCanonicalProvenanceAttributeOrder { previous: String, current: String },
    /// Bytes remained after the declared snapshot fields were decoded.
    TrailingBytes { offset: usize, len: usize },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => formatter.write_str("invalid snapshot object magic"),
            Self::UnexpectedEof {
                offset,
                needed,
                len,
            } => write!(
                formatter,
                "unexpected end of snapshot object at offset {offset}: needed {needed} bytes, object length is {len}"
            ),
            Self::InvalidOptionalStringTag { field, value } => write!(
                formatter,
                "invalid optional string tag {value} for snapshot field {field}"
            ),
            Self::StringNotUtf8 { field } => {
                write!(formatter, "snapshot field {field} is not UTF-8")
            }
            Self::DuplicateProvenanceAttribute { key } => {
                write!(formatter, "duplicate provenance attribute key {key:?}")
            }
            Self::NonCanonicalProvenanceAttributeOrder { previous, current } => write!(
                formatter,
                "provenance attributes are not in canonical order: {previous:?} before {current:?}"
            ),
            Self::TrailingBytes { offset, len } => write!(
                formatter,
                "snapshot object has trailing bytes starting at offset {offset}; object length is {len}"
            ),
        }
    }
}

impl Error for SnapshotError {}

struct SnapshotCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> SnapshotCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_magic(&mut self) -> Result<(), SnapshotError> {
        if self.bytes.len() < SNAPSHOT_MAGIC.len() {
            return Err(SnapshotError::InvalidMagic);
        }

        let magic = self.read_exact(SNAPSHOT_MAGIC.len())?;
        if magic == SNAPSHOT_MAGIC {
            Ok(())
        } else {
            Err(SnapshotError::InvalidMagic)
        }
    }

    fn read_object_id(&mut self) -> Result<ObjectId, SnapshotError> {
        let bytes = self.read_exact(crate::OBJECT_ID_BYTES)?;
        let mut id = [0_u8; crate::OBJECT_ID_BYTES];
        id.copy_from_slice(bytes);
        Ok(ObjectId::from_bytes(id))
    }

    fn read_u32(&mut self) -> Result<u32, SnapshotError> {
        let bytes = self.read_exact(4)?;
        let mut value = [0_u8; 4];
        value.copy_from_slice(bytes);
        Ok(u32::from_be_bytes(value))
    }

    fn read_u64(&mut self) -> Result<u64, SnapshotError> {
        let bytes = self.read_exact(8)?;
        let mut value = [0_u8; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(value))
    }

    fn read_byte(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_string(&mut self, field: &'static str) -> Result<String, SnapshotError> {
        let length = self.read_u32()? as usize;
        let bytes = self.read_exact(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| SnapshotError::StringNotUtf8 { field })
    }

    fn read_optional_string(
        &mut self,
        field: &'static str,
    ) -> Result<Option<String>, SnapshotError> {
        match self.read_byte()? {
            0 => Ok(None),
            1 => self.read_string(field).map(Some),
            value => Err(SnapshotError::InvalidOptionalStringTag { field, value }),
        }
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], SnapshotError> {
        let end = self.offset.saturating_add(length);
        if end > self.bytes.len() {
            return Err(SnapshotError::UnexpectedEof {
                offset: self.offset,
                needed: length,
                len: self.bytes.len(),
            });
        }

        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn write_u32(output: &mut Vec<u8>, value: usize) {
    output.extend_from_slice(&(value as u32).to_be_bytes());
}

fn write_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn write_optional_string(output: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            output.push(1);
            write_string(output, value);
        }
        None => output.push(0),
    }
}

fn write_string(output: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    write_u32(output, bytes.len());
    output.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tree;

    fn sample_snapshot() -> Snapshot {
        Snapshot::new(
            Tree::empty().id(),
            vec![
                ObjectId::from_content(b"parent-1"),
                ObjectId::from_content(b"parent-2"),
            ],
            1_700_000_000_123,
            Some("Ada Lovelace".to_owned()),
            Some("capture useful state".to_owned()),
            SnapshotProvenance::manual()
                .with_attribute("agent", "pi")
                .with_attribute("model", "test-model"),
        )
    }

    #[test]
    fn snapshot_round_trips_through_canonical_bytes() {
        let snapshot = sample_snapshot();

        assert_eq!(
            Snapshot::from_canonical_bytes(&snapshot.to_canonical_bytes()).unwrap(),
            snapshot
        );
    }

    #[test]
    fn snapshot_id_is_hash_of_canonical_bytes() {
        let snapshot = sample_snapshot();

        assert_eq!(
            snapshot.id(),
            ObjectId::from_content(snapshot.to_canonical_bytes())
        );
    }

    #[test]
    fn snapshot_parent_order_is_preserved() {
        let snapshot = sample_snapshot();
        let decoded = Snapshot::from_canonical_bytes(&snapshot.to_canonical_bytes()).unwrap();

        assert_eq!(decoded.parents(), snapshot.parents());
    }

    #[test]
    fn provenance_attributes_are_serialized_in_canonical_order() {
        let first = Snapshot::new(
            Tree::empty().id(),
            Vec::new(),
            42,
            None,
            None,
            SnapshotProvenance::manual()
                .with_attribute("z", "last")
                .with_attribute("a", "first"),
        );
        let second = Snapshot::new(
            Tree::empty().id(),
            Vec::new(),
            42,
            None,
            None,
            SnapshotProvenance::manual()
                .with_attribute("a", "first")
                .with_attribute("z", "last"),
        );

        assert_eq!(first.to_canonical_bytes(), second.to_canonical_bytes());
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn snapshot_decode_rejects_invalid_magic() {
        let mut bytes = sample_snapshot().to_canonical_bytes();
        bytes[0] = b'X';

        assert_eq!(
            Snapshot::from_canonical_bytes(&bytes).unwrap_err(),
            SnapshotError::InvalidMagic
        );
    }

    #[test]
    fn snapshot_decode_rejects_trailing_bytes() {
        let mut bytes = sample_snapshot().to_canonical_bytes();
        let offset = bytes.len();
        bytes.push(0);

        assert_eq!(
            Snapshot::from_canonical_bytes(&bytes).unwrap_err(),
            SnapshotError::TrailingBytes {
                offset,
                len: bytes.len()
            }
        );
    }

    #[test]
    fn snapshot_decode_rejects_non_canonical_provenance_order() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(SNAPSHOT_MAGIC);
        bytes.extend_from_slice(Tree::empty().id().as_bytes());
        write_u32(&mut bytes, 0);
        write_u64(&mut bytes, 1);
        write_optional_string(&mut bytes, None);
        write_optional_string(&mut bytes, None);
        write_string(&mut bytes, "manual-snapshot");
        write_u32(&mut bytes, 2);
        write_string(&mut bytes, "z");
        write_string(&mut bytes, "last");
        write_string(&mut bytes, "a");
        write_string(&mut bytes, "first");

        assert_eq!(
            Snapshot::from_canonical_bytes(&bytes).unwrap_err(),
            SnapshotError::NonCanonicalProvenanceAttributeOrder {
                previous: "z".to_owned(),
                current: "a".to_owned()
            }
        );
    }

    #[test]
    fn snapshot_decode_rejects_duplicate_provenance_keys() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(SNAPSHOT_MAGIC);
        bytes.extend_from_slice(Tree::empty().id().as_bytes());
        write_u32(&mut bytes, 0);
        write_u64(&mut bytes, 1);
        write_optional_string(&mut bytes, None);
        write_optional_string(&mut bytes, None);
        write_string(&mut bytes, "manual-snapshot");
        write_u32(&mut bytes, 2);
        write_string(&mut bytes, "a");
        write_string(&mut bytes, "first");
        write_string(&mut bytes, "a");
        write_string(&mut bytes, "second");

        assert_eq!(
            Snapshot::from_canonical_bytes(&bytes).unwrap_err(),
            SnapshotError::DuplicateProvenanceAttribute {
                key: "a".to_owned()
            }
        );
    }

    #[test]
    fn snapshot_golden_empty_root_v1_decodes_and_keeps_id() {
        let bytes = include_bytes!("../tests/fixtures/snapshots/empty-root-v1.bin");
        let id = include_str!("../tests/fixtures/snapshots/empty-root-v1.id").trim();

        let snapshot = Snapshot::from_canonical_bytes(bytes).unwrap();

        assert_eq!(snapshot.to_canonical_bytes(), bytes);
        assert_eq!(snapshot.id().to_string(), id);
        assert_eq!(snapshot.root_tree_id(), Tree::empty().id());
        assert!(snapshot.parents().is_empty());
        assert_eq!(snapshot.provenance().source(), "repository-init");
    }

    #[test]
    fn snapshot_golden_one_parent_v1_decodes_and_keeps_id() {
        let bytes = include_bytes!("../tests/fixtures/snapshots/one-parent-v1.bin");
        let id = include_str!("../tests/fixtures/snapshots/one-parent-v1.id").trim();

        let snapshot = Snapshot::from_canonical_bytes(bytes).unwrap();

        assert_eq!(snapshot.to_canonical_bytes(), bytes);
        assert_eq!(snapshot.id().to_string(), id);
        assert_eq!(snapshot.parents().len(), 1);
        assert_eq!(snapshot.message(), Some("manual checkpoint"));
        assert_eq!(snapshot.provenance().source(), "manual-snapshot");
    }

    #[test]
    fn snapshot_golden_with_metadata_v1_decodes_and_keeps_id() {
        let bytes = include_bytes!("../tests/fixtures/snapshots/with-metadata-v1.bin");
        let id = include_str!("../tests/fixtures/snapshots/with-metadata-v1.id").trim();

        let snapshot = Snapshot::from_canonical_bytes(bytes).unwrap();

        assert_eq!(snapshot.to_canonical_bytes(), bytes);
        assert_eq!(snapshot.id().to_string(), id);
        assert_eq!(snapshot.author(), Some("agent@example"));
        assert_eq!(snapshot.message(), Some("capture ✅"));
        assert_eq!(
            snapshot
                .provenance()
                .attributes()
                .get("model")
                .map(String::as_str),
            Some("test-model")
        );
    }
}
