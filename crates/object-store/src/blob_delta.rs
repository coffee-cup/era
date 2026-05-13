use era_core::{OBJECT_ID_BYTES, ObjectId};

pub(crate) const MAX_DELTA_CHAIN_DEPTH: u32 = 16;
pub(crate) const MIN_DELTA_TARGET_BYTES: usize = 16 * 1024;
const MIN_DELTA_SAVINGS_BYTES: usize = 1024;
const MAGIC: &[u8] = b"era-blob-delta-v1\n";
const U32_BYTES: usize = 4;
const U64_BYTES: usize = 8;
const HEADER_LEN: usize = MAGIC.len() + U32_BYTES + OBJECT_ID_BYTES + U64_BYTES * 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlobDelta {
    pub(crate) base_id: ObjectId,
    pub(crate) depth: u32,
    base_len: u64,
    target_len: u64,
    prefix_len: u64,
    suffix_len: u64,
    insert: Vec<u8>,
}

impl BlobDelta {
    pub(crate) fn create(
        base_id: ObjectId,
        base: &[u8],
        base_depth: u32,
        target: &[u8],
    ) -> Option<Self> {
        if target.len() < MIN_DELTA_TARGET_BYTES || base_depth >= MAX_DELTA_CHAIN_DEPTH {
            return None;
        }

        let prefix_len = common_prefix_len(base, target);
        let suffix_len = common_suffix_len(base, target, prefix_len);
        let insert_start = prefix_len;
        let insert_end = target.len() - suffix_len;
        let insert = target[insert_start..insert_end].to_vec();
        let delta = Self {
            base_id,
            depth: base_depth + 1,
            base_len: base.len() as u64,
            target_len: target.len() as u64,
            prefix_len: prefix_len as u64,
            suffix_len: suffix_len as u64,
            insert,
        };

        if delta.encoded_len().saturating_add(MIN_DELTA_SAVINGS_BYTES) >= target.len() {
            return None;
        }

        Some(delta)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Option<Self>, String> {
        if !bytes.starts_with(MAGIC) {
            return Ok(None);
        }
        if bytes.len() < HEADER_LEN {
            return Err("truncated blob delta header".to_owned());
        }

        let mut cursor = MAGIC.len();
        let depth = read_u32(bytes, &mut cursor)?;
        let base_id = read_object_id(bytes, &mut cursor)?;
        let base_len = read_u64(bytes, &mut cursor)?;
        let target_len = read_u64(bytes, &mut cursor)?;
        let prefix_len = read_u64(bytes, &mut cursor)?;
        let suffix_len = read_u64(bytes, &mut cursor)?;
        let insert_len = read_u64(bytes, &mut cursor)?;
        let insert_len = usize::try_from(insert_len)
            .map_err(|_| "blob delta insert length overflows usize".to_owned())?;
        if bytes.len() != cursor + insert_len {
            return Err("blob delta insert length does not match payload".to_owned());
        }

        Ok(Some(Self {
            base_id,
            depth,
            base_len,
            target_len,
            prefix_len,
            suffix_len,
            insert: bytes[cursor..].to_vec(),
        }))
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.encoded_len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&self.depth.to_be_bytes());
        bytes.extend_from_slice(self.base_id.as_bytes());
        bytes.extend_from_slice(&self.base_len.to_be_bytes());
        bytes.extend_from_slice(&self.target_len.to_be_bytes());
        bytes.extend_from_slice(&self.prefix_len.to_be_bytes());
        bytes.extend_from_slice(&self.suffix_len.to_be_bytes());
        bytes.extend_from_slice(&(self.insert.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&self.insert);
        bytes
    }

    pub(crate) fn reconstruct(&self, base: &[u8]) -> Result<Vec<u8>, String> {
        let base_len = usize::try_from(self.base_len)
            .map_err(|_| "blob delta base length overflows usize".to_owned())?;
        let target_len = usize::try_from(self.target_len)
            .map_err(|_| "blob delta target length overflows usize".to_owned())?;
        let prefix_len = usize::try_from(self.prefix_len)
            .map_err(|_| "blob delta prefix length overflows usize".to_owned())?;
        let suffix_len = usize::try_from(self.suffix_len)
            .map_err(|_| "blob delta suffix length overflows usize".to_owned())?;

        if base.len() != base_len {
            return Err("blob delta base length does not match stored base".to_owned());
        }
        if prefix_len > base.len() || suffix_len > base.len().saturating_sub(prefix_len) {
            return Err("blob delta base ranges are out of bounds".to_owned());
        }
        if prefix_len > target_len || suffix_len > target_len.saturating_sub(prefix_len) {
            return Err("blob delta target ranges are out of bounds".to_owned());
        }
        if self.insert.len() != target_len - prefix_len - suffix_len {
            return Err("blob delta insert length does not match target length".to_owned());
        }

        let mut target = Vec::with_capacity(target_len);
        target.extend_from_slice(&base[..prefix_len]);
        target.extend_from_slice(&self.insert);
        if suffix_len > 0 {
            target.extend_from_slice(&base[base.len() - suffix_len..]);
        }
        Ok(target)
    }

    fn encoded_len(&self) -> usize {
        HEADER_LEN + self.insert.len()
    }
}

fn common_prefix_len(base: &[u8], target: &[u8]) -> usize {
    base.iter()
        .zip(target)
        .take_while(|(base_byte, target_byte)| base_byte == target_byte)
        .count()
}

fn common_suffix_len(base: &[u8], target: &[u8], prefix_len: usize) -> usize {
    let max_suffix = base.len().min(target.len()).saturating_sub(prefix_len);
    base.iter()
        .rev()
        .zip(target.iter().rev())
        .take(max_suffix)
        .take_while(|(base_byte, target_byte)| base_byte == target_byte)
        .count()
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let end = *cursor + U32_BYTES;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| "truncated blob delta u32".to_owned())?
        .try_into()
        .expect("slice length checked");
    *cursor = end;
    Ok(u32::from_be_bytes(value))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let end = *cursor + U64_BYTES;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| "truncated blob delta u64".to_owned())?
        .try_into()
        .expect("slice length checked");
    *cursor = end;
    Ok(u64::from_be_bytes(value))
}

fn read_object_id(bytes: &[u8], cursor: &mut usize) -> Result<ObjectId, String> {
    let end = *cursor + OBJECT_ID_BYTES;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| "truncated blob delta object id".to_owned())?
        .try_into()
        .expect("slice length checked");
    *cursor = end;
    Ok(ObjectId::from_bytes(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_round_trips_single_byte_edit() {
        let base = vec![b'a'; 64 * 1024];
        let mut target = base.clone();
        target[30_000] = b'b';
        let base_id = ObjectId::from_content(&base);

        let delta = BlobDelta::create(base_id, &base, 0, &target).unwrap();
        let encoded = delta.encode();
        let decoded = BlobDelta::decode(&encoded).unwrap().unwrap();

        assert_eq!(decoded.base_id, base_id);
        assert!(encoded.len() < target.len() / 8);
        assert_eq!(decoded.reconstruct(&base).unwrap(), target);
    }

    #[test]
    fn small_targets_do_not_delta() {
        let base = b"hello";
        let target = b"hallo";
        let base_id = ObjectId::from_content(base);

        assert!(BlobDelta::create(base_id, base, 0, target).is_none());
    }
}
