pub const PACT_FILE_SCHEMA: &str = "pact.file.v2";
pub(crate) const FILE_MAGIC: &[u8; 8] = b"PACTBIN2";
pub(crate) const FILE_VERSION: u16 = 2;
pub(crate) const HEADER_BYTES: usize = 4096;
pub(crate) const ROOT_SLOT_COUNT: usize = 2;
pub(crate) const ROOT_SLOT_START: usize = 64;
pub(crate) const ROOT_SLOT_BYTES: usize = 128;

pub(crate) const FILE_IDENTITY_START: usize = 16;
pub(crate) const FILE_IDENTITY_BYTES: usize = 32;
pub(crate) const ROOT_SLOT_DIGEST_START: usize = 96;
pub(crate) const ROOT_SLOT_DIGEST_BYTES: usize = 32;
pub(crate) const ROOT_SLOT_PAYLOAD_BYTES: usize = 96;
pub(crate) const MAX_FILE_LENGTH_WATERMARK: u64 = 1_u64 << 56;
