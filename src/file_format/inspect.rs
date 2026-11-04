use super::layout::{
    RECORD_PAYLOAD_MAGIC, TOKEN_SPAN_PART_DENSE_TOKEN_ID, TOKEN_SPAN_PART_DENSE_TOKEN_ID_RUN,
    TOKEN_SPAN_PART_LEX_REF, TOKEN_SPAN_PART_LEX_REF_RUN, TOKEN_SPAN_PART_SPAN_REF,
    TOKEN_SPAN_PART_SPAN_REF_RUN, TOKEN_SPAN_PART_TOKEN_ID, TOKEN_SPAN_PART_TOKEN_ID_RUN,
    TOKEN_SPAN_PART_TOKEN_MIXED_RUN, TOKEN_SPAN_PART_TOKEN_TABLE_RUN, TOKEN_SPAN_PART_UTF8_ESCAPE,
    TOKEN_SPAN_TOKENIZER_O200K_BASE_ID, VALUE_TAG_ARRAY, VALUE_TAG_BYTES, VALUE_TAG_F64,
    VALUE_TAG_FALSE, VALUE_TAG_FILE_LOCAL_ID_REF, VALUE_TAG_I64, VALUE_TAG_NULL, VALUE_TAG_OBJECT,
    VALUE_TAG_OBJECT_SYMBOL_KEYS, VALUE_TAG_STRING, VALUE_TAG_SYMBOL_REF,
    VALUE_TAG_TOKEN_SPAN_STRING, VALUE_TAG_TRUE, VALUE_TAG_U64, VALUE_TAG_VAR_U64,
    VALUE_TAG_ZIGZAG_I64,
};
use super::*;
use crate::atomic_output::{PublishMode, publish_bytes};
use crate::error::{PactError, PactResult};

type PageKindAccumulator = (usize, usize, usize, usize, BTreeMap<String, usize>);

fn avg_bytes(bytes: usize, count: usize) -> usize {
    bytes.checked_div(count).unwrap_or(0)
}

#[derive(Default)]
struct RefStats {
    content_refs: Vec<String>,
    external_ref_count: usize,
    inline_ref_count: usize,
    long_id_count: usize,
    pact_ref_vector_count: usize,
    estimated_pact_ref_key_bytes_saved: usize,
    repeated_strings: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default)]
struct NodePayloadAudit {
    sequence: u64,
    node_type: String,
    encoded_payload_bytes: usize,
    expanded_json_bytes: usize,
    content_bytes: usize,
}

#[derive(Clone, Debug, Default)]
struct TypeByteTotals {
    count: usize,
    encoded_payload_bytes: usize,
    expanded_json_bytes: usize,
    content_bytes: usize,
}

pub fn file_inspect_json(path: &Path) -> PactResult<String> {
    let scanned = scan_file_page_summary_from_path(path)?;
    serde_json::to_string_pretty(&scanned.manifest)
        .map_err(|err| PactError::codec(format!("failed to encode .pact manifest: {err}")))
}

pub fn file_inspect_full_json(path: &Path) -> PactResult<String> {
    let scanned = scan_file_cached(path, true)?;
    serde_json::to_string_pretty(&scanned.manifest)
        .map_err(|err| PactError::codec(format!("failed to encode .pact manifest: {err}")))
}

pub fn file_info_json(path: &Path) -> PactResult<String> {
    let scanned = scan_file_cached(path, true)?;
    let validation = validation_from_scan_ref(&scanned);
    let bundle = pact_bundle_info(path, &scanned);
    let pages = page_kind_info(&scanned.manifest);
    let record_kinds = record_kind_info(&scanned.records);
    let largest_records = largest_record_info(&scanned.records);
    let payload_kinds = component_payload_kind_info(&scanned.records);
    let content = content_storage_info(&scanned.records);
    let fields = field_size_info(&scanned.records);
    let hot_fields = hot_field_info(&scanned.records);
    let payload_slots = payload_slot_info(&scanned.records);
    let traces = trace_payload_info(&scanned.records);
    let counts = semantic_count_info(&scanned.records, &scanned.manifest);
    let refs = ref_info(&scanned.records);
    let symbols = symbol_info(&scanned.records);
    let smells = storage_smells(&scanned, &bundle, &fields);
    let live_shape = live_shape_info(path, &scanned);

    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.info.v1"),
        "path": path.display().to_string(),
        "file_bytes": scanned.manifest.file_bytes,
        "valid": validation.ok,
        "errors": validation.errors,
        "manifest": scanned.manifest,
        "counts": counts,
        "pages": pages,
        "records": record_kinds,
        "largest_records": largest_records,
        "component_payloads": payload_kinds,
        "content": content,
        "fields": fields,
        "hot_fields": hot_fields,
        "payload_slots": payload_slots,
        "traces": traces,
        "refs": refs,
        "symbols": symbols,
        "bundle": bundle,
        "smells": smells,
        "live_shape": live_shape,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact info: {err}")))
}

pub fn file_storage_audit_json(path: &Path) -> PactResult<String> {
    let scanned = scan_file_cached(path, true)?;
    let validation = validation_from_scan_ref(&scanned);
    let fields = field_size_info(&scanned.records);
    let bundle = pact_bundle_info(path, &scanned);
    let smells = storage_smells(&scanned, &bundle, &fields);
    let traces = trace_payload_info(&scanned.records);

    let mut audit = json!({
        "schema": format!("{PACT_FILE_SCHEMA}.storage_audit.v1"),
        "path": path.display().to_string(),
        "file_bytes": scanned.manifest.file_bytes,
        "valid": validation.ok,
        "errors": validation.errors,
        "traces": traces,
        "smells": smells,
    });
    if crate::pact_file::is_v2_file(&std::fs::read(path)?) {
        audit["physical_layout"] = serde_json::to_value(
            crate::pact_file::layout_metrics::measure_path(path)?,
        )
        .map_err(|err| PactError::codec(format!("failed to encode physical layout: {err}")))?;
    }

    serde_json::to_string_pretty(&audit)
        .map_err(|err| PactError::codec(format!("failed to encode .pact storage audit: {err}")))
}

pub fn file_live_shape_info_json(path: &Path) -> PactResult<String> {
    let scanned = scan_file_cached(path, true)?;
    let validation = validation_from_scan_ref(&scanned);
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.live_shape.v1"),
        "path": path.display().to_string(),
        "valid": validation.ok,
        "errors": validation.errors,
        "live_shape": live_shape_info(path, &scanned),
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact live shape info: {err}")))
}

pub fn file_token_span_stats_json(path: &Path) -> PactResult<String> {
    let raw = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let bytes = if crate::pact_pack::is_packed_v1(&raw) {
        super::history::packed_v1_to_v0_bytes(&raw)?
    } else {
        raw
    };
    let manifest = scan_file(&bytes, false).manifest;
    let mut stats = TokenSpanStats::default();
    let mut page_token_stats = Vec::new();
    let mut offset = FILE_HEADER_LEN;
    while offset < bytes.len() {
        if bytes
            .get(offset..offset + FILE_FOOTER_MAGIC.len())
            .is_some_and(|magic| magic == FILE_FOOTER_MAGIC)
        {
            offset += FILE_FOOTER_LEN;
            continue;
        }
        if bytes.get(offset..offset + 2) != Some(b"PG") {
            break;
        }
        let page_offset = offset;
        let raw_page_kind = read_u16(&bytes, offset + 2).unwrap_or_default();
        let page_seq = read_u64(&bytes, offset + 4).unwrap_or_default();
        let Some((page_kind, page_compressed)) = decode_page_kind(raw_page_kind) else {
            return Err(PactError::corrupt(format!(
                "unknown page kind {raw_page_kind} at offset {offset}"
            )));
        };
        let record_count = read_u32(&bytes, offset + 12).unwrap_or_default() as usize;
        let payload_len = read_u64(&bytes, offset + 16).unwrap_or_default() as usize;
        let payload_start = offset + PAGE_HEADER_LEN;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or_else(|| PactError::corrupt("page payload offset overflow"))?;
        let page_payload = bytes.get(payload_start..payload_end).ok_or_else(|| {
            PactError::corrupt(format!("truncated page payload at offset {page_offset}"))
        })?;
        stats.pages_seen += 1;
        stats.records_seen += record_count;
        let decoded_payload = if page_compressed {
            decompress_page_payload(page_payload)?
        } else {
            page_payload.to_vec()
        };
        let mut page_stats = TokenSpanStats {
            pages_seen: 1,
            records_seen: record_count,
            ..TokenSpanStats::default()
        };
        inspect_page_token_spans(page_kind, page_seq, &decoded_payload, &mut page_stats)?;
        page_token_stats.push(token_span_page_summary(
            page_kind,
            page_seq,
            payload_len,
            decoded_payload.len(),
            &page_stats,
        ));
        stats.merge(page_stats);
        offset = payload_end;
    }
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.token_span_stats.v1"),
        "path": path.display().to_string(),
        "file_bytes": bytes.len(),
        "manifest_pages": manifest.pages,
        "pages_seen": stats.pages_seen,
        "records_seen": stats.records_seen,
        "page_token_stats": page_token_stats,
        "records_with_token_span_strings": stats.records_with_token_span_strings,
        "token_span_strings": stats.token_span_strings,
        "token_span_encoded_bytes": stats.token_span_encoded_bytes,
        "tokenizer_ids": stats.tokenizer_ids,
        "part_counts": stats.part_counts,
        "part_bytes": stats.part_bytes,
        "literal_token_count": stats.literal_token_count,
        "literal_token_varint_bytes": stats.literal_token_varint_bytes,
        "literal_token_avg_varint_width": avg_bytes(stats.literal_token_varint_bytes, stats.literal_token_count),
        "literal_token_width_buckets": {
            "one_byte": stats.literal_token_width_buckets[0],
            "two_byte": stats.literal_token_width_buckets[1],
            "three_byte": stats.literal_token_width_buckets[2],
            "four_plus_byte": stats.literal_token_width_buckets[3],
        },
        "dense_token_refs": stats.dense_token_refs,
        "lex_refs": stats.lex_refs,
        "span_refs": stats.span_refs,
        "utf8_escape_bytes": stats.utf8_escape_bytes,
        "token_table_runs": stats.token_table_runs,
        "token_table_tokens": stats.token_table_tokens,
        "token_mixed_runs": stats.token_mixed_runs,
        "token_mixed_tokens": stats.token_mixed_tokens,
        "distinct_literal_tokens": stats.literal_token_counts.len(),
        "top_literal_tokens": top_token_counts(&stats.literal_token_counts, 24),
        "record_kinds": stats.record_kinds,
        "errors": stats.errors,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode token span stats: {err}")))
}

pub fn file_page_compare_json(baseline_path: &Path, candidate_path: &Path) -> PactResult<String> {
    let baseline = scan_file_cached(baseline_path, true)?;
    let candidate = scan_file_cached(candidate_path, true)?;
    let baseline_pages = page_totals_by_kind(&baseline.manifest);
    let candidate_pages = page_totals_by_kind(&candidate.manifest);
    let mut kinds = baseline_pages.keys().cloned().collect::<BTreeSet<_>>();
    kinds.extend(candidate_pages.keys().cloned());
    let pages = kinds
        .into_iter()
        .map(|kind| {
            let baseline = baseline_pages.get(&kind).copied().unwrap_or_default();
            let candidate = candidate_pages.get(&kind).copied().unwrap_or_default();
            let stored_delta = candidate.0 as i128 - baseline.0 as i128;
            let decoded_delta = candidate.1 as i128 - baseline.1 as i128;
            json!({
                "page_type": kind,
                "baseline_stored_payload_bytes": baseline.0,
                "candidate_stored_payload_bytes": candidate.0,
                "stored_delta_bytes": stored_delta,
                "baseline_decoded_payload_bytes": baseline.1,
                "candidate_decoded_payload_bytes": candidate.1,
                "decoded_delta_bytes": decoded_delta,
                "decoded_win_stored_loss": decoded_delta < 0 && stored_delta > 0,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.page_compare.v1"),
        "baseline_path": baseline_path.display().to_string(),
        "candidate_path": candidate_path.display().to_string(),
        "baseline_file_bytes": baseline.manifest.file_bytes,
        "candidate_file_bytes": candidate.manifest.file_bytes,
        "file_delta_bytes": candidate.manifest.file_bytes as i128 - baseline.manifest.file_bytes as i128,
        "pages": pages,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode page comparison: {err}")))
}

pub fn file_manifest_info_json(path: &Path) -> PactResult<String> {
    let scanned = scan_file_page_summary_from_path(path)?;
    let pages = page_kind_info(&scanned.manifest);
    let file_bytes = scanned.manifest.file_bytes;
    let valid = scanned.errors.is_empty();
    let record_count = scanned.records.len();
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.manifest_info.v1"),
        "path": path.display().to_string(),
        "file_bytes": file_bytes,
        "valid": valid,
        "errors": scanned.errors,
        "manifest": scanned.manifest,
        "pages": pages,
        "records_collected": record_count,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact manifest info: {err}")))
}

fn page_totals_by_kind(manifest: &PactFileManifest) -> BTreeMap<String, (usize, usize)> {
    let mut totals = BTreeMap::<String, (usize, usize)>::new();
    for page in &manifest.pages {
        let entry = totals.entry(page.page_type.clone()).or_default();
        entry.0 += page.stored_payload_bytes;
        entry.1 += page.decoded_payload_bytes;
    }
    totals
}

#[derive(Default)]
struct TokenSpanStats {
    pages_seen: usize,
    records_seen: usize,
    records_with_token_span_strings: usize,
    token_span_strings: usize,
    token_span_encoded_bytes: usize,
    tokenizer_ids: BTreeMap<String, usize>,
    part_counts: BTreeMap<String, usize>,
    part_bytes: BTreeMap<String, usize>,
    literal_token_count: usize,
    literal_token_varint_bytes: usize,
    literal_token_width_buckets: [usize; 4],
    literal_token_counts: BTreeMap<u64, usize>,
    dense_token_refs: usize,
    lex_refs: usize,
    span_refs: usize,
    utf8_escape_bytes: usize,
    token_table_runs: usize,
    token_table_tokens: usize,
    token_mixed_runs: usize,
    token_mixed_tokens: usize,
    record_kinds: BTreeMap<String, usize>,
    errors: Vec<String>,
}

fn inspect_page_token_spans(
    page_kind: PageKind,
    page_seq: u64,
    payload: &[u8],
    stats: &mut TokenSpanStats,
) -> PactResult<()> {
    let mut offset = 0usize;
    while offset < payload.len() {
        if payload.len() - offset < RECORD_HEADER_LEN {
            return Err(PactError::corrupt(format!(
                "truncated record header in page {page_seq} at payload offset {offset}"
            )));
        }
        let record_start = offset;
        let raw_record_kind = read_u16(payload, offset).unwrap_or_default();
        offset += 2;
        let _record_seq = read_u32(payload, offset).unwrap_or_default();
        offset += 4;
        let record_payload_len = read_u64(payload, offset).unwrap_or_default() as usize;
        offset += 8;
        let _checksum = read_u64(payload, offset).unwrap_or_default();
        offset += 8;
        let record_payload = payload
            .get(offset..offset + record_payload_len)
            .ok_or_else(|| {
                PactError::corrupt(format!(
                    "truncated record payload in page {page_seq} at payload offset {record_start}"
                ))
            })?;
        offset += record_payload_len;
        let kind = RecordKind::from_u16(raw_record_kind)
            .map(RecordKind::as_str)
            .unwrap_or("unknown")
            .to_string();
        *stats.record_kinds.entry(kind).or_default() += 1;
        let before = stats.token_span_strings;
        if let Err(error) = inspect_record_payload_token_spans(record_payload, stats) {
            stats.errors.push(format!(
                "page {page_seq} ({}) record at payload offset {record_start}: {error}",
                page_kind.as_str()
            ));
        }
        if stats.token_span_strings > before {
            stats.records_with_token_span_strings += 1;
        }
    }
    Ok(())
}

fn inspect_record_payload_token_spans(
    payload: &[u8],
    stats: &mut TokenSpanStats,
) -> PactResult<()> {
    if payload.len() < RECORD_PAYLOAD_MAGIC.len()
        || &payload[..RECORD_PAYLOAD_MAGIC.len()] != RECORD_PAYLOAD_MAGIC
    {
        return Err(PactError::corrupt(
            "record payload missing Pact value magic",
        ));
    }
    let mut offset = RECORD_PAYLOAD_MAGIC.len();
    inspect_value_token_spans(payload, &mut offset, stats)?;
    Ok(())
}

fn inspect_value_token_spans(
    bytes: &[u8],
    offset: &mut usize,
    stats: &mut TokenSpanStats,
) -> PactResult<()> {
    let tag = read_diag_byte(bytes, offset)?;
    match tag {
        VALUE_TAG_NULL | VALUE_TAG_FALSE | VALUE_TAG_TRUE => Ok(()),
        VALUE_TAG_I64 | VALUE_TAG_U64 | VALUE_TAG_F64 => {
            skip_diag_bytes(bytes, offset, 8)?;
            Ok(())
        }
        VALUE_TAG_VAR_U64 | VALUE_TAG_ZIGZAG_I64 | VALUE_TAG_FILE_LOCAL_ID_REF => {
            let _ = read_diag_varint(bytes, offset)?;
            Ok(())
        }
        VALUE_TAG_STRING | VALUE_TAG_BYTES => {
            let len = read_diag_len(bytes, offset)?;
            skip_diag_bytes(bytes, offset, len)?;
            Ok(())
        }
        VALUE_TAG_TOKEN_SPAN_STRING => inspect_token_span_string(bytes, offset, stats),
        VALUE_TAG_SYMBOL_REF => {
            skip_diag_bytes(bytes, offset, 4)?;
            Ok(())
        }
        VALUE_TAG_ARRAY => {
            let len = read_diag_len(bytes, offset)?;
            for _ in 0..len {
                inspect_value_token_spans(bytes, offset, stats)?;
            }
            Ok(())
        }
        VALUE_TAG_OBJECT => {
            let len = read_diag_len(bytes, offset)?;
            for _ in 0..len {
                let key_len = read_diag_len(bytes, offset)?;
                skip_diag_bytes(bytes, offset, key_len)?;
                inspect_value_token_spans(bytes, offset, stats)?;
            }
            Ok(())
        }
        VALUE_TAG_OBJECT_SYMBOL_KEYS => {
            let len = read_diag_len(bytes, offset)?;
            for _ in 0..len {
                let _ = read_diag_varint(bytes, offset)?;
                inspect_value_token_spans(bytes, offset, stats)?;
            }
            Ok(())
        }
        _ => Err(PactError::corrupt(format!(
            "unknown Pact value tag 0x{tag:02x}"
        ))),
    }
}

fn inspect_token_span_string(
    bytes: &[u8],
    offset: &mut usize,
    stats: &mut TokenSpanStats,
) -> PactResult<()> {
    let start = offset.saturating_sub(1);
    let (tokenizer_id, _) = read_diag_varint(bytes, offset)?;
    let tokenizer_name = if tokenizer_id == TOKEN_SPAN_TOKENIZER_O200K_BASE_ID {
        "o200k_base".to_string()
    } else {
        format!("unknown:{tokenizer_id}")
    };
    *stats.tokenizer_ids.entry(tokenizer_name).or_default() += 1;
    let (part_count, _) = read_diag_varint(bytes, offset)?;
    for _ in 0..part_count {
        let part_start = *offset;
        let part_tag = read_diag_byte(bytes, offset)?;
        match part_tag {
            TOKEN_SPAN_PART_TOKEN_ID => {
                let (token, width) = read_diag_varint(bytes, offset)?;
                stats.record_literal_token(token, width);
                stats.record_part("token_id", *offset - part_start);
            }
            TOKEN_SPAN_PART_TOKEN_ID_RUN => {
                let (len, _) = read_diag_varint(bytes, offset)?;
                for _ in 0..len {
                    let (token, width) = read_diag_varint(bytes, offset)?;
                    stats.record_literal_token(token, width);
                }
                stats.record_part("token_id_run", *offset - part_start);
            }
            TOKEN_SPAN_PART_LEX_REF => {
                let _ = read_diag_varint(bytes, offset)?;
                stats.lex_refs += 1;
                stats.record_part("lex_ref", *offset - part_start);
            }
            TOKEN_SPAN_PART_LEX_REF_RUN => {
                let (len, _) = read_diag_varint(bytes, offset)?;
                for _ in 0..len {
                    let _ = read_diag_varint(bytes, offset)?;
                }
                stats.lex_refs += len as usize;
                stats.record_part("lex_ref_run", *offset - part_start);
            }
            TOKEN_SPAN_PART_SPAN_REF => {
                let _ = read_diag_varint(bytes, offset)?;
                stats.span_refs += 1;
                stats.record_part("span_ref", *offset - part_start);
            }
            TOKEN_SPAN_PART_SPAN_REF_RUN => {
                let (len, _) = read_diag_varint(bytes, offset)?;
                for _ in 0..len {
                    let _ = read_diag_varint(bytes, offset)?;
                }
                stats.span_refs += len as usize;
                stats.record_part("span_ref_run", *offset - part_start);
            }
            TOKEN_SPAN_PART_DENSE_TOKEN_ID => {
                let _ = read_diag_varint(bytes, offset)?;
                stats.dense_token_refs += 1;
                stats.record_part("dense_token_id", *offset - part_start);
            }
            TOKEN_SPAN_PART_DENSE_TOKEN_ID_RUN => {
                let (len, _) = read_diag_varint(bytes, offset)?;
                for _ in 0..len {
                    let _ = read_diag_varint(bytes, offset)?;
                }
                stats.dense_token_refs += len as usize;
                stats.record_part("dense_token_id_run", *offset - part_start);
            }
            TOKEN_SPAN_PART_TOKEN_MIXED_RUN => {
                let (len, _) = read_diag_varint(bytes, offset)?;
                let bitset_len = (len as usize).div_ceil(8);
                let bitset_start = *offset;
                skip_diag_bytes(bytes, offset, bitset_len)?;
                for index in 0..len as usize {
                    let (value, width) = read_diag_varint(bytes, offset)?;
                    if bytes[bitset_start + index / 8] & (1 << (index % 8)) == 0 {
                        stats.record_literal_token(value, width);
                    } else {
                        stats.dense_token_refs += 1;
                    }
                }
                stats.token_mixed_runs += 1;
                stats.token_mixed_tokens += len as usize;
                stats.record_part("token_mixed_run", *offset - part_start);
            }
            TOKEN_SPAN_PART_TOKEN_TABLE_RUN => {
                let (len, _) = read_diag_varint(bytes, offset)?;
                let (vocab_len, _) = read_diag_varint(bytes, offset)?;
                let width = read_diag_byte(bytes, offset)?;
                let mut vocab = Vec::with_capacity(vocab_len as usize);
                for _ in 0..vocab_len {
                    let (token, _) = read_diag_varint(bytes, offset)?;
                    vocab.push(token);
                }
                for _ in 0..len {
                    let index = match width {
                        1 => usize::from(read_diag_byte(bytes, offset)?),
                        2 => {
                            let raw = read_diag_array::<2>(bytes, offset)?;
                            usize::from(u16::from_le_bytes(raw))
                        }
                        _ => {
                            return Err(PactError::corrupt(format!(
                                "unsupported token table width {width}"
                            )));
                        }
                    };
                    if let Some(token) = vocab.get(index).copied() {
                        *stats.literal_token_counts.entry(token).or_default() += 1;
                    }
                }
                stats.token_table_runs += 1;
                stats.token_table_tokens += len as usize;
                stats.record_part("token_table_run", *offset - part_start);
            }
            TOKEN_SPAN_PART_UTF8_ESCAPE => {
                let len = read_diag_len(bytes, offset)?;
                skip_diag_bytes(bytes, offset, len)?;
                stats.utf8_escape_bytes += len;
                stats.record_part("utf8_escape", *offset - part_start);
            }
            _ => {
                return Err(PactError::corrupt(format!(
                    "unknown token-span part tag 0x{part_tag:02x}"
                )));
            }
        }
    }
    stats.token_span_strings += 1;
    stats.token_span_encoded_bytes += *offset - start;
    Ok(())
}

impl TokenSpanStats {
    fn merge(&mut self, other: TokenSpanStats) {
        self.pages_seen += other.pages_seen;
        self.records_seen += other.records_seen;
        self.records_with_token_span_strings += other.records_with_token_span_strings;
        self.token_span_strings += other.token_span_strings;
        self.token_span_encoded_bytes += other.token_span_encoded_bytes;
        self.literal_token_count += other.literal_token_count;
        self.literal_token_varint_bytes += other.literal_token_varint_bytes;
        self.dense_token_refs += other.dense_token_refs;
        self.lex_refs += other.lex_refs;
        self.span_refs += other.span_refs;
        self.utf8_escape_bytes += other.utf8_escape_bytes;
        self.token_table_runs += other.token_table_runs;
        self.token_table_tokens += other.token_table_tokens;
        self.token_mixed_runs += other.token_mixed_runs;
        self.token_mixed_tokens += other.token_mixed_tokens;
        for index in 0..self.literal_token_width_buckets.len() {
            self.literal_token_width_buckets[index] += other.literal_token_width_buckets[index];
        }
        merge_usize_map(&mut self.tokenizer_ids, other.tokenizer_ids);
        merge_usize_map(&mut self.part_counts, other.part_counts);
        merge_usize_map(&mut self.part_bytes, other.part_bytes);
        merge_usize_map(&mut self.literal_token_counts, other.literal_token_counts);
        merge_usize_map(&mut self.record_kinds, other.record_kinds);
        self.errors.extend(other.errors);
    }

    fn record_part(&mut self, kind: &str, bytes: usize) {
        *self.part_counts.entry(kind.to_string()).or_default() += 1;
        *self.part_bytes.entry(kind.to_string()).or_default() += bytes;
    }

    fn record_literal_token(&mut self, token: u64, width: usize) {
        self.literal_token_count += 1;
        self.literal_token_varint_bytes += width;
        let bucket = match width {
            0 | 1 => 0,
            2 => 1,
            3 => 2,
            _ => 3,
        };
        self.literal_token_width_buckets[bucket] += 1;
        *self.literal_token_counts.entry(token).or_default() += 1;
    }
}

fn token_span_page_summary(
    page_kind: PageKind,
    page_seq: u64,
    stored_payload_bytes: usize,
    decoded_payload_bytes: usize,
    stats: &TokenSpanStats,
) -> Value {
    json!({
        "page_type": page_kind.as_str(),
        "page_seq": page_seq,
        "stored_payload_bytes": stored_payload_bytes,
        "decoded_payload_bytes": decoded_payload_bytes,
        "token_span_strings": stats.token_span_strings,
        "token_span_encoded_bytes": stats.token_span_encoded_bytes,
        "literal_token_count": stats.literal_token_count,
        "literal_token_avg_varint_width": avg_bytes(stats.literal_token_varint_bytes, stats.literal_token_count),
        "distinct_literal_tokens": stats.literal_token_counts.len(),
        "part_counts": stats.part_counts,
        "part_bytes": stats.part_bytes,
        "lex_refs": stats.lex_refs,
        "span_refs": stats.span_refs,
        "dense_token_refs": stats.dense_token_refs,
        "token_table_runs": stats.token_table_runs,
        "token_mixed_runs": stats.token_mixed_runs,
        "errors": stats.errors,
    })
}

fn merge_usize_map<K: Ord>(target: &mut BTreeMap<K, usize>, source: BTreeMap<K, usize>) {
    for (key, value) in source {
        *target.entry(key).or_default() += value;
    }
}

fn top_token_counts(counts: &BTreeMap<u64, usize>, limit: usize) -> Vec<Value> {
    let mut rows = counts
        .iter()
        .map(|(token, count)| (*token, *count))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows.into_iter()
        .take(limit)
        .map(|(token, count)| json!({ "token": token, "count": count }))
        .collect()
}

fn read_diag_byte(bytes: &[u8], offset: &mut usize) -> PactResult<u8> {
    let start = *offset;
    let byte = *bytes.get(start).ok_or_else(|| {
        PactError::corrupt(format!("unexpected end of payload at offset {start}"))
    })?;
    *offset += 1;
    Ok(byte)
}

fn read_diag_len(bytes: &[u8], offset: &mut usize) -> PactResult<usize> {
    let (len, _) = read_diag_varint(bytes, offset)?;
    usize::try_from(len)
        .map_err(|_| PactError::corrupt(format!("value length {len} exceeds usize")))
}

fn read_diag_varint(bytes: &[u8], offset: &mut usize) -> PactResult<(u64, usize)> {
    let start = *offset;
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        if shift >= 64 {
            return Err(PactError::corrupt("varint length exceeds u64"));
        }
        let byte = read_diag_byte(bytes, offset)?;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, *offset - start));
        }
        shift += 7;
    }
}

fn skip_diag_bytes(bytes: &[u8], offset: &mut usize, len: usize) -> PactResult<()> {
    let start = *offset;
    let end = start
        .checked_add(len)
        .ok_or_else(|| PactError::corrupt("payload offset overflow"))?;
    bytes.get(start..end).ok_or_else(|| {
        PactError::corrupt(format!(
            "unexpected end of payload at offset {start} for {len} bytes"
        ))
    })?;
    *offset = end;
    Ok(())
}

fn read_diag_array<const N: usize>(bytes: &[u8], offset: &mut usize) -> PactResult<[u8; N]> {
    let start = *offset;
    let end = start
        .checked_add(N)
        .ok_or_else(|| PactError::corrupt("payload offset overflow"))?;
    let value = bytes.get(start..end).ok_or_else(|| {
        PactError::corrupt(format!(
            "unexpected end of payload at offset {start} for {N} bytes"
        ))
    })?;
    *offset = end;
    value
        .try_into()
        .map_err(|_| PactError::corrupt("failed to read fixed-width payload value"))
}

pub fn file_bench_storage_json(path: &Path) -> PactResult<String> {
    file_bench_storage_with_profile_json(path, None)
}

pub fn file_bench_storage_with_profile_json(
    path: &Path,
    target_profile: Option<&str>,
) -> PactResult<String> {
    let read_start = Instant::now();
    let bytes = fs::read(path)
        .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
    let read_ms = read_start.elapsed().as_secs_f64() * 1000.0;

    let scan_bytes: std::borrow::Cow<[u8]> = if crate::pact_pack::is_packed_v1(&bytes) {
        std::borrow::Cow::Owned(super::history::packed_v1_to_v0_bytes(&bytes)?)
    } else {
        std::borrow::Cow::Borrowed(&bytes)
    };
    let scan_slice: &[u8] = &scan_bytes;

    let manifest_scan_start = Instant::now();
    let manifest_scan = scan_file(scan_slice, false);
    let manifest_scan_ms = manifest_scan_start.elapsed().as_secs_f64() * 1000.0;

    let full_scan_start = Instant::now();
    let full_scan = scan_file(scan_slice, true);
    let full_scan_ms = full_scan_start.elapsed().as_secs_f64() * 1000.0;

    let project_head_start = Instant::now();
    let projected_head_result = if full_scan.errors.is_empty() {
        file_project_head_json(path).and_then(|json| {
            serde_json::from_str::<Value>(&json)
                .map(|value| (json.len(), value))
                .map_err(|err| PactError::codec(format!("failed to decode projected head: {err}")))
        })
    } else {
        Err(PactError::corrupt(full_scan.errors.join("; ")))
    };
    let project_head_ms = project_head_start.elapsed().as_secs_f64() * 1000.0;

    invalidate_scan_cache(path);
    let cached_first_start = Instant::now();
    let _cached_first = scan_file_cached(path, true)?;
    let cached_first_ms = cached_first_start.elapsed().as_secs_f64() * 1000.0;
    let cached_second_start = Instant::now();
    let _cached_second = scan_file_cached(path, true)?;
    let cached_second_ms = cached_second_start.elapsed().as_secs_f64() * 1000.0;
    let cache_snapshot = scan_cache_snapshot();
    let fast_head_start = Instant::now();
    let fast_head_result = file_open_head_fast_json(path);
    let fast_head_ms = fast_head_start.elapsed().as_secs_f64() * 1000.0;

    let preview_lookup_start = Instant::now();
    let content_by_id = content_payloads_by_id(&full_scan.records);
    let preview_lookup_count = content_by_id
        .keys()
        .filter(|content_id| content_preview_inline_by_id(&content_by_id, content_id).is_some())
        .count();
    let preview_lookup_ms = preview_lookup_start.elapsed().as_secs_f64() * 1000.0;

    let full_state_jsonl = expanded_record_jsonl_bytes(&full_scan.records);
    let full_state_jsonl_bytes = full_state_jsonl.len();
    let full_state_jsonl_gzip_bytes = gzip_len(&full_state_jsonl).ok();
    let provider_thread_jsonl = projected_head_result
        .as_ref()
        .map(|(_, tree)| provider_thread_jsonl_bytes_from_tree(tree))
        .unwrap_or_default();
    let provider_thread_jsonl_bytes = provider_thread_jsonl.len();
    let provider_thread_jsonl_gzip_bytes = gzip_len(&provider_thread_jsonl).ok();
    let whole_file_zstd_bytes = compress_zstd_chunk(&bytes)
        .ok()
        .map(|compressed| compressed.len());
    let semantic_archive_zstd_bytes = semantic_archive_zstd_bytes(&full_scan.records).ok();
    let raw_pact_to_full_state_jsonl_gzip_ratio =
        full_state_jsonl_gzip_bytes.map(|gzip| ratio(bytes.len(), gzip));
    let pact_zstd_to_full_state_jsonl_gzip_ratio = whole_file_zstd_bytes
        .zip(full_state_jsonl_gzip_bytes)
        .map(|(pact_zstd, gzip)| ratio(pact_zstd, gzip));
    let pact_archive_zstd_to_full_state_jsonl_gzip_ratio = semantic_archive_zstd_bytes
        .zip(full_state_jsonl_gzip_bytes)
        .map(|(archive_zstd, gzip)| ratio(archive_zstd, gzip));
    let raw_pact_to_provider_thread_jsonl_gzip_ratio =
        provider_thread_jsonl_gzip_bytes.map(|gzip| ratio(bytes.len(), gzip));
    let target = target_profile.map(storage_bench_target).transpose()?;
    let violations = target
        .as_ref()
        .map(|target| storage_bench_violations(bytes.len(), target))
        .unwrap_or_default();
    let target_json = target
        .as_ref()
        .map(|target| {
            json!({
                "profile": target.profile,
                "max_file_bytes": target.max_file_bytes,
                "stretch_file_bytes": target.stretch_file_bytes,
                "pass": violations.is_empty(),
                "violations": violations,
            })
        })
        .unwrap_or(Value::Null);

    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.bench_storage.v1"),
        "path": path.display().to_string(),
        "target": target_json,
        "file_bytes": bytes.len(),
        "valid": full_scan.errors.is_empty(),
        "errors": full_scan.errors,
        "record_count": full_scan.manifest.record_count,
        "cycle_count": full_scan.manifest.cycle_count,
        "timings_ms": {
            "read_file": read_ms,
            "scan_manifest": manifest_scan_ms,
            "scan_records": full_scan_ms,
            "project_head": project_head_ms,
            "cached_scan_records_first": cached_first_ms,
            "cached_scan_records_second": cached_second_ms,
            "fast_head_open": fast_head_ms,
            "preview_lookup": preview_lookup_ms,
        },
        "whole_file_compression": {
            "zstd_bytes": whole_file_zstd_bytes,
            "semantic_archive_zstd_bytes": semantic_archive_zstd_bytes,
        },
        "jsonl_comparison": {
            "expanded_full_state_jsonl_bytes": full_state_jsonl_bytes,
            "expanded_full_state_jsonl_gzip_bytes": full_state_jsonl_gzip_bytes,
            "provider_thread_jsonl_bytes": provider_thread_jsonl_bytes,
            "provider_thread_jsonl_gzip_bytes": provider_thread_jsonl_gzip_bytes,
            "ratio_pact_to_full_state_jsonl_gzip": raw_pact_to_full_state_jsonl_gzip_ratio,
            "ratio_pact_zstd_to_full_state_jsonl_gzip": pact_zstd_to_full_state_jsonl_gzip_ratio,
            "ratio_pact_archive_zstd_to_full_state_jsonl_gzip": pact_archive_zstd_to_full_state_jsonl_gzip_ratio,
            "ratio_pact_to_provider_thread_jsonl_gzip": raw_pact_to_provider_thread_jsonl_gzip_ratio,
        },
        "scan_cache": {
            "hits": cache_snapshot.0,
            "misses": cache_snapshot.1,
            "invalidations": cache_snapshot.2,
        },
        "manifest_only": {
            "record_count": manifest_scan.manifest.record_count,
            "page_count": manifest_scan.manifest.page_count,
            "last_valid_offset": manifest_scan.manifest.last_valid_offset,
        },
        "preview_lookup": {
            "count": preview_lookup_count,
        },
        "fast_head_valid": fast_head_result.is_ok(),
        "projected_head_bytes": projected_head_result.map(|(bytes, _)| bytes).unwrap_or_default(),
    }))
    .map_err(|err| PactError::codec(format!("failed to encode .pact storage bench: {err}")))
}

pub fn file_export_storage_profile_json(
    source_path: &Path,
    output_path: &Path,
    profile: &str,
) -> PactResult<String> {
    let profile = StorageProfile::parse(profile)?;
    let bytes = fs::read(source_path)
        .map_err(|err| PactError::storage(format!("failed to read source .pact file: {err}")))?;
    let scanned = scan_file(&bytes, true);
    if !scanned.errors.is_empty() {
        return Err(PactError::corrupt(format!(
            "cannot export storage profile from invalid .pact file: {}",
            scanned.errors.join("; ")
        )));
    }
    let head_before = file_project_head_json(source_path)?;
    let profile_stats = storage_profile_slimming_stats(&scanned.records);
    let full_state_jsonl = expanded_record_jsonl_bytes(&scanned.records);
    let full_state_jsonl_gzip_bytes = gzip_len(&full_state_jsonl).ok();
    let (output_bytes, page_addressable, appendable) = match profile {
        StorageProfile::Hot => (bytes.clone(), true, true),
        StorageProfile::Compact => (bytes.clone(), true, true),
        StorageProfile::Archive => (
            semantic_archive_payload_bytes(&scanned.records)?,
            false,
            false,
        ),
    };
    publish_bytes(output_path, &output_bytes, PublishMode::CreateNew)?;
    let head_preserved = if page_addressable {
        invalidate_scan_cache(output_path);
        let head_after = file_project_head_json(output_path)?;
        if !projected_heads_equal(&head_after, &head_before)? {
            return Err(PactError::storage(format!(
                "storage profile {} changed provider-visible head",
                profile.as_str()
            )));
        }
        true
    } else {
        let head_after = semantic_archive_projected_head_json(&output_bytes)?;
        projected_heads_equal(&head_after, &head_before)?
    };
    serde_json::to_string_pretty(&json!({
        "schema": format!("{PACT_FILE_SCHEMA}.storage_profile"),
        "profile": profile.as_str(),
        "source_path": source_path.display().to_string(),
        "output_path": output_path.display().to_string(),
        "source_bytes": bytes.len(),
        "output_bytes": output_bytes.len(),
        "page_addressable": page_addressable,
        "appendable": appendable,
        "head_preserved": head_preserved,
        "full_state_jsonl_gzip_bytes": full_state_jsonl_gzip_bytes,
        "ratio_to_full_state_jsonl_gzip": full_state_jsonl_gzip_bytes.map(|gzip| ratio(output_bytes.len(), gzip)),
        "omitted_preview_bytes": profile_stats.preview_bytes,
        "omitted_index_bytes": if page_addressable { 0 } else { profile_stats.index_bytes },
        "omitted_duplicate_base_bytes": profile_stats.duplicate_base_bytes,
    }))
    .map_err(|err| PactError::codec(format!("failed to encode storage profile report: {err}")))
}

fn semantic_archive_payload_bytes(records: &[ScannedRecord]) -> PactResult<Vec<u8>> {
    let root_store = ledger_from_records(records)?;
    let text_address_indexes = records
        .iter()
        .filter(|record| record.kind == "index")
        .filter(|record| record.payload.get("name").and_then(Value::as_str) == Some("text_address"))
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    let archive = json!({
        "schema": format!("{PACT_FILE_SCHEMA}.semantic_archive.v1"),
        "root_store": root_store,
        "indexes": text_address_indexes,
    });
    let archive_bytes = serde_json::to_vec(&archive).map_err(|err| {
        PactError::codec(format!(
            "failed to encode semantic archive root store: {err}"
        ))
    })?;
    let compressed = compress_zstd_archive(&archive_bytes)?;
    let mut output = Vec::with_capacity(8 + compressed.len());
    output.extend_from_slice(b"PACTAR1\0");
    output.extend_from_slice(&compressed);
    Ok(output)
}

pub(super) fn semantic_archive_zstd_bytes(records: &[ScannedRecord]) -> PactResult<usize> {
    semantic_archive_payload_bytes(records).map(|bytes| bytes.len())
}

fn semantic_archive_projected_head_json(bytes: &[u8]) -> PactResult<String> {
    let root_store = semantic_archive_root_store(bytes)?;
    let branch_id = root_store.current_branch_id.clone();
    if !root_store.branches.contains_key(&branch_id) {
        return Err(PactError::not_found(format!(
            "archive current branch missing: {}",
            root_store.current_branch_id
        )));
    }
    let root_store_json = serde_json::to_string(&root_store).map_err(|err| {
        PactError::codec(format!(
            "failed to encode semantic archive root store: {err}"
        ))
    })?;
    crate::store::ledger_project_branch_tree_json_with_scope(
        &root_store_json,
        &branch_id,
        None,
        ProjectionTreeScope::Head,
    )
    .map_err(|err| PactError::codec(format!("failed to encode archive projected head: {err}")))
}

fn projected_heads_equal(left: &str, right: &str) -> PactResult<bool> {
    let left = canonical_projected_head_value(left)?;
    let right = canonical_projected_head_value(right)?;
    Ok(left == right)
}

fn canonical_projected_head_value(json: &str) -> PactResult<Value> {
    let value: Value = serde_json::from_str(json)
        .map_err(|err| PactError::codec(format!("invalid projected head JSON: {err}")))?;
    Ok(crate::tree::ComponentTree::from_value(&value)?.project_value())
}

fn semantic_archive_root_store(bytes: &[u8]) -> PactResult<ContextTreeLedger> {
    let payload = bytes
        .strip_prefix(b"PACTAR1\0")
        .ok_or_else(|| PactError::corrupt("not a PACT semantic archive payload"))?;
    let decoded = decompress_zstd_chunk(payload)?;
    let value: Value = serde_json::from_slice(&decoded)
        .map_err(|err| PactError::codec(format!("invalid semantic archive payload: {err}")))?;
    let root_value = value.get("root_store").cloned().unwrap_or(value);
    crate::store::reject_removed_patch_history(&root_value)?;
    let mut root_store: ContextTreeLedger = serde_json::from_value(root_value)
        .map_err(|err| PactError::codec(format!("invalid semantic archive root store: {err}")))?;
    root_store.content.rebuild_hash_index();
    Ok(root_store)
}

#[derive(Default)]
struct StorageProfileSlimmingStats {
    preview_bytes: usize,
    index_bytes: usize,
    duplicate_base_bytes: usize,
}

fn storage_profile_slimming_stats(records: &[ScannedRecord]) -> StorageProfileSlimmingStats {
    let mut stats = StorageProfileSlimmingStats::default();
    let root_store = ledger_from_records(records).ok();
    for record in records {
        if record.kind == "content" {
            stats.preview_bytes += record
                .payload
                .get("preview_inline")
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or_default();
        }
        if record.kind == "index" {
            stats.index_bytes += record.payload_bytes;
        }
    }
    if let Some(root_store) = root_store {
        stats.duplicate_base_bytes = omitted_duplicate_branch_base_bytes(&root_store);
    }
    stats
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageProfile {
    Hot,
    Compact,
    Archive,
}

impl StorageProfile {
    fn parse(value: &str) -> PactResult<Self> {
        match value {
            "hot" => Ok(Self::Hot),
            "compact" => Ok(Self::Compact),
            "archive" => Ok(Self::Archive),
            other => Err(PactError::invalid(format!(
                "unknown storage profile {other:?}; expected hot, compact, or archive"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Compact => "compact",
            Self::Archive => "archive",
        }
    }
}

fn ratio(left: usize, right: usize) -> f64 {
    if right == 0 {
        return 0.0;
    }
    left as f64 / right as f64
}

pub(super) fn gzip_len(bytes: &[u8]) -> PactResult<usize> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .map_err(|err| PactError::codec(format!("failed to gzip benchmark payload: {err}")))?;
    encoder
        .finish()
        .map(|bytes| bytes.len())
        .map_err(|err| PactError::codec(format!("failed to finish gzip benchmark payload: {err}")))
}

pub(super) fn expanded_record_jsonl_bytes(records: &[ScannedRecord]) -> Vec<u8> {
    let mut jsonl = Vec::new();
    for record in records {
        let line = json!({
            "kind": record.kind,
            "sequence": record.sequence,
            "payload": record.payload,
        });
        if let Ok(bytes) = serde_json::to_vec(&line) {
            jsonl.extend_from_slice(&bytes);
            jsonl.push(b'\n');
        }
    }
    jsonl
}

fn provider_thread_jsonl_bytes_from_tree(tree: &Value) -> Vec<u8> {
    let mut rows = Vec::new();
    collect_provider_thread_rows(tree, &mut rows);
    let mut jsonl = Vec::new();
    for row in rows {
        if let Ok(bytes) = serde_json::to_vec(&row) {
            jsonl.extend_from_slice(&bytes);
            jsonl.push(b'\n');
        }
    }
    jsonl
}

fn collect_provider_thread_rows(node: &Value, rows: &mut Vec<Value>) {
    let Some(object) = node.as_object() else {
        return;
    };
    let node_type = object
        .get("componentType")
        .or_else(|| object.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match node_type {
        "message_turn" => {
            let role = object
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("assistant");
            let content = object
                .get("children")
                .and_then(Value::as_array)
                .map(|children| {
                    children
                        .iter()
                        .filter_map(text_content_from_node)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            rows.push(json!({
                "role": role,
                "content": content,
            }));
        }
        "tool_call" | "tool_result" => {
            rows.push(node.clone());
        }
        _ => {}
    }
    if let Some(children) = object.get("children").and_then(Value::as_array) {
        for child in children {
            collect_provider_thread_rows(child, rows);
        }
    }
}

fn text_content_from_node(node: &Value) -> Option<String> {
    let object = node.as_object()?;
    let node_type = object
        .get("componentType")
        .or_else(|| object.get("type"))
        .and_then(Value::as_str)?;
    if node_type != "text_content" {
        return None;
    }
    object
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string)
}

struct StorageBenchTarget {
    profile: &'static str,
    max_file_bytes: usize,
    stretch_file_bytes: usize,
}

fn storage_bench_target(profile: &str) -> PactResult<StorageBenchTarget> {
    match profile {
        "medium-agentic" => Ok(StorageBenchTarget {
            profile: "medium-agentic",
            max_file_bytes: 150 * 1024,
            stretch_file_bytes: 100 * 1024,
        }),
        other => Err(PactError::invalid(format!(
            "unknown storage bench target profile: {other}"
        ))),
    }
}

fn storage_bench_violations(file_bytes: usize, target: &StorageBenchTarget) -> Vec<String> {
    let mut violations = Vec::new();
    if file_bytes > target.max_file_bytes {
        violations.push(format!(
            "file_bytes {file_bytes} exceeds {} byte target for {}",
            target.max_file_bytes, target.profile
        ));
    }
    violations
}

fn record_kind_info(records: &[ScannedRecord]) -> Value {
    let mut by_kind: BTreeMap<String, (usize, usize, usize, u64)> = BTreeMap::new();
    for record in records {
        let entry = by_kind
            .entry(record.kind.clone())
            .or_insert((0, 0, 0, record.sequence));
        entry.0 += 1;
        entry.1 += record.payload_bytes;
        if record.payload_bytes > entry.2 {
            entry.2 = record.payload_bytes;
            entry.3 = record.sequence;
        }
    }
    Value::Object(
        by_kind
            .into_iter()
            .map(|(kind, (count, bytes, max_bytes, max_sequence))| {
                (
                    kind,
                    json!({
                        "count": count,
                        "bytes": bytes,
                        "avg_bytes": avg_bytes(bytes, count),
                        "max_bytes": max_bytes,
                        "max_sequence": max_sequence,
                    }),
                )
            })
            .collect(),
    )
}

fn page_kind_info(manifest: &PactFileManifest) -> Value {
    let mut by_kind: BTreeMap<String, PageKindAccumulator> = BTreeMap::new();
    for page in &manifest.pages {
        let entry = by_kind.entry(page.page_type.clone()).or_default();
        entry.0 += 1;
        entry.1 += page.stored_payload_bytes;
        entry.2 += page.decoded_payload_bytes;
        entry.3 += page.record_count;
        *entry.4.entry(page.codec.clone()).or_default() += 1;
    }
    Value::Object(
        by_kind
            .into_iter()
            .map(
                |(
                    kind,
                    (count, stored_payload_bytes, decoded_payload_bytes, record_count, codecs),
                )| {
                    (
                        kind,
                        json!({
                            "count": count,
                            "payload_bytes": stored_payload_bytes,
                            "stored_payload_bytes": stored_payload_bytes,
                            "decoded_payload_bytes": decoded_payload_bytes,
                            "codecs": codecs,
                            "record_count": record_count,
                            "page_header_bytes": count * PAGE_HEADER_LEN,
                            "estimated_total_bytes": stored_payload_bytes + count * PAGE_HEADER_LEN,
                        }),
                    )
                },
            )
            .collect(),
    )
}

fn largest_record_info(records: &[ScannedRecord]) -> Value {
    let mut items = records
        .iter()
        .map(|record| {
            let (path, bytes) = largest_payload_path(&record.payload, &record.kind);
            json!({
                "sequence": record.sequence,
                "kind": record.kind,
                "payload_bytes": record.payload_bytes,
                "record_header_bytes": RECORD_HEADER_LEN,
                "largest_path": path,
                "largest_path_bytes": bytes,
                "id": record_identifier(record),
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .get("payload_bytes")
            .and_then(Value::as_u64)
            .cmp(&left.get("payload_bytes").and_then(Value::as_u64))
    });
    items.truncate(25);
    Value::Array(items)
}

fn record_identifier(record: &ScannedRecord) -> Option<String> {
    for key in [
        "id",
        "branch_id",
        "transaction_id",
        "component_id",
        "payload_id",
        "view_id",
        "content_id",
        "root_id",
        "name",
    ] {
        if let Some(value) = record.payload.get(key).and_then(Value::as_str) {
            return Some(value.to_string());
        }
    }
    None
}

fn largest_payload_path(value: &Value, root: &str) -> (String, usize) {
    let mut best = (
        root.to_string(),
        serde_json::to_vec(value).map_or(0, |bytes| bytes.len()),
    );
    collect_largest_payload_path(value, root, 0, &mut best);
    best
}

fn collect_largest_payload_path(
    value: &Value,
    path: &str,
    depth: usize,
    best: &mut (String, usize),
) {
    if depth >= 8 {
        return;
    }
    let bytes = serde_json::to_vec(value).map_or(0, |bytes| bytes.len());
    if bytes > best.1 {
        *best = (path.to_string(), bytes);
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                collect_largest_payload_path(child, &format!("{path}.{key}"), depth + 1, best);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_largest_payload_path(child, &format!("{path}[{index}]"), depth + 1, best);
            }
        }
        _ => {}
    }
}

fn content_storage_info(records: &[ScannedRecord]) -> Value {
    let mut content_count = 0usize;
    let mut chunk_count = 0usize;
    let mut manifest_count = 0usize;
    let mut manifest_chunk_refs = 0usize;
    let mut manifest_bytes = 0u64;
    let mut decoded_bytes = 0u64;
    let mut stored_chunk_bytes = 0u64;
    let mut decoded_chunk_text_bytes = 0u64;
    let mut codecs: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut hashes: BTreeMap<String, usize> = BTreeMap::new();
    let mut chunk_hashes: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut chunks_by_content: BTreeMap<String, usize> = BTreeMap::new();
    let mut previews = Vec::new();
    for record in records {
        match record.kind.as_str() {
            "content" => {
                content_count += 1;
                let content_id = record
                    .payload
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                decoded_bytes += record
                    .payload
                    .get("bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                if let Some(preview_inline) =
                    record.payload.get("preview_inline").and_then(Value::as_str)
                {
                    previews.push(json!({
                        "content_id": content_id,
                        "preview_inline": preview_inline,
                    }));
                }
                if let Some(hash) = record.payload.get("hash").and_then(Value::as_str) {
                    *hashes.entry(hash.to_string()).or_default() += 1;
                }
            }
            "content_chunk" => {
                chunk_count += 1;
                let codec = record
                    .payload
                    .get("codec")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let bytes = record
                    .payload
                    .get("bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                stored_chunk_bytes += bytes;
                if let Some(content_id) = record.payload.get("content_id").and_then(Value::as_str) {
                    *chunks_by_content.entry(content_id.to_string()).or_default() += 1;
                }
                if let Some(hash) = record.payload.get("hash").and_then(Value::as_str) {
                    let entry = chunk_hashes.entry(hash.to_string()).or_default();
                    entry.0 += 1;
                    entry.1 = bytes;
                }
                decoded_chunk_text_bytes += record
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| text.len() as u64)
                    .unwrap_or_default();
                let entry = codecs.entry(codec).or_default();
                entry.0 += 1;
                entry.1 += bytes;
            }
            "content_manifest" => {
                manifest_count += 1;
                manifest_bytes += record
                    .payload
                    .get("bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                manifest_chunk_refs += record
                    .payload
                    .get("chunks")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or_default();
            }
            "index"
                if record.payload.get("name").and_then(Value::as_str)
                    == Some("content_preview") =>
            {
                if let Some(entries) = record.payload.get("entries").and_then(Value::as_object) {
                    for value in entries.values() {
                        if let (Some(content_id), Some(preview)) = (
                            value.get("content_id").and_then(Value::as_str),
                            value.get("preview").and_then(Value::as_str),
                        ) {
                            previews.push(json!({
                                "content_id": content_id,
                                "preview": preview,
                                "source": "content_preview_index",
                            }));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let duplicate_hash_count = hashes.values().filter(|count| **count > 1).count();
    let duplicate_chunk_hash_count = chunk_hashes
        .values()
        .filter(|(count, _)| *count > 1)
        .count();
    let repeated_chunk_bytes = chunk_hashes
        .values()
        .filter(|(count, _)| *count > 1)
        .map(|(count, bytes)| (*count as u64).saturating_sub(1) * *bytes)
        .sum::<u64>();
    previews.truncate(10);
    json!({
        "content_count": content_count,
        "chunk_count": chunk_count,
        "manifest_count": manifest_count,
        "manifest_chunk_refs": manifest_chunk_refs,
        "manifest_bytes": manifest_bytes,
        "decoded_bytes": decoded_bytes,
        "stored_chunk_bytes": stored_chunk_bytes,
        "decoded_chunk_text_bytes": decoded_chunk_text_bytes,
        "duplicate_hash_count": duplicate_hash_count,
        "duplicate_chunk_hash_count": duplicate_chunk_hash_count,
        "estimated_reusable_chunk_bytes": repeated_chunk_bytes,
        "max_chunks_per_content": chunks_by_content.values().copied().max().unwrap_or_default(),
        "previews": previews,
        "codecs": codecs
            .into_iter()
            .map(|(codec, (count, bytes))| {
                (
                    codec,
                    json!({
                        "count": count,
                        "bytes": bytes,
                        "avg_bytes": if count == 0 { 0 } else { bytes / count as u64 },
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>(),
    })
}

fn component_payload_kind_info(records: &[ScannedRecord]) -> Value {
    let mut by_kind: BTreeMap<String, (usize, usize, usize, String)> = BTreeMap::new();
    for record in records
        .iter()
        .filter(|record| record.kind == "component_payload")
    {
        let payload_kind = record
            .payload
            .get("payload")
            .and_then(|payload| payload.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let payload_id = record
            .payload
            .get("payload_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let entry = by_kind.entry(payload_kind).or_default();
        entry.0 += 1;
        entry.1 += record.payload_bytes;
        if record.payload_bytes > entry.2 {
            entry.2 = record.payload_bytes;
            entry.3 = payload_id;
        }
    }
    Value::Object(
        by_kind
            .into_iter()
            .map(|(kind, (count, bytes, max_bytes, max_payload_id))| {
                (
                    kind,
                    json!({
                        "count": count,
                        "bytes": bytes,
                        "avg_bytes": avg_bytes(bytes, count),
                        "max_bytes": max_bytes,
                        "max_payload_id": max_payload_id,
                    }),
                )
            })
            .collect(),
    )
}

fn field_size_info(records: &[ScannedRecord]) -> Value {
    let mut field_bytes: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for record in records {
        collect_field_sizes(&record.payload, &record.kind, 0, &mut field_bytes);
    }
    let mut top = field_bytes
        .into_iter()
        .map(|(field, (count, bytes))| {
            json!({
                "field": field,
                "count": count,
                "bytes": bytes,
                "avg_bytes": avg_bytes(bytes, count),
            })
        })
        .collect::<Vec<_>>();
    top.sort_by(|left, right| {
        right
            .get("bytes")
            .and_then(Value::as_u64)
            .cmp(&left.get("bytes").and_then(Value::as_u64))
    });
    top.truncate(40);
    json!({ "top": top })
}

fn hot_field_info(records: &[ScannedRecord]) -> Value {
    let mut hot = HotFieldStats::default();
    for record in records {
        collect_hot_field_sizes(&record.payload, &record.kind, 0, &mut hot);
    }
    json!({
        "attrs": {
            "count": hot.attrs.0,
            "bytes": hot.attrs.1,
        },
        "org": {
            "count": hot.org.0,
            "bytes": hot.org.1,
        },
        "aux": {
            "count": hot.aux.0,
            "bytes": hot.aux.1,
        },
    })
}

#[derive(Default)]
struct HotFieldStats {
    attrs: (usize, usize),
    org: (usize, usize),
    aux: (usize, usize),
}

fn collect_hot_field_sizes(value: &Value, path: &str, depth: usize, out: &mut HotFieldStats) {
    if depth >= 8 {
        return;
    }
    let Value::Object(object) = value else {
        if let Value::Array(items) = value {
            for item in items {
                collect_hot_field_sizes(item, path, depth + 1, out);
            }
        }
        return;
    };
    for (key, child) in object {
        let bytes = serde_json::to_vec(child).map_or(0, |bytes| bytes.len());
        match key.as_str() {
            "attrs" => {
                out.attrs.0 += 1;
                out.attrs.1 += bytes;
            }
            "org" => {
                out.org.0 += 1;
                out.org.1 += bytes;
            }
            "aux" => {
                out.aux.0 += 1;
                out.aux.1 += bytes;
            }
            _ => {}
        }
        collect_hot_field_sizes(child, &format!("{path}.{key}"), depth + 1, out);
    }
}

fn payload_slot_info(records: &[ScannedRecord]) -> Value {
    let mut declared_policy_count = 0usize;
    let mut content_ref_count = 0usize;
    let mut external_ref_count = 0usize;
    let mut inline_payload_count = 0usize;
    let mut oversized_inline_count = 0usize;
    let mut preview_inline_count = 0usize;
    let mut inline_bytes_by_slot = BTreeMap::<String, usize>::new();

    for record in records {
        collect_payload_slot_info(
            &record.payload,
            &mut declared_policy_count,
            &mut content_ref_count,
            &mut external_ref_count,
            &mut inline_payload_count,
            &mut oversized_inline_count,
            &mut preview_inline_count,
            &mut inline_bytes_by_slot,
        );
    }

    json!({
        "declared_policy_count": declared_policy_count,
        "content_ref_count": content_ref_count,
        "external_ref_count": external_ref_count,
        "inline_payload_count": inline_payload_count,
        "oversized_inline_count": oversized_inline_count,
        "preview_inline_count": preview_inline_count,
        "inline_bytes_by_slot": inline_bytes_by_slot,
        "inline_limit_bytes": crate::components::DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES,
        "preview_limit_bytes": crate::components::DEFAULT_PREVIEW_INLINE_LIMIT_BYTES,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_payload_slot_info(
    value: &Value,
    declared_policy_count: &mut usize,
    content_ref_count: &mut usize,
    external_ref_count: &mut usize,
    inline_payload_count: &mut usize,
    oversized_inline_count: &mut usize,
    preview_inline_count: &mut usize,
    inline_bytes_by_slot: &mut BTreeMap<String, usize>,
) {
    match value {
        Value::Object(object) => {
            if object.contains_key("payload_slot_policy") {
                *declared_policy_count += 1;
            }
            if object.contains_key("content_ref") {
                *content_ref_count += 1;
            }
            if object.contains_key("external_ref") {
                *external_ref_count += 1;
            }
            if object.contains_key("preview_inline") {
                *preview_inline_count += 1;
            }
            for (key, child) in object {
                if matches!(key.as_str(), "content" | "text" | "result" | "data")
                    && child.as_str().is_some()
                {
                    *inline_payload_count += 1;
                    *inline_bytes_by_slot.entry(key.clone()).or_default() +=
                        child.as_str().unwrap_or_default().len();
                    if child.as_str().unwrap_or_default().len()
                        > crate::components::DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES
                    {
                        *oversized_inline_count += 1;
                    }
                } else if matches!(key.as_str(), "parameters" | "result")
                    && (child.is_object() || child.is_array())
                {
                    let bytes = serde_json::to_vec(child).map_or(0, |bytes| bytes.len());
                    *inline_payload_count += 1;
                    *inline_bytes_by_slot.entry(key.clone()).or_default() += bytes;
                    if bytes > crate::components::DEFAULT_PAYLOAD_INLINE_LIMIT_BYTES {
                        *oversized_inline_count += 1;
                    }
                }
                collect_payload_slot_info(
                    child,
                    declared_policy_count,
                    content_ref_count,
                    external_ref_count,
                    inline_payload_count,
                    oversized_inline_count,
                    preview_inline_count,
                    inline_bytes_by_slot,
                );
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_payload_slot_info(
                    item,
                    declared_policy_count,
                    content_ref_count,
                    external_ref_count,
                    inline_payload_count,
                    oversized_inline_count,
                    preview_inline_count,
                    inline_bytes_by_slot,
                );
            }
        }
        _ => {}
    }
}

fn trace_payload_info(records: &[ScannedRecord]) -> Value {
    let mut count = 0usize;
    let mut decoded_bytes = 0usize;
    let mut max_bytes = 0usize;
    let mut relative_ref_savings_bytes = 0usize;
    let mut top_paths = Vec::new();
    for record in records.iter().filter(|record| record.kind == "trace") {
        count += 1;
        decoded_bytes += record.payload_bytes;
        max_bytes = max_bytes.max(record.payload_bytes);
        relative_ref_savings_bytes += estimate_trace_relative_ref_savings(&record.payload);
        let (path, bytes) = largest_payload_path(&record.payload, "trace");
        top_paths.push(json!({
            "sequence": record.sequence,
            "bytes": record.payload_bytes,
            "largest_path": path,
            "largest_path_bytes": bytes,
        }));
    }
    top_paths.sort_by(|left, right| {
        right
            .get("bytes")
            .and_then(Value::as_u64)
            .cmp(&left.get("bytes").and_then(Value::as_u64))
    });
    top_paths.truncate(10);
    json!({
        "count": count,
        "decoded_bytes": decoded_bytes,
        "max_bytes": max_bytes,
        "avg_bytes": avg_bytes(decoded_bytes, count),
        "relative_ref_savings_bytes": relative_ref_savings_bytes,
        "top": top_paths,
    })
}

fn estimate_trace_relative_ref_savings(value: &Value) -> usize {
    let Some(object) = value.as_object() else {
        return 0;
    };
    let Some(root_id) = object.get("root_id").and_then(Value::as_str) else {
        return 0;
    };
    let Some(branch_id) = object.get("branch_id").and_then(Value::as_str) else {
        return 0;
    };
    let mut saved = 0usize;
    for key in TRACE_BOUNDARY_REF_KEYS {
        if let Some(ref_value) = object.get(*key) {
            saved += estimate_trace_ref_savings(ref_value, root_id, branch_id);
        }
    }
    if let Some(ref_value) = object
        .get("commit")
        .and_then(Value::as_object)
        .and_then(|commit| commit.get("pact_ref"))
    {
        saved += estimate_trace_ref_savings(ref_value, root_id, branch_id);
    }
    saved
}

fn estimate_trace_ref_savings(value: &Value, root_id: &str, branch_id: &str) -> usize {
    let full = serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0);
    let compact = compact_trace_ref_value(value.clone(), root_id, branch_id, None);
    let compact_len = serde_json::to_vec(&compact)
        .map(|bytes| bytes.len())
        .unwrap_or(full);
    full.saturating_sub(compact_len)
}

fn live_shape_info(path: &Path, scanned: &ScannedFile) -> Value {
    let commit_record_count = scanned.manifest.commit_count;
    let head_record_count = scanned.manifest.head_count;
    let branch_snapshot_record_bytes = sum_record_payload_bytes(&scanned.records, "branch");
    let branch_current_snapshot_bytes =
        sum_field_payload_bytes(&scanned.records, "branch", &["current", "components"]);
    let head_record_bytes = sum_record_payload_bytes(&scanned.records, "head");
    let commit_record_bytes = sum_record_payload_bytes(&scanned.records, "commit");
    let trace_decoded_bytes = sum_record_payload_bytes(&scanned.records, "trace");
    let record_page_stored_bytes = sum_page_stored_bytes(&scanned.manifest, "record");
    let record_page_decoded_bytes = sum_page_decoded_bytes(&scanned.manifest, "record");
    let trace_stored_bytes_estimate = proportional_bytes(
        trace_decoded_bytes,
        record_page_stored_bytes,
        record_page_decoded_bytes,
    );
    let content_metadata_bytes = sum_record_payload_bytes(&scanned.records, "content")
        + sum_record_payload_bytes(&scanned.records, "content_manifest");
    let content_body_record_bytes = sum_record_payload_bytes(&scanned.records, "content_chunk")
        + sum_field_payload_bytes(&scanned.records, "content", &["text"]);
    let content_page_stored_bytes = sum_page_stored_bytes(&scanned.manifest, "content");
    let current_head_bytes = latest_projected_cycle_bytes(&scanned.records).unwrap_or_default();
    let max_provider_visible_head_bytes = current_head_bytes;
    let shape = if commit_record_count > 0 && head_record_count > 0 {
        if branch_current_snapshot_bytes > commit_record_bytes.saturating_mul(2).max(1) {
            "mixed-snapshot-and-commit-head"
        } else {
            "commit-head-shaped"
        }
    } else if branch_snapshot_record_bytes > 0 {
        "snapshot-shaped"
    } else if scanned.manifest.record_count == 0 {
        "empty"
    } else {
        "record-shaped-without-head"
    };

    json!({
        "shape": shape,
        "commit_record_count": commit_record_count,
        "head_record_count": head_record_count,
        "branch_snapshot_record_bytes": branch_snapshot_record_bytes,
        "branch_current_snapshot_bytes": branch_current_snapshot_bytes,
        "commit_record_bytes": commit_record_bytes,
        "head_record_bytes": head_record_bytes,
        "current_head_bytes": current_head_bytes,
        "max_provider_visible_head_bytes": max_provider_visible_head_bytes,
        "trace_decoded_bytes": trace_decoded_bytes,
        "trace_stored_bytes_estimate": trace_stored_bytes_estimate,
        "content_metadata_bytes": content_metadata_bytes,
        "content_body_record_bytes": content_body_record_bytes,
        "content_page_stored_bytes": content_page_stored_bytes,
        "anchor_policy": {
            "default_freeze_cycle_interval": DEFAULT_FREEZE_CYCLE_INTERVAL,
            "default_freeze_commit_bytes": Option::<usize>::None,
            "available_freeze_commit_bytes_threshold": DEFAULT_FREEZE_COMMIT_BYTES_THRESHOLD,
            "current_frozen_cycle_count": scanned.manifest.frozen_cycle_count,
        },
        "fast_head": {
            "available": head_record_count > 0 && scanned.manifest.latest_footer.is_some(),
            "footer_last_commit_offset": scanned
                .manifest
                .latest_footer
                .as_ref()
                .map(|footer| footer.last_commit_offset)
                .unwrap_or_default(),
        },
        "source_path": path.display().to_string(),
    })
}

fn sum_record_payload_bytes(records: &[ScannedRecord], kind: &str) -> usize {
    records
        .iter()
        .filter(|record| record.kind == kind)
        .map(|record| record.payload_bytes)
        .sum()
}

fn sum_field_payload_bytes(records: &[ScannedRecord], kind: &str, fields: &[&str]) -> usize {
    records
        .iter()
        .filter(|record| record.kind == kind)
        .map(|record| {
            fields
                .iter()
                .filter_map(|field| record.payload.get(*field))
                .filter_map(|value| serde_json::to_vec(value).ok())
                .map(|bytes| bytes.len())
                .sum::<usize>()
        })
        .sum()
}

fn sum_page_stored_bytes(manifest: &PactFileManifest, page_type: &str) -> usize {
    manifest
        .pages
        .iter()
        .filter(|page| page.page_type == page_type)
        .map(|page| page.stored_payload_bytes)
        .sum()
}

fn sum_page_decoded_bytes(manifest: &PactFileManifest, page_type: &str) -> usize {
    manifest
        .pages
        .iter()
        .filter(|page| page.page_type == page_type)
        .map(|page| page.decoded_payload_bytes)
        .sum()
}

fn proportional_bytes(part_decoded: usize, total_stored: usize, total_decoded: usize) -> usize {
    if part_decoded == 0 || total_stored == 0 || total_decoded == 0 {
        return 0;
    }
    ((part_decoded as u128 * total_stored as u128) / total_decoded as u128) as usize
}

fn latest_projected_cycle_bytes(records: &[ScannedRecord]) -> PactResult<usize> {
    let root_store = super::history::ledger_from_records(records)?;
    let branch = root_store
        .branches
        .get(&root_store.current_branch_id)
        .ok_or_else(|| {
            PactError::not_found(format!(
                "missing current branch: {}",
                root_store.current_branch_id
            ))
        })?;
    let root = crate::store::project_branch_component_tree(
        &root_store,
        branch,
        Some(branch.head_revision),
    )?
    .project_value();
    serde_json::to_vec(&root)
        .map(|bytes| bytes.len())
        .map_err(|err| PactError::codec(format!("failed to encode projected head: {err}")))
}

fn collect_field_sizes(
    value: &Value,
    path: &str,
    depth: usize,
    out: &mut BTreeMap<String, (usize, usize)>,
) {
    if depth >= 5 {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let next_path = format!("{path}.{key}");
                let bytes = key.len() + serde_json::to_vec(child).map(|v| v.len()).unwrap_or(0);
                let entry = out.entry(next_path.clone()).or_default();
                entry.0 += 1;
                entry.1 += bytes;
                collect_field_sizes(child, &next_path, depth + 1, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_field_sizes(child, path, depth + 1, out);
            }
        }
        _ => {}
    }
}

fn semantic_count_info(records: &[ScannedRecord], manifest: &PactFileManifest) -> Value {
    let mut user_messages = 0usize;
    let mut assistant_responses = 0usize;
    let mut system_messages = 0usize;
    let mut tool_calls = 0usize;
    let mut tool_results = 0usize;
    let mut model_or_provider_calls = 0usize;
    let mut total_latency_ms = 0f64;
    let mut latency_samples = 0usize;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut total_tokens = 0u64;
    let mut token_samples = 0usize;

    for record in records {
        match record.kind.as_str() {
            "node" => {
                let role = record.payload.get("role").and_then(Value::as_str);
                let node_kind = record
                    .payload
                    .get("kind")
                    .or_else(|| record.payload.get("componentType"))
                    .or_else(|| record.payload.get("node_type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match role {
                    Some("user") => user_messages += 1,
                    Some("assistant") => assistant_responses += 1,
                    Some("system") => system_messages += 1,
                    _ => {}
                }
                if node_kind.contains("tool_call") || role == Some("tool_call") {
                    tool_calls += 1;
                }
                if node_kind.contains("tool_result") || role == Some("tool_result") {
                    tool_results += 1;
                }
            }
            "trace" => {
                if trace_looks_like_provider_call(&record.payload) {
                    model_or_provider_calls += 1;
                }
                if let Some(ms) = trace_latency_ms(&record.payload) {
                    total_latency_ms += ms;
                    latency_samples += 1;
                }
                let tokens = trace_token_counts(&record.payload);
                if tokens.3 {
                    input_tokens += tokens.0;
                    output_tokens += tokens.1;
                    total_tokens += tokens.2;
                    token_samples += 1;
                }
            }
            _ => {}
        }
    }

    json!({
        "cycles": manifest.cycle_count,
        "user_messages": user_messages,
        "assistant_responses": assistant_responses,
        "response_count": assistant_responses,
        "system_messages": system_messages,
        "tool_calls": tool_calls,
        "tool_results": tool_results,
        "tool_messages": tool_calls + tool_results,
        "traces": manifest.trace_count,
        "model_or_provider_calls": model_or_provider_calls,
        "latency_samples": latency_samples,
        "total_latency_ms": total_latency_ms,
        "token_samples": token_samples,
        "tokens": {
            "input": input_tokens,
            "output": output_tokens,
            "total": total_tokens,
        },
    })
}

fn trace_looks_like_provider_call(value: &Value) -> bool {
    let haystack = serde_json::to_string(value)
        .unwrap_or_default()
        .to_lowercase();
    haystack.contains("provider") || haystack.contains("model")
}

fn trace_latency_ms(value: &Value) -> Option<f64> {
    match value {
        Value::Object(object) => {
            for key in [
                "latency_ms",
                "duration_ms",
                "elapsed_ms",
                "wall_time_ms",
                "total_latency_ms",
            ] {
                if let Some(ms) = object.get(key).and_then(Value::as_f64) {
                    return Some(ms);
                }
            }
            object.values().find_map(trace_latency_ms)
        }
        Value::Array(items) => items.iter().find_map(trace_latency_ms),
        _ => None,
    }
}

fn trace_token_counts(value: &Value) -> (u64, u64, u64, bool) {
    match value {
        Value::Object(object) => {
            let mut input = object
                .get("input_tokens")
                .or_else(|| object.get("prompt_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let mut output = object
                .get("output_tokens")
                .or_else(|| object.get("completion_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let mut total = object
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let mut found = input > 0 || output > 0 || total > 0;
            for child in object.values() {
                let child_tokens = trace_token_counts(child);
                input += child_tokens.0;
                output += child_tokens.1;
                total += child_tokens.2;
                found |= child_tokens.3;
            }
            if total == 0 && (input > 0 || output > 0) {
                total = input + output;
            }
            (input, output, total, found)
        }
        Value::Array(items) => {
            let mut input = 0;
            let mut output = 0;
            let mut total = 0;
            let mut found = false;
            for child in items {
                let child_tokens = trace_token_counts(child);
                input += child_tokens.0;
                output += child_tokens.1;
                total += child_tokens.2;
                found |= child_tokens.3;
            }
            (input, output, total, found)
        }
        _ => (0, 0, 0, false),
    }
}

fn ref_info(records: &[ScannedRecord]) -> Value {
    let content_by_id = content_payloads_by_id(records);
    let resolvable_ref_ids = records
        .iter()
        .filter_map(|record| match record.kind.as_str() {
            "content" => record.payload.get("id").and_then(Value::as_str),
            "component_payload" => record.payload.get("payload_id").and_then(Value::as_str),
            _ => None,
        })
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let mut stats = RefStats::default();
    for record in records {
        collect_ref_stats(&record.payload, &mut stats);
    }
    let missing_content_refs = stats
        .content_refs
        .iter()
        .filter(|content_ref| !resolvable_ref_ids.contains(*content_ref))
        .cloned()
        .collect::<BTreeSet<_>>();
    let preview_inline_ref_count = stats
        .content_refs
        .iter()
        .filter(|content_ref| content_preview_inline_by_id(&content_by_id, content_ref).is_some())
        .count();
    let resolved_content_ref_count = stats
        .content_refs
        .len()
        .saturating_sub(missing_content_refs.len());
    let estimated_repeated_string_bytes = stats
        .repeated_strings
        .iter()
        .filter(|(value, count)| **count > 1 && value.len() >= 8)
        .map(|(value, count)| (count - 1) * value.len())
        .sum::<usize>();
    let mut top_repeated = stats
        .repeated_strings
        .into_iter()
        .filter(|(value, count)| *count > 1 && value.len() >= 8)
        .map(|(value, count)| {
            json!({
                "value": value,
                "count": count,
                "estimated_duplicate_bytes": (count - 1) * value.len(),
            })
        })
        .collect::<Vec<_>>();
    top_repeated.sort_by(|left, right| {
        right
            .get("estimated_duplicate_bytes")
            .and_then(Value::as_u64)
            .cmp(
                &left
                    .get("estimated_duplicate_bytes")
                    .and_then(Value::as_u64),
            )
    });
    top_repeated.truncate(20);
    json!({
        "content_ref_count": stats.content_refs.len(),
        "resolved_content_ref_count": resolved_content_ref_count,
        "preview_inline_ref_count": preview_inline_ref_count,
        "missing_content_ref_count": missing_content_refs.len(),
        "missing_content_refs": missing_content_refs.into_iter().collect::<Vec<_>>(),
        "external_ref_count": stats.external_ref_count,
        "inline_ref_count": stats.inline_ref_count,
        "long_id_like_string_count": stats.long_id_count,
        "pact_ref_vector_count": stats.pact_ref_vector_count,
        "estimated_pact_ref_key_bytes_saved": stats.estimated_pact_ref_key_bytes_saved,
        "estimated_repeated_string_bytes": estimated_repeated_string_bytes,
        "top_repeated_strings": top_repeated,
    })
}

pub(super) fn symbol_info(records: &[ScannedRecord]) -> Value {
    let mut record_count = 0_u64;
    let mut dynamic_symbol_count = 0_u64;
    let mut substituted_occurrences = 0_u64;
    let mut estimated_saved_bytes = 0_u64;
    let mut dictionary_bytes = 0_u64;
    let mut file_local_id_count = 0_u64;
    let mut file_local_id_kinds = BTreeMap::<String, u64>::new();
    let mut top = Vec::new();
    let string_counts = string_counts_from_records(records);
    for record in records.iter().filter(|record| record.kind == "symbol") {
        record_count += 1;
        dictionary_bytes += serde_json::to_vec(&record.payload)
            .map(|bytes| bytes.len() as u64)
            .unwrap_or_default();
        if let Some(values) = record.payload.get("values").and_then(Value::as_array) {
            let occurrence_values = record
                .payload
                .get("occurrences")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let saved_values = record
                .payload
                .get("estimated_saved_bytes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(ids) = record
                .payload
                .get("file_local_ids")
                .and_then(Value::as_array)
            {
                file_local_id_count += ids.len() as u64;
                let kinds = record
                    .payload
                    .get("file_local_id_kinds")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for kind in kinds.iter().filter_map(Value::as_str) {
                    *file_local_id_kinds.entry(kind.to_string()).or_default() += 1;
                }
            }
            for (index, value) in values.iter().filter_map(Value::as_str).enumerate() {
                dynamic_symbol_count += 1;
                let occurrences = occurrence_values
                    .get(index)
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| {
                        string_counts.get(value).copied().unwrap_or_default() as u64
                    });
                let saved = saved_values
                    .get(index)
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| {
                        estimate_symbol_saved_bytes(value, occurrences as usize) as u64
                    });
                substituted_occurrences += occurrences;
                estimated_saved_bytes += saved;
                top.push(json!({
                    "id": BUILTIN_SYMBOLS.len() + index,
                    "value": value,
                    "occurrences": occurrences,
                    "estimated_saved_bytes": saved,
                }));
            }
            continue;
        }

        let value = match record.payload.get("value").and_then(Value::as_str) {
            Some(value) => value,
            None => continue,
        };
        dynamic_symbol_count += 1;
        let occurrences = record
            .payload
            .get("occurrences")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| string_counts.get(value).copied().unwrap_or_default() as u64);
        let saved = record
            .payload
            .get("estimated_saved_bytes")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| estimate_symbol_saved_bytes(value, occurrences as usize) as u64);
        substituted_occurrences += occurrences;
        estimated_saved_bytes += saved;
        top.push(json!({
            "id": record.payload.get("id").cloned().unwrap_or(Value::Null),
            "value": value,
            "occurrences": occurrences,
            "estimated_saved_bytes": saved,
        }));
    }
    top.sort_by(|left, right| {
        right
            .get("estimated_saved_bytes")
            .and_then(Value::as_u64)
            .cmp(&left.get("estimated_saved_bytes").and_then(Value::as_u64))
    });
    top.truncate(25);
    json!({
        "symbol_count": dynamic_symbol_count,
        "symbol_record_count": record_count,
        "dynamic_symbol_count": dynamic_symbol_count,
        "dictionary_bytes": dictionary_bytes,
        "substituted_occurrence_count": substituted_occurrences,
        "estimated_saved_bytes": estimated_saved_bytes,
        "file_local_id_count": file_local_id_count,
        "file_local_id_kinds": file_local_id_kinds,
        "top_symbols": top,
    })
}

fn collect_ref_stats(value: &Value, stats: &mut RefStats) {
    match value {
        Value::Object(object) => {
            if object_looks_like_pact_ref(object) {
                stats.pact_ref_vector_count += 1;
                stats.estimated_pact_ref_key_bytes_saved +=
                    object.keys().map(|key| key.len()).sum::<usize>();
            }
            if let Some(content_ref) = ref_id_from_content_ref_value(object.get("content_ref")) {
                stats.content_refs.push(content_ref.to_string());
            }
            for (key, child) in object {
                if key == "external_ref" || key == "external_refs" {
                    stats.external_ref_count += 1;
                }
                if key == "inline" {
                    stats.inline_ref_count += 1;
                }
                if (key.ends_with("_id") || key.ends_with("_ref") || key == "id")
                    && child.as_str().is_some_and(|text| text.len() > 12)
                {
                    stats.long_id_count += 1;
                }
                collect_ref_stats(child, stats);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_ref_stats(child, stats);
            }
        }
        Value::String(text) if text.len() >= 8 && text.len() <= 80 => {
            *stats.repeated_strings.entry(text.clone()).or_default() += 1;
        }
        _ => {}
    }
}

fn pact_bundle_info(path: &Path, _scanned: &ScannedFile) -> Value {
    json!({
        "path": path.display().to_string(),
        "exists": false,
        "bytes": 0,
        "item_count": 0,
        "inline_item_count": 0,
        "required_item_count": 0,
        "missing": [],
        "extras": [],
    })
}

fn storage_smells(scanned: &ScannedFile, _bundle: &Value, fields: &Value) -> Value {
    let mut warnings = Vec::new();
    let mut branch_current_bytes = 0usize;
    let ledger_audit = ledger_remediation_audit(&scanned.records);

    for record in &scanned.records {
        if record.kind == "branch"
            && let Some(current) = record.payload.get("current")
        {
            branch_current_bytes += serde_json::to_vec(current).map(|v| v.len()).unwrap_or(0);
        }
    }

    if scanned.manifest.symbol_count == 0 && scanned.manifest.record_count > 20 {
        warnings.push(json!({
            "code": "symbols_inactive",
            "message": "no symbol records are present; repeated canonical keys are still encoded as strings",
        }));
    }
    if branch_current_bytes > 256 * 1024 {
        warnings.push(json!({
            "code": "large_branch_current",
            "message": "branch current projected state is large",
            "bytes": branch_current_bytes,
        }));
    }
    if ledger_audit["full_component_replacements"]["count"]
        .as_u64()
        .unwrap_or_default()
        > 0
    {
        warnings.push(json!({
            "code": "full_component_replacements",
            "message": "commit records replace whole nodes; canonical ledger should prefer per-node deltas",
            "count": ledger_audit["full_component_replacements"]["count"],
            "encoded_payload_bytes": ledger_audit["full_component_replacements"]["encoded_payload_bytes"],
            "expanded_json_bytes": ledger_audit["full_component_replacements"]["expanded_json_bytes"],
        }));
    }
    json!({
        "warnings": warnings,
        "branch_current_bytes": branch_current_bytes,
        "ledger_remediation": ledger_audit,
        "top_field_bytes": fields.get("top").cloned().unwrap_or(Value::Array(vec![])),
    })
}

fn ledger_remediation_audit(records: &[ScannedRecord]) -> Value {
    let node_payloads = node_payload_audit_by_id(records);
    let mut replacement_count = 0usize;
    let mut replacement_missing_refs = 0usize;
    let mut replacement_encoded_bytes = 0usize;
    let mut replacement_expanded_bytes = 0usize;
    let mut replacement_content_bytes = 0usize;
    let mut insert_count = 0usize;
    let mut insert_encoded_bytes = 0usize;
    let mut insert_expanded_bytes = 0usize;
    let mut insert_content_bytes = 0usize;
    let mut delete_count = 0usize;
    let mut set_attr_count = 0usize;
    let mut unset_attr_count = 0usize;
    let mut attach_content_ref_count = 0usize;
    let mut replace_content_range_count = 0usize;
    let mut append_components_count = 0usize;
    let mut by_type: BTreeMap<String, TypeByteTotals> = BTreeMap::new();
    let mut top_replacements = Vec::new();

    let mut verbose_trace_bytes = 0usize;
    let mut verbose_trace_count = 0usize;
    let mut derived_record_bytes = 0usize;
    let mut derived_by_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for record in records {
        match record.kind.as_str() {
            "trace" => {
                verbose_trace_count += 1;
                verbose_trace_bytes += record.payload_bytes;
            }
            kind @ ("head" | "projection" | "component" | "components") => {
                derived_record_bytes += record.payload_bytes;
                let entry = derived_by_kind.entry(kind.to_string()).or_default();
                entry.0 += 1;
                entry.1 += record.payload_bytes;
            }
            _ => {}
        }
    }

    for record in records.iter().filter(|record| record.kind == "commit") {
        let Ok(commit) = serde_json::from_value::<CommitRecord>(record.payload.clone()) else {
            continue;
        };
        for op in commit.ops {
            match op {
                CommitRecordOp::ReplaceComponent {
                    component_id,
                    node_ref,
                } => {
                    replacement_count += 1;
                    if let Some(node) =
                        latest_node_payload_before(&node_payloads, &node_ref, record.sequence)
                            .or_else(|| {
                                latest_node_payload_before(
                                    &node_payloads,
                                    &component_id,
                                    record.sequence,
                                )
                            })
                    {
                        replacement_encoded_bytes += node.encoded_payload_bytes;
                        replacement_expanded_bytes += node.expanded_json_bytes;
                        replacement_content_bytes += node.content_bytes;
                        let totals = by_type.entry(node.node_type.clone()).or_default();
                        totals.count += 1;
                        totals.encoded_payload_bytes += node.encoded_payload_bytes;
                        totals.expanded_json_bytes += node.expanded_json_bytes;
                        totals.content_bytes += node.content_bytes;
                        top_replacements.push(json!({
                            "component_id": component_id,
                            "node_ref": node_ref,
                            "node_type": node.node_type.clone(),
                            "encoded_payload_bytes": node.encoded_payload_bytes,
                            "expanded_json_bytes": node.expanded_json_bytes,
                            "content_bytes": node.content_bytes,
                        }));
                    } else {
                        replacement_missing_refs += 1;
                    }
                }
                CommitRecordOp::InsertComponent { node_ref, .. } => {
                    insert_count += 1;
                    if let Some(node) =
                        latest_node_payload_before(&node_payloads, &node_ref, record.sequence)
                    {
                        insert_encoded_bytes += node.encoded_payload_bytes;
                        insert_expanded_bytes += node.expanded_json_bytes;
                        insert_content_bytes += node.content_bytes;
                    }
                }
                CommitRecordOp::AppendComponents { node_refs, .. } => {
                    append_components_count += node_refs.len();
                    for node_ref in node_refs {
                        if let Some(node) =
                            latest_node_payload_before(&node_payloads, &node_ref, record.sequence)
                        {
                            insert_encoded_bytes += node.encoded_payload_bytes;
                            insert_expanded_bytes += node.expanded_json_bytes;
                            insert_content_bytes += node.content_bytes;
                        }
                    }
                }
                CommitRecordOp::DeleteComponent { .. } => delete_count += 1,
                CommitRecordOp::SetComponentAux { .. } => set_attr_count += 1,
                CommitRecordOp::UnsetComponentAux { .. } => unset_attr_count += 1,
                CommitRecordOp::AttachContentRef { .. } => attach_content_ref_count += 1,
                CommitRecordOp::ReplaceContentRange { .. } => replace_content_range_count += 1,
            }
        }
    }

    top_replacements.sort_by(|left, right| {
        right
            .get("expanded_json_bytes")
            .and_then(Value::as_u64)
            .cmp(&left.get("expanded_json_bytes").and_then(Value::as_u64))
    });
    top_replacements.truncate(10);

    let by_type = by_type
        .into_iter()
        .map(|(node_type, totals)| {
            json!({
                "node_type": node_type,
                "count": totals.count,
                "encoded_payload_bytes": totals.encoded_payload_bytes,
                "expanded_json_bytes": totals.expanded_json_bytes,
                "content_bytes": totals.content_bytes,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "full_component_replacements": {
            "count": replacement_count,
            "missing_node_ref_count": replacement_missing_refs,
            "encoded_payload_bytes": replacement_encoded_bytes,
            "expanded_json_bytes": replacement_expanded_bytes,
            "content_bytes": replacement_content_bytes,
            "by_type": by_type,
            "top": top_replacements,
        },
        "new_component_operands": {
            "count": insert_count + append_components_count,
            "insert_component_count": insert_count,
            "append_components_count": append_components_count,
            "encoded_payload_bytes": insert_encoded_bytes,
            "expanded_json_bytes": insert_expanded_bytes,
            "content_bytes": insert_content_bytes,
        },
        "delta_like_ops": {
            "delete_component_count": delete_count,
            "set_component_aux_count": set_attr_count,
            "unset_component_aux_count": unset_attr_count,
            "attach_content_ref_count": attach_content_ref_count,
            "replace_content_range_count": replace_content_range_count,
        },
        "verbose_trace": {
            "count": verbose_trace_count,
            "bytes": verbose_trace_bytes,
        },
        "derived_records_in_canonical_path": {
            "bytes": derived_record_bytes,
            "by_kind": derived_by_kind
                .into_iter()
                .map(|(kind, (count, bytes))| json!({ "kind": kind, "count": count, "bytes": bytes }))
                .collect::<Vec<_>>(),
        },
        "content_bytes_by_node_type": content_bytes_by_node_type(records),
    })
}

fn node_payload_audit_by_id(records: &[ScannedRecord]) -> BTreeMap<String, Vec<NodePayloadAudit>> {
    let mut nodes: BTreeMap<String, Vec<NodePayloadAudit>> = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "node") {
        let Some(component_id) = record.payload.get("id").and_then(Value::as_str) else {
            continue;
        };
        nodes
            .entry(component_id.to_string())
            .or_default()
            .push(NodePayloadAudit {
                sequence: record.sequence,
                node_type: node_type_label(&record.payload),
                encoded_payload_bytes: record.payload_bytes,
                expanded_json_bytes: serde_json::to_vec(&record.payload).map_or(0, |v| v.len()),
                content_bytes: node_inline_content_bytes(&record.payload),
            });
    }
    nodes
}

fn latest_node_payload_before<'a>(
    nodes: &'a BTreeMap<String, Vec<NodePayloadAudit>>,
    component_id: &str,
    commit_sequence: u64,
) -> Option<&'a NodePayloadAudit> {
    nodes.get(component_id).and_then(|items| {
        items
            .iter()
            .rev()
            .find(|node| node.sequence < commit_sequence)
    })
}

fn content_bytes_by_node_type(records: &[ScannedRecord]) -> Value {
    let mut totals: BTreeMap<String, TypeByteTotals> = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind == "node") {
        let node_type = node_type_label(&record.payload);
        let content_bytes = node_inline_content_bytes(&record.payload);
        let entry = totals.entry(node_type).or_default();
        entry.count += 1;
        entry.encoded_payload_bytes += record.payload_bytes;
        entry.expanded_json_bytes += serde_json::to_vec(&record.payload).map_or(0, |v| v.len());
        entry.content_bytes += content_bytes;
    }
    let mut rows = totals
        .into_iter()
        .map(|(node_type, totals)| {
            json!({
                "node_type": node_type,
                "count": totals.count,
                "encoded_payload_bytes": totals.encoded_payload_bytes,
                "expanded_json_bytes": totals.expanded_json_bytes,
                "content_bytes": totals.content_bytes,
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .get("content_bytes")
            .and_then(Value::as_u64)
            .cmp(&left.get("content_bytes").and_then(Value::as_u64))
    });
    Value::Array(rows)
}

fn node_type_label(node: &Value) -> String {
    node.get("type")
        .or_else(|| node.get("componentType"))
        .or_else(|| node.get("node_type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn node_inline_content_bytes(node: &Value) -> usize {
    ["content", "text", "result", "data"]
        .iter()
        .filter_map(|key| node.get(*key).and_then(Value::as_str))
        .map(|text| text.len())
        .sum()
}

#[cfg(test)]
mod tests;
