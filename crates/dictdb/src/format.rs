//! On-disk layout of a `.dictdb` directory (format version 2).
//!
//! All integers are little-endian.
//!
//! - `meta.json`: [`Meta`].
//! - `keys.fst`: `fst::Map` from normalized key to a byte offset into `postings.bin`.
//! - `postings.bin`: at each offset, a `u32` count followed by that many
//!   [`POSTING_LEN`]-byte postings: `entry u32, priority u8, flags u8,
//!   title u32, anchor u32`. `title` and `anchor` are offsets into
//!   `strings.bin`, with [`NO_STRING`] meaning absent. Postings of one key are
//!   sorted by priority (lowest value first), then entry number.
//! - `strings.bin`: deduplicated strings, each a `u32` length then UTF-8 bytes.
//! - `ids.fst`: `fst::Map` from source entry id to entry number.
//! - `entries.idx`: [`IDX_MAGIC`], `u32 entry_count`, `u32 block_count`, then
//!   `entry_count` × `(block u32, offset u32, len u32, title u32, detail u32)`,
//!   then `block_count` × `(file_offset u64, compressed_len u32)`. `title` and
//!   `detail` are offsets into `strings.bin` (or [`NO_STRING`]) that label the
//!   entry in result lists.
//! - `entries.bin`: zstd-compressed blocks of concatenated entry HTML.

use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 2;

pub const META_FILE: &str = "meta.json";
pub const KEYS_FILE: &str = "keys.fst";
pub const POSTINGS_FILE: &str = "postings.bin";
pub const STRINGS_FILE: &str = "strings.bin";
pub const IDS_FILE: &str = "ids.fst";
pub const ENTRIES_IDX_FILE: &str = "entries.idx";
pub const ENTRIES_FILE: &str = "entries.bin";
/// Images and other files referenced by entries.
pub const RESOURCES_DIR: &str = "resources";

pub const IDX_MAGIC: &[u8; 4] = b"DDBI";
pub const IDX_HEADER_LEN: usize = 12;
pub const IDX_ENTRY_LEN: usize = 20;
pub const IDX_BLOCK_LEN: usize = 12;

pub const POSTING_LEN: usize = 14;
pub const NO_STRING: u32 = u32::MAX;

/// Uncompressed size at which an entry block is flushed.
pub const BLOCK_TARGET_LEN: usize = 32 * 1024;
pub const ZSTD_LEVEL: i32 = 19;

/// Posting flag: the key is hidden when parental controls are enabled.
pub const FLAG_PARENTAL: u8 = 1;

/// Dictionary-level metadata, stored as `meta.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub format_version: u32,
    /// Human-readable name shown in the UI.
    pub name: String,
    /// Stable identifier, e.g. the source bundle identifier.
    pub identifier: String,
    /// Languages covered, as BCP 47-ish codes from the source.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Which importer produced this dictionary, e.g. `apple-bundle` or `ddk-xml`.
    pub source_kind: String,
    #[serde(default)]
    pub copyright: Option<String>,
    pub entry_count: u32,
    pub key_count: u32,
}

/// Metadata supplied by an importer; counts and version are filled in by the writer.
#[derive(Debug, Clone, Default)]
pub struct DictInfo {
    pub name: String,
    pub identifier: String,
    pub languages: Vec<String>,
    pub source_kind: String,
    pub copyright: Option<String>,
}

pub(crate) fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

pub(crate) fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}
