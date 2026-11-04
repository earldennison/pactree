pub const PACT_FILE_SCHEMA: &str = "pact.file.v0";
pub const PACT_FILE_PROJECTION_SCHEMA: &str = "pact.projection.v1";
pub const PACT_FILE_HEAD_SCHEMA: &str = "pact.head.v1";
pub(super) const FILE_VERSION: u16 = 1;
pub(super) const FILE_HEADER_LEN: usize = 16;
pub(super) const PAGE_HEADER_LEN: usize = 32;
pub(super) const RECORD_HEADER_LEN: usize = 22;
pub(super) const PAGE_KIND_COMPRESSED_FLAG: u16 = 0x8000;
pub(super) const PAGE_DICT_MAGIC: &[u8] = b"PZD1";
pub(super) const PACT_RECORD_ZSTD_DICT_V1: &[u8] = b"pact.file.v0.record.dict.v1\0schema\0kind\0sequence\0payload\0componentType\0type\0id\0children\0content\0content_ref\0role\0message_turn\0text_content\0reasoning_content\0visibility\0displayable\0opaque\0encoding\0data\0provider_encrypted\0tool_call\0tool_result\0tool_call_id\0tool_name\0parameters\0parameters_ref\0result\0result_ref\0success\0error_message\0error_ref\0branch_id\0revision\0cycle\0transaction_id\0parent_id\0node_ref\0node_id\0ops\0op\0position\0commit\0head\0root_node_ref\0changed_node_refs\0content_ref_ids\0record_sequence_ids\0ttl\0cad\0key\0tags\0attrs\0aux\0org";
pub(super) const FILE_FOOTER_MAGIC: &[u8; 4] = b"PFTR";
pub(super) const FILE_FOOTER_VERSION: u16 = 1;
pub(super) const FILE_FOOTER_LEN: usize = 72;
pub(super) const LIVE_CONTENT_CHUNK_BYTES: usize = 2 * 1024;
pub const DEFAULT_FREEZE_CYCLE_INTERVAL: u64 = 1000;
pub const DEFAULT_FREEZE_COMMIT_BYTES_THRESHOLD: usize = 1024 * 1024;
pub(super) const FROZEN_CYCLE_SCHEMA: &str = "pact.frozen_cycle.v1";
pub(super) const COMMIT_RECORD_SCHEMA: &str = "pact.commit_record.v1";
pub(super) const HEAD_RECORD_SCHEMA: &str = "pact.head_record.v1";
pub(super) const PACT_BUNDLE_SCHEMA: &str = "pact.bundle.v1";
pub(super) const CONTENT_HASH_SCHEME: &str = "blake3-128";
pub(super) const RECORD_PAYLOAD_MAGIC: &[u8; 4] = b"PVL1";
pub(super) const VALUE_TAG_NULL: u8 = 0x00;
pub(super) const VALUE_TAG_FALSE: u8 = 0x01;
pub(super) const VALUE_TAG_TRUE: u8 = 0x02;
pub(super) const VALUE_TAG_I64: u8 = 0x03;
pub(super) const VALUE_TAG_U64: u8 = 0x04;
pub(super) const VALUE_TAG_F64: u8 = 0x05;
pub(super) const VALUE_TAG_STRING: u8 = 0x06;
pub(super) const VALUE_TAG_ARRAY: u8 = 0x07;
pub(super) const VALUE_TAG_OBJECT: u8 = 0x08;
pub(super) const VALUE_TAG_BYTES: u8 = 0x09;
pub(super) const VALUE_TAG_SYMBOL_REF: u8 = 0x0a;
pub(super) const VALUE_TAG_VAR_U64: u8 = 0x0b;
pub(super) const VALUE_TAG_ZIGZAG_I64: u8 = 0x0c;
pub(super) const VALUE_TAG_OBJECT_SYMBOL_KEYS: u8 = 0x0d;
pub(super) const VALUE_TAG_FILE_LOCAL_ID_REF: u8 = 0x0e;
pub(super) const VALUE_TAG_TOKEN_SPAN_STRING: u8 = 0x0f;
pub(super) const TOKEN_SPAN_PART_LEX_REF: u8 = 0x00;
pub(super) const TOKEN_SPAN_PART_SPAN_REF: u8 = 0x01;
pub(super) const TOKEN_SPAN_PART_TOKEN_ID: u8 = 0x02;
pub(super) const TOKEN_SPAN_PART_UTF8_ESCAPE: u8 = 0x03;
pub(super) const TOKEN_SPAN_PART_LEX_REF_RUN: u8 = 0x04;
pub(super) const TOKEN_SPAN_PART_TOKEN_ID_RUN: u8 = 0x05;
pub(super) const TOKEN_SPAN_PART_SPAN_REF_RUN: u8 = 0x06;
pub(super) const TOKEN_SPAN_PART_DENSE_TOKEN_ID: u8 = 0x07;
pub(super) const TOKEN_SPAN_PART_DENSE_TOKEN_ID_RUN: u8 = 0x08;
pub(super) const TOKEN_SPAN_PART_TOKEN_MIXED_RUN: u8 = 0x09;
pub(super) const TOKEN_SPAN_PART_TOKEN_TABLE_RUN: u8 = 0x0a;
pub(super) const TOKEN_SPAN_TOKENIZER_O200K_BASE_ID: u64 = 0;
pub(super) const TOKEN_SPAN_STRINGS_V1: &str = "token_span_strings_v1";
pub(super) const ENGLISH_LEXPACK_V1: &str = "english_lexpack_v1";
pub(super) const ENGLISH_LEXPACK_ID: &str = "pact.lexpack.english.v1";
pub(super) const SHELL_LEXPACK_V1: &str = "shell_lexpack_v1";
pub(super) const SHELL_LEXPACK_ID: &str = "pact.lexpack.shell.v1";
pub(super) const TRACE_COMPACT_V2: &str = "trace_compact:v2";
pub(super) const BINARY_BYTES_SENTINEL: &str = "\u{0}pact.bytes";
pub(super) const BUILTIN_SYMBOLS: &[&str] = &[
    "active",
    "assistant",
    "branch",
    "component",
    "component.args",
    "component.delta_state",
    "content_manifest",
    "content_ref",
    "main",
    "message_turn",
    "node",
    "pact.commit_record.v1",
    "pact.file.v0",
    "pact.frozen_cycle.v1",
    "pact.head_record.v1",
    "root",
    "reasoning_content",
    "state_changed",
    "system",
    "text_content",
    "user",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub(super) enum PageKind {
    Meta = 1,
    Symbol = 2,
    Content = 3,
    Record = 4,
    Index = 5,
}

impl PageKind {
    pub(super) fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Meta),
            2 => Some(Self::Symbol),
            3 => Some(Self::Content),
            4 => Some(Self::Record),
            5 => Some(Self::Index),
            _ => None,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Meta => "meta",
            Self::Symbol => "symbol",
            Self::Content => "content",
            Self::Record => "record",
            Self::Index => "index",
        }
    }
}

pub(super) fn decode_page_kind(raw_page_kind: u16) -> Option<(PageKind, bool)> {
    let compressed = raw_page_kind & PAGE_KIND_COMPRESSED_FLAG != 0;
    let kind = PageKind::from_u16(raw_page_kind & !PAGE_KIND_COMPRESSED_FLAG)?;
    Some((kind, compressed))
}

pub(super) fn encode_page_kind(page_kind: PageKind, compressed: bool) -> u16 {
    let raw = page_kind as u16;
    if compressed {
        raw | PAGE_KIND_COMPRESSED_FLAG
    } else {
        raw
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub(super) enum RecordKind {
    Meta = 1,
    Symbol = 2,
    Content = 3,
    ContentChunk = 4,
    ContentManifest = 5,
    Root = 10,
    Branch = 11,
    Node = 12,
    Trace = 13,
    Component = 14,
    ComponentPayload = 16,
    View = 17,
    Commit = 18,
    FrozenCycle = 19,
    Index = 20,
    Head = 21,
}

impl RecordKind {
    pub(super) fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Meta),
            2 => Some(Self::Symbol),
            3 => Some(Self::Content),
            4 => Some(Self::ContentChunk),
            5 => Some(Self::ContentManifest),
            10 => Some(Self::Root),
            11 => Some(Self::Branch),
            12 => Some(Self::Node),
            13 => Some(Self::Trace),
            14 => Some(Self::Component),
            16 => Some(Self::ComponentPayload),
            17 => Some(Self::View),
            18 => Some(Self::Commit),
            19 => Some(Self::FrozenCycle),
            20 => Some(Self::Index),
            21 => Some(Self::Head),
            _ => None,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Meta => "meta",
            Self::Symbol => "symbol",
            Self::Content => "content",
            Self::ContentChunk => "content_chunk",
            Self::ContentManifest => "content_manifest",
            Self::Root => "root",
            Self::Branch => "branch",
            Self::Node => "node",
            Self::Trace => "trace",
            Self::Component => "component",
            Self::ComponentPayload => "component_payload",
            Self::View => "view",
            Self::Commit => "commit",
            Self::FrozenCycle => "frozen_cycle",
            Self::Index => "index",
            Self::Head => "head",
        }
    }

    pub(super) fn from_str(value: &str) -> Option<Self> {
        match value {
            "meta" => Some(Self::Meta),
            "symbol" => Some(Self::Symbol),
            "content" => Some(Self::Content),
            "content_chunk" => Some(Self::ContentChunk),
            "content_manifest" => Some(Self::ContentManifest),
            "root" => Some(Self::Root),
            "branch" => Some(Self::Branch),
            "node" => Some(Self::Node),
            "trace" => Some(Self::Trace),
            "component" => Some(Self::Component),
            "component_payload" => Some(Self::ComponentPayload),
            "view" => Some(Self::View),
            "commit" => Some(Self::Commit),
            "frozen_cycle" => Some(Self::FrozenCycle),
            "index" => Some(Self::Index),
            "head" => Some(Self::Head),
            _ => None,
        }
    }
}
