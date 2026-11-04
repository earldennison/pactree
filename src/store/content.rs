use super::model::{ContentRecord, ContentRecordSummary, ContentStorageRef, ContentStore};
use super::util::stable_content_hash;

impl ContentStore {
    pub fn rebuild_hash_index(&mut self) {
        self.records_by_hash = self
            .records
            .values()
            .map(|record| (record.hash.clone(), record.id.clone()))
            .collect();
    }

    pub fn put_inline(&mut self, kind: &str, content: &str, preview_chars: usize) -> ContentRecord {
        if self.records_by_hash.len() != self.records.len() {
            self.rebuild_hash_index();
        }
        let hash = stable_content_hash(kind, content.as_bytes());
        if let Some(existing) = self.record_for_hash(&hash) {
            return existing;
        }

        let id = format!("cnt_{}", &hash[..8]);
        let record = ContentRecord {
            id: id.clone(),
            kind: kind.to_string(),
            hash: hash.clone(),
            bytes: content.len(),
            chars: Some(content.chars().count()),
            preview: content.chars().take(preview_chars).collect(),
            storage: ContentStorageRef {
                kind: "inline".to_string(),
                inline: Some(content.to_string()),
                inline_bytes: None,
                path: None,
                external_ref: None,
            },
        };
        self.records_by_hash.insert(hash, id.clone());
        self.records.insert(id, record.clone());
        record
    }

    pub fn put_bytes(
        &mut self,
        kind: &str,
        bytes: &[u8],
        preview: &str,
        metadata_hash_key: &str,
    ) -> ContentRecord {
        if self.records_by_hash.len() != self.records.len() {
            self.rebuild_hash_index();
        }
        let hash = stable_content_hash(&format!("{kind}:{metadata_hash_key}"), bytes);
        if let Some(existing) = self.record_for_hash(&hash) {
            return existing;
        }

        let id = format!("cnt_{}", &hash[..8]);
        let record = ContentRecord {
            id: id.clone(),
            kind: kind.to_string(),
            hash: hash.clone(),
            bytes: bytes.len(),
            chars: None,
            preview: preview.to_string(),
            storage: ContentStorageRef {
                kind: "inline_bytes".to_string(),
                inline: None,
                inline_bytes: Some(bytes.to_vec()),
                path: None,
                external_ref: None,
            },
        };
        self.records_by_hash.insert(hash, id.clone());
        self.records.insert(id, record.clone());
        record
    }

    fn record_for_hash(&mut self, hash: &str) -> Option<ContentRecord> {
        let existing_id = self.records_by_hash.get(hash)?.clone();
        if let Some(record) = self.records.get(&existing_id) {
            return Some(record.clone());
        }

        self.rebuild_hash_index();
        self.records_by_hash
            .get(hash)
            .and_then(|id| self.records.get(id))
            .cloned()
    }
}

impl ContentRecord {
    pub fn summary(&self) -> ContentRecordSummary {
        ContentRecordSummary {
            id: self.id.clone(),
            kind: self.kind.clone(),
            hash: self.hash.clone(),
            bytes: self.bytes,
            chars: self.chars,
            preview: self.preview.clone(),
        }
    }
}
