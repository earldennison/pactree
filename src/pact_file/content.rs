use std::collections::BTreeMap;

use crate::error::{PactError, PactResult};
use crate::store::ContentRecord;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContentInsert {
    pub(crate) id: String,
    pub(crate) full_hash: [u8; 32],
    pub(crate) payload_written: bool,
}

#[derive(Default)]
pub(crate) struct ContentSpine {
    records: BTreeMap<String, ContentRecord>,
    payloads: BTreeMap<[u8; 32], Vec<u8>>,
}

impl ContentSpine {
    pub(crate) fn insert(&mut self, record: ContentRecord) -> PactResult<ContentInsert> {
        let payload = captured_payload(&record)?;
        if record.bytes != payload.len() {
            return Err(PactError::invalid(format!(
                "content {} byte count does not match captured payload",
                record.id
            )));
        }
        let public_hash = crate::store::stable_content_hash(&record.kind, &payload);
        if record.hash != public_hash {
            return Err(PactError::invalid(format!(
                "content {} hash does not match {public_hash}",
                record.id
            )));
        }
        if let Some(existing) = self.records.get(&record.id) {
            if existing != &record {
                return Err(PactError::invalid(format!(
                    "content id {} already refers to different content",
                    record.id
                )));
            }
            let full_hash = *blake3::hash(&payload).as_bytes();
            return Ok(ContentInsert {
                id: record.id,
                full_hash,
                payload_written: false,
            });
        }

        let full_hash = *blake3::hash(&payload).as_bytes();
        let payload_written = self.payloads.insert(full_hash, payload).is_none();
        let id = record.id.clone();
        self.records.insert(id.clone(), record);
        Ok(ContentInsert {
            id,
            full_hash,
            payload_written,
        })
    }

    pub(crate) fn record(&self, id: &str) -> Option<&ContentRecord> {
        self.records.get(id)
    }

    pub(crate) fn payload_count(&self) -> usize {
        self.payloads.len()
    }
}

fn captured_payload(record: &ContentRecord) -> PactResult<Vec<u8>> {
    if let Some(bytes) = &record.storage.inline_bytes {
        return Ok(bytes.clone());
    }
    if let Some(text) = &record.storage.inline {
        return Ok(text.as_bytes().to_vec());
    }
    Err(PactError::unsupported(format!(
        "candidate content {} has no captured payload bytes",
        record.id
    )))
}
