use crate::error::{PactError, PactResult};

pub fn should_freeze_commit_bytes(
    bytes_since_last_freeze: usize,
    threshold: Option<usize>,
) -> bool {
    threshold.is_some_and(|threshold| threshold > 0 && bytes_since_last_freeze >= threshold)
}

pub fn should_freeze_cycle_interval(
    cycle: u64,
    last_anchor_cycle: Option<u64>,
    interval: Option<u64>,
) -> bool {
    let Some(interval) = interval else {
        return false;
    };
    if interval == 0 || cycle == 0 {
        return false;
    }
    let previous = last_anchor_cycle.unwrap_or(0);
    cycle >= previous.saturating_add(interval)
}

pub(super) fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
}

pub(super) fn stable_hash(kind: &str, bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(kind.as_bytes());
    hasher.update(&[0]);
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.as_bytes()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn stable_checksum(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

pub(super) fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset + 2)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
}

pub(super) fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
}

pub(super) fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    bytes
        .get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(BASE64_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(BASE64_ALPHABET[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub(crate) fn base64_decode(value: &str) -> PactResult<Vec<u8>> {
    let clean = value
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if clean.len() % 4 != 0 {
        return Err(PactError::codec(
            "base64 value length is not a multiple of 4".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for (block_index, chunk) in clean.chunks(4).enumerate() {
        let mut sextets = [0_u8; 4];
        let mut padding = 0usize;
        for (index, byte) in chunk.iter().copied().enumerate() {
            if byte == b'=' {
                padding += 1;
                sextets[index] = 0;
                continue;
            }
            if padding > 0 {
                return Err(PactError::codec(format!(
                    "invalid base64 padding in block {block_index}: data after padding"
                )));
            }
            sextets[index] = base64_value(byte).ok_or_else(|| {
                PactError::codec(format!(
                    "invalid base64 byte 0x{byte:02x} in block {block_index}"
                ))
            })?;
        }
        if padding > 2 {
            return Err(PactError::codec(format!(
                "invalid base64 padding in block {block_index}"
            )));
        }
        out.push((sextets[0] << 2) | (sextets[1] >> 4));
        if padding < 2 {
            out.push((sextets[1] << 4) | (sextets[2] >> 2));
        }
        if padding < 1 {
            out.push((sextets[2] << 6) | sextets[3]);
        }
    }
    Ok(out)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}
