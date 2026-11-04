use super::layout::{LIVE_CONTENT_CHUNK_BYTES, PACT_RECORD_ZSTD_DICT_V1, PAGE_DICT_MAGIC};
use super::util::{base64_decode, base64_encode, sanitize_id, stable_hash};
use super::{ContentChunkPayload, ContentPayload, ScannedRecord};
use crate::error::{PactError, PactResult};
use crate::store::ContentRecord;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "compression-zstd")]
use std::io::{Cursor, Read, Write};

type ContentChunkMap = BTreeMap<String, Vec<(usize, Vec<u8>)>>;

#[derive(Default)]
pub(super) struct ContentCollector {
    pub(super) by_hash: BTreeMap<String, ContentPayload>,
    pub(super) chunks: Vec<ContentChunkPayload>,
    pub(super) chunk_index: BTreeMap<String, (String, usize)>,
    pub(super) previews_by_id: BTreeMap<String, String>,
    pub(super) compress_chunks: bool,
    pub(super) include_inline_previews: bool,
}

struct ChunkedPayloadInput<'a> {
    content_id: &'a str,
    kind: &'a str,
    hash: &'a str,
    bytes: usize,
    chars: Option<usize>,
    preview_inline: &'a str,
    inline: &'a str,
}

impl ContentCollector {
    pub(super) fn with_compression() -> Self {
        Self {
            by_hash: BTreeMap::new(),
            chunks: Vec::new(),
            chunk_index: BTreeMap::new(),
            previews_by_id: BTreeMap::new(),
            #[cfg(feature = "compression-zstd")]
            compress_chunks: true,
            #[cfg(not(feature = "compression-zstd"))]
            compress_chunks: false,
            include_inline_previews: false,
        }
    }

    pub(super) fn seed_chunk_index_from_records(&mut self, records: &[ScannedRecord]) {
        for record in records
            .iter()
            .filter(|record| record.kind == "content_chunk")
        {
            let Some(hash) = record.payload.get("hash").and_then(Value::as_str) else {
                continue;
            };
            let Some(content_id) = record.payload.get("content_id").and_then(Value::as_str) else {
                continue;
            };
            let ordinal = record
                .payload
                .get("ordinal")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize;
            self.chunk_index
                .entry(hash.to_string())
                .or_insert_with(|| (content_id.to_string(), ordinal));
        }
    }

    pub(super) fn put_text(&mut self, kind: &str, id_hint: &str, text: &str) -> PactResult<String> {
        let hash = stable_hash(kind, text.as_bytes());
        if let Some(existing) = self.by_hash.get(&hash) {
            return Ok(existing.id.clone());
        }

        let id = format!("cnt_{}_{}", sanitize_id(id_hint), &hash[..8]);
        let chars = text.chars().count();
        let bytes = text.len();
        let preview: String = text.chars().take(160).collect();
        let payload = if bytes >= LIVE_CONTENT_CHUNK_BYTES {
            self.chunked_payload(ChunkedPayloadInput {
                content_id: &id,
                kind,
                hash: &hash,
                bytes,
                chars: Some(chars),
                preview_inline: &preview,
                inline: text,
            })?
        } else {
            let body = encode_content_body(text.as_bytes(), self.compress_chunks)?;
            ContentPayload {
                id: id.clone(),
                kind: kind.to_string(),
                hash: hash.clone(),
                bytes,
                chars: Some(chars),
                preview_inline: self.inline_preview_cache(&preview),
                chunk_count: if text.is_empty() { 0 } else { 1 },
                codec: body.codec,
                encoding: body.encoding,
                text: Some(body.data),
            }
        };
        self.remember_preview(&id, &preview);
        self.by_hash.insert(hash, payload);
        Ok(id)
    }

    pub(super) fn put_content_record(&mut self, record: &ContentRecord) -> PactResult<String> {
        if let Some(bytes) = &record.storage.inline_bytes {
            if !self.by_hash.values().any(|payload| payload.id == record.id) {
                let body = encode_content_body(bytes, self.compress_chunks)?;
                let payload = ContentPayload {
                    id: record.id.clone(),
                    kind: record.kind.clone(),
                    hash: record.hash.clone(),
                    bytes: record.bytes,
                    chars: None,
                    preview_inline: self.inline_preview_cache(&record.preview),
                    chunk_count: if bytes.is_empty() { 0 } else { 1 },
                    codec: body.codec,
                    encoding: body.encoding,
                    text: Some(body.data),
                };
                self.remember_preview(&record.id, &record.preview);
                self.by_hash.insert(record.hash.clone(), payload);
            }
            Ok(record.id.clone())
        } else if let Some(inline) = &record.storage.inline {
            if !self.by_hash.values().any(|payload| payload.id == record.id) {
                let payload = if inline.len() >= LIVE_CONTENT_CHUNK_BYTES {
                    self.chunked_payload(ChunkedPayloadInput {
                        content_id: &record.id,
                        kind: &record.kind,
                        hash: &record.hash,
                        bytes: record.bytes,
                        chars: record.chars,
                        preview_inline: &record.preview,
                        inline,
                    })?
                } else {
                    let body = encode_content_body(inline.as_bytes(), self.compress_chunks)?;
                    ContentPayload {
                        id: record.id.clone(),
                        kind: record.kind.clone(),
                        hash: record.hash.clone(),
                        bytes: record.bytes,
                        chars: record.chars,
                        preview_inline: self.inline_preview_cache(&record.preview),
                        chunk_count: if inline.is_empty() { 0 } else { 1 },
                        codec: body.codec,
                        encoding: body.encoding,
                        text: Some(body.data),
                    }
                };
                self.remember_preview(&record.id, &record.preview);
                self.by_hash.insert(record.hash.clone(), payload);
            }
            Ok(record.id.clone())
        } else {
            let hash = record.hash.clone();
            if !self.by_hash.contains_key(&hash) {
                self.by_hash.insert(
                    hash.clone(),
                    ContentPayload {
                        id: record.id.clone(),
                        kind: record.kind.clone(),
                        hash,
                        bytes: record.bytes,
                        chars: record.chars,
                        preview_inline: self.inline_preview_cache(&record.preview),
                        chunk_count: 0,
                        codec: "external".to_string(),
                        encoding: None,
                        text: None,
                    },
                );
                self.remember_preview(&record.id, &record.preview);
            }
            Ok(record.id.clone())
        }
    }

    fn chunked_payload(&mut self, input: ChunkedPayloadInput<'_>) -> PactResult<ContentPayload> {
        let content_id = input.content_id;
        let chunks = split_text_chunks(input.inline, LIVE_CONTENT_CHUNK_BYTES);
        for (ordinal, chunk_text) in chunks.iter().enumerate() {
            let chunk_bytes = chunk_text.as_bytes();
            let hash = stable_hash("content.chunk", chunk_bytes);
            if let Some((ref_content_id, ref_ordinal)) = self.chunk_index.get(&hash).cloned() {
                self.chunks.push(ContentChunkPayload {
                    content_id: content_id.to_string(),
                    ordinal,
                    hash,
                    bytes: chunk_bytes.len(),
                    codec: "ref".to_string(),
                    encoding: None,
                    text: String::new(),
                    ref_content_id: Some(ref_content_id),
                    ref_ordinal: Some(ref_ordinal),
                });
                continue;
            }

            let body = encode_content_body(chunk_bytes, false)?;
            self.chunks.push(ContentChunkPayload {
                content_id: content_id.to_string(),
                ordinal,
                hash: hash.clone(),
                bytes: chunk_bytes.len(),
                codec: body.codec,
                encoding: body.encoding,
                text: body.data,
                ref_content_id: None,
                ref_ordinal: None,
            });
            self.chunk_index
                .insert(hash, (content_id.to_string(), ordinal));
        }
        Ok(ContentPayload {
            id: content_id.to_string(),
            kind: input.kind.to_string(),
            hash: input.hash.to_string(),
            bytes: input.bytes,
            chars: input.chars,
            preview_inline: self.inline_preview_cache(input.preview_inline),
            chunk_count: chunks.len(),
            codec: "chunked".to_string(),
            encoding: None,
            text: None,
        })
    }

    fn inline_preview_cache(&self, preview: &str) -> Option<String> {
        self.include_inline_previews
            .then(|| preview.to_string())
            .filter(|preview| !preview.is_empty())
    }

    fn remember_preview(&mut self, content_id: &str, preview: &str) {
        if preview.is_empty() {
            return;
        }
        self.previews_by_id
            .entry(content_id.to_string())
            .or_insert_with(|| preview.to_string());
    }
}

pub(super) struct EncodedContentBody {
    pub(super) codec: String,
    pub(super) encoding: Option<String>,
    pub(super) data: String,
}

pub(super) fn split_text_chunks(text: &str, max_bytes: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let max_bytes = max_bytes.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();

    for segment in text.split_inclusive('\n') {
        if segment.len() > max_bytes {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.extend(split_long_text_segment(segment, max_bytes));
            continue;
        }
        if !current.is_empty() && current.len() + segment.len() > max_bytes {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(segment);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    if chunks.is_empty() {
        split_long_text_segment(text, max_bytes)
    } else {
        chunks
    }
}

fn split_long_text_segment(text: &str, max_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut last_boundary = 0;
    for (index, _) in text.char_indices() {
        if index == start {
            last_boundary = index;
            continue;
        }
        if index - start > max_bytes {
            let boundary = if last_boundary > start {
                last_boundary
            } else {
                index
            };
            chunks.push(text[start..boundary].to_string());
            start = boundary;
        }
        last_boundary = index;
    }
    if text.len() - start > max_bytes && last_boundary > start {
        chunks.push(text[start..last_boundary].to_string());
        start = last_boundary;
    }
    if start < text.len() {
        chunks.push(text[start..].to_string());
    }
    chunks
}

pub(super) fn encode_content_body(bytes: &[u8], compress: bool) -> PactResult<EncodedContentBody> {
    if compress {
        let compressed = compress_zstd_chunk(bytes)?;
        if compressed.len() + 16 < bytes.len() {
            return Ok(EncodedContentBody {
                codec: "zstd".to_string(),
                encoding: Some("base64".to_string()),
                data: base64_encode(&compressed),
            });
        }
    }
    if std::str::from_utf8(bytes).is_err() {
        return Ok(EncodedContentBody {
            codec: "base64".to_string(),
            encoding: Some("base64".to_string()),
            data: base64_encode(bytes),
        });
    }
    Ok(EncodedContentBody {
        codec: "none".to_string(),
        encoding: None,
        data: String::from_utf8_lossy(bytes).to_string(),
    })
}

pub(super) fn content_texts_from_records(records: &[ScannedRecord]) -> BTreeMap<String, String> {
    let mut content = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "content") {
        if let Ok(payload) = serde_json::from_value::<ContentPayload>(record.payload.clone())
            && payload.text.is_some()
            && let Ok(text) = decode_content_payload_text(&payload)
        {
            content.entry(payload.id.clone()).or_insert(text);
        }
    }
    let chunks = content_chunks_from_records(records).unwrap_or_default();
    for (content_id, values) in chunks {
        if content.contains_key(&content_id) {
            continue;
        }
        content.insert(
            content_id,
            String::from_utf8(
                values
                    .into_iter()
                    .flat_map(|(_, bytes)| bytes)
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_default(),
        );
    }
    content
}

pub(super) fn content_chunks_from_records(
    records: &[ScannedRecord],
) -> PactResult<ContentChunkMap> {
    let mut payloads = BTreeMap::new();
    for record in records
        .iter()
        .filter(|record| record.kind == "content_chunk")
    {
        let content_id = record
            .payload
            .get("content_id")
            .and_then(Value::as_str)
            .ok_or_else(|| PactError::corrupt("content chunk missing content_id"))?
            .to_string();
        let ordinal = record
            .payload
            .get("ordinal")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                PactError::corrupt(format!("content chunk {content_id} missing ordinal"))
            })? as usize;
        payloads.insert((content_id, ordinal), record.payload.clone());
    }

    let mut memo: BTreeMap<(String, usize), Vec<u8>> = BTreeMap::new();
    let mut out: BTreeMap<String, Vec<(usize, Vec<u8>)>> = BTreeMap::new();
    let keys = payloads.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        let bytes = resolve_content_chunk_bytes(&key, &payloads, &mut memo, &mut BTreeSet::new())?;
        out.entry(key.0.clone()).or_default().push((key.1, bytes));
    }
    for values in out.values_mut() {
        values.sort_by_key(|(ordinal, _)| *ordinal);
    }
    Ok(out)
}

fn resolve_content_chunk_bytes(
    key: &(String, usize),
    payloads: &BTreeMap<(String, usize), Value>,
    memo: &mut BTreeMap<(String, usize), Vec<u8>>,
    seen: &mut BTreeSet<(String, usize)>,
) -> PactResult<Vec<u8>> {
    if let Some(bytes) = memo.get(key) {
        return Ok(bytes.clone());
    }
    if !seen.insert(key.clone()) {
        return Err(PactError::corrupt(format!(
            "content chunk ref cycle at {}:{}",
            key.0, key.1
        )));
    }
    let payload = payloads.get(key).ok_or_else(|| {
        PactError::not_found(format!("missing content chunk {}:{}", key.0, key.1))
    })?;
    let bytes = if payload.get("codec").and_then(Value::as_str) == Some("ref") {
        let ref_content_id = payload
            .get("ref_content_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                PactError::corrupt(format!(
                    "content chunk {}:{} missing ref_content_id",
                    key.0, key.1
                ))
            })?
            .to_string();
        let ref_ordinal = payload
            .get("ref_ordinal")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                PactError::corrupt(format!(
                    "content chunk {}:{} missing ref_ordinal",
                    key.0, key.1
                ))
            })? as usize;
        resolve_content_chunk_bytes(&(ref_content_id, ref_ordinal), payloads, memo, seen)?
    } else {
        decode_content_chunk_bytes(payload)?
    };
    seen.remove(key);
    memo.insert(key.clone(), bytes.clone());
    Ok(bytes)
}

pub(super) fn content_payloads_by_hash(
    records: &[ScannedRecord],
) -> PactResult<BTreeMap<String, ContentPayload>> {
    let mut by_hash = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "content") {
        let payload: ContentPayload = serde_json::from_value(record.payload.clone())
            .map_err(|err| PactError::codec(format!("invalid content record: {err}")))?;
        by_hash.entry(payload.hash.clone()).or_insert(payload);
    }
    Ok(by_hash)
}

pub(super) fn decode_content_payload_text(payload: &ContentPayload) -> PactResult<String> {
    let decoded = decode_content_payload_bytes(payload)?;
    String::from_utf8(decoded).map_err(|err| {
        PactError::codec(format!("decompressed content payload is not UTF-8: {err}"))
    })
}

pub(super) fn decode_content_payload_bytes(payload: &ContentPayload) -> PactResult<Vec<u8>> {
    let text = payload.text.as_deref().unwrap_or_default();
    match payload.codec.as_str() {
        "none" => Ok(text.as_bytes().to_vec()),
        "base64" => {
            let encoding = payload.encoding.as_deref().unwrap_or("base64");
            match encoding {
                "base64" => Ok(base64_decode(text)?),
                other => Err(PactError::codec(format!(
                    "unsupported base64 content payload encoding: {other}"
                ))),
            }
        }
        "zstd" => {
            let encoding = payload.encoding.as_deref().unwrap_or("base64");
            let compressed = match encoding {
                "base64" => base64_decode(text)?,
                other => {
                    return Err(PactError::codec(format!(
                        "unsupported zstd content payload encoding: {other}"
                    )));
                }
            };
            decompress_zstd_chunk(&compressed)
        }
        "external" => Err(PactError::codec(format!(
            "content {} is external",
            payload.id
        ))),
        other => Err(PactError::codec(format!(
            "unsupported content payload codec: {other}"
        ))),
    }
}

pub(super) fn decode_content_chunk_bytes(payload: &Value) -> PactResult<Vec<u8>> {
    let codec = payload
        .get("codec")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let text = payload
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match codec {
        "none" => Ok(text.as_bytes().to_vec()),
        "base64" => {
            let encoding = payload
                .get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("base64");
            match encoding {
                "base64" => Ok(base64_decode(text)?),
                other => Err(PactError::codec(format!(
                    "unsupported base64 content chunk encoding: {other}"
                ))),
            }
        }
        "zstd" => {
            let encoding = payload
                .get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("base64");
            let compressed = match encoding {
                "base64" => base64_decode(text)?,
                other => {
                    return Err(PactError::codec(format!(
                        "unsupported zstd content chunk encoding: {other}"
                    )));
                }
            };
            decompress_zstd_chunk(&compressed)
        }
        other => Err(PactError::codec(format!(
            "unsupported content chunk codec: {other}"
        ))),
    }
}

#[cfg(feature = "compression-zstd")]
pub(super) fn compress_zstd_chunk(chunk: &[u8]) -> PactResult<Vec<u8>> {
    zstd::stream::encode_all(chunk, 0)
        .map_err(|err| PactError::codec(format!("failed to compress zstd content chunk: {err}")))
}

#[cfg(feature = "compression-zstd")]
pub(super) fn compress_zstd_archive(chunk: &[u8]) -> PactResult<Vec<u8>> {
    zstd::stream::encode_all(chunk, 19)
        .map_err(|err| PactError::codec(format!("failed to compress zstd archive payload: {err}")))
}

#[cfg(not(feature = "compression-zstd"))]
pub(super) fn compress_zstd_chunk(_chunk: &[u8]) -> PactResult<Vec<u8>> {
    Err(PactError::codec(
        "zstd codec not enabled in this Pact build",
    ))
}

#[cfg(not(feature = "compression-zstd"))]
pub(super) fn compress_zstd_archive(_chunk: &[u8]) -> PactResult<Vec<u8>> {
    Err(PactError::codec(
        "zstd codec not enabled in this Pact build",
    ))
}

#[cfg(feature = "compression-zstd")]
pub(super) fn compress_zstd_with_dict(chunk: &[u8], dict: &[u8]) -> PactResult<Vec<u8>> {
    let mut encoder =
        zstd::stream::Encoder::with_dictionary(Vec::new(), 0, dict).map_err(|err| {
            PactError::codec(format!("failed to create zstd dictionary encoder: {err}"))
        })?;
    encoder.write_all(chunk).map_err(|err| {
        PactError::codec(format!("failed to write zstd dictionary payload: {err}"))
    })?;
    encoder
        .finish()
        .map_err(|err| PactError::codec(format!("failed to finish zstd dictionary payload: {err}")))
}

#[cfg(not(feature = "compression-zstd"))]
pub(super) fn compress_zstd_with_dict(_chunk: &[u8], _dict: &[u8]) -> PactResult<Vec<u8>> {
    Err(PactError::codec(
        "zstd codec not enabled in this Pact build",
    ))
}

#[cfg(feature = "compression-zstd")]
pub(super) fn decompress_zstd_chunk(chunk: &[u8]) -> PactResult<Vec<u8>> {
    zstd::stream::decode_all(chunk)
        .map_err(|err| PactError::codec(format!("failed to decompress zstd content chunk: {err}")))
}

#[cfg(not(feature = "compression-zstd"))]
pub(super) fn decompress_zstd_chunk(_chunk: &[u8]) -> PactResult<Vec<u8>> {
    Err(PactError::codec(
        "zstd codec not enabled in this Pact build",
    ))
}

pub(super) fn decompress_page_payload(payload: &[u8]) -> PactResult<Vec<u8>> {
    if let Some(compressed) = payload.strip_prefix(PAGE_DICT_MAGIC) {
        return decompress_zstd_with_dict(compressed, PACT_RECORD_ZSTD_DICT_V1);
    }
    decompress_zstd_chunk(payload)
}

#[cfg(feature = "compression-zstd")]
pub(super) fn decompress_zstd_with_dict(chunk: &[u8], dict: &[u8]) -> PactResult<Vec<u8>> {
    let cursor = Cursor::new(chunk);
    let mut decoder = zstd::stream::Decoder::with_dictionary(cursor, dict).map_err(|err| {
        PactError::codec(format!("failed to create zstd dictionary decoder: {err}"))
    })?;
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).map_err(|err| {
        PactError::codec(format!("failed to read zstd dictionary payload: {err}"))
    })?;
    Ok(decoded)
}

#[cfg(not(feature = "compression-zstd"))]
pub(super) fn decompress_zstd_with_dict(_chunk: &[u8], _dict: &[u8]) -> PactResult<Vec<u8>> {
    Err(PactError::codec(
        "zstd codec not enabled in this Pact build",
    ))
}

pub(super) fn content_payloads_by_id(
    records: &[ScannedRecord],
) -> BTreeMap<String, ContentPayload> {
    let mut by_id = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "content") {
        if let Ok(payload) = serde_json::from_value::<ContentPayload>(record.payload.clone()) {
            by_id.entry(payload.id.clone()).or_insert(payload);
        }
    }
    by_id
}

pub(super) fn content_preview_inline_by_id<'a>(
    content_by_id: &'a BTreeMap<String, ContentPayload>,
    content_id: &str,
) -> Option<&'a str> {
    content_by_id
        .get(content_id)
        .and_then(|payload| payload.preview_inline.as_deref())
}
