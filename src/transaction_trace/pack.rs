use super::TransactionTrace;
use super::encode::{decode_transaction_trace, encode_transaction_trace};
use crate::error::{PactError, PactResult};

fn write_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn read_varint(data: &[u8], pos: &mut usize) -> PactResult<u64> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data
            .get(*pos)
            .ok_or_else(|| PactError::codec("unexpected end of pack bytes"))?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            return Err(PactError::codec("varint overflow"));
        }
    }
    Ok(value)
}

pub fn pack_transaction_traces(traces: &[TransactionTrace]) -> Vec<u8> {
    let mut out = Vec::new();
    write_varint(&mut out, traces.len() as u64);
    for trace in traces {
        let bytes = encode_transaction_trace(trace);
        write_varint(&mut out, bytes.len() as u64);
        out.extend_from_slice(&bytes);
    }
    out
}

pub fn unpack_transaction_traces(data: &[u8]) -> PactResult<Vec<TransactionTrace>> {
    let mut pos = 0;

    let count = read_varint(data, &mut pos)?;
    let mut traces = Vec::new();
    for _ in 0..count {
        let len = read_varint(data, &mut pos)? as usize;
        let end = pos
            .checked_add(len)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| PactError::corrupt("trace length overruns pack bytes"))?;
        traces.push(decode_transaction_trace(&data[pos..end])?);
        pos = end;
    }
    Ok(traces)
}

#[cfg(feature = "compression-zstd")]
pub fn compress_pack(pack: &[u8]) -> PactResult<Vec<u8>> {
    zstd::stream::encode_all(pack, 19)
        .map_err(|err| PactError::codec(format!("failed to compress trace pack: {err}")))
}

#[cfg(feature = "compression-zstd")]
pub fn decompress_pack(data: &[u8]) -> PactResult<Vec<u8>> {
    zstd::stream::decode_all(data)
        .map_err(|err| PactError::codec(format!("failed to decompress trace pack: {err}")))
}

#[cfg(feature = "compression-zstd")]
pub fn compress_pack_with_dict(pack: &[u8], dict: &[u8]) -> PactResult<Vec<u8>> {
    use std::io::Write;
    let mut encoder =
        zstd::stream::Encoder::with_dictionary(Vec::new(), 19, dict).map_err(|err| {
            PactError::codec(format!("failed to create trace dictionary encoder: {err}"))
        })?;
    encoder.write_all(pack).map_err(|err| {
        PactError::codec(format!("failed to write trace dictionary payload: {err}"))
    })?;
    encoder.finish().map_err(|err| {
        PactError::codec(format!("failed to finish trace dictionary payload: {err}"))
    })
}

#[cfg(feature = "compression-zstd")]
pub fn decompress_pack_with_dict(data: &[u8], dict: &[u8]) -> PactResult<Vec<u8>> {
    use std::io::Read;
    let cursor = std::io::Cursor::new(data);
    let mut decoder = zstd::stream::Decoder::with_dictionary(cursor, dict).map_err(|err| {
        PactError::codec(format!("failed to create trace dictionary decoder: {err}"))
    })?;
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).map_err(|err| {
        PactError::codec(format!("failed to read trace dictionary payload: {err}"))
    })?;
    Ok(decoded)
}

#[cfg(test)]
mod tests;
