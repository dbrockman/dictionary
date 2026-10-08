use std::collections::HashSet;
use std::fs::{self, File};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fst::{IntoStreamer, Streamer};
use lru::LruCache;
use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::format::*;
use crate::normalize::normalize_key;

/// Number of decompressed entry blocks kept in memory per dictionary.
const BLOCK_CACHE_LEN: usize = 8;

/// A search result: one key pointing at one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The normalized key that matched.
    pub key: String,
    /// Text to show in the result list.
    pub title: String,
    pub entry: u32,
    /// Element id inside the entry to scroll to.
    pub anchor: Option<String>,
    pub priority: u8,
    pub parental: bool,
}

/// An open, memory-mapped `.dictdb` directory.
pub struct Dictionary {
    path: PathBuf,
    meta: Meta,
    keys: fst::Map<Data>,
    ids: fst::Map<Data>,
    postings: Data,
    strings: Data,
    index: Data,
    entries: Data,
    block_count: usize,
    blocks: Mutex<LruCache<u32, Arc<[u8]>>>,
}

impl Dictionary {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let meta: Meta = serde_json::from_slice(&fs::read(path.join(META_FILE))?)?;
        if meta.format_version != FORMAT_VERSION {
            return Err(Error::UnsupportedVersion {
                path,
                version: meta.format_version,
            });
        }
        let corrupt = |reason| Error::Corrupt {
            path: path.clone(),
            reason,
        };

        let index = Data::map(&path.join(ENTRIES_IDX_FILE))?;
        if index.len() < IDX_HEADER_LEN || &index[..4] != IDX_MAGIC {
            return Err(corrupt("bad entries.idx header"));
        }
        let entry_count = read_u32(&index, 4) as usize;
        let block_count = read_u32(&index, 8) as usize;
        if index.len() != IDX_HEADER_LEN + entry_count * IDX_ENTRY_LEN + block_count * IDX_BLOCK_LEN
        {
            return Err(corrupt("entries.idx size mismatch"));
        }

        Ok(Self {
            keys: fst::Map::new(Data::map(&path.join(KEYS_FILE))?)?,
            ids: fst::Map::new(Data::map(&path.join(IDS_FILE))?)?,
            postings: Data::map(&path.join(POSTINGS_FILE))?,
            strings: Data::map(&path.join(STRINGS_FILE))?,
            entries: Data::map(&path.join(ENTRIES_FILE))?,
            index,
            block_count,
            blocks: Mutex::new(LruCache::new(NonZeroUsize::new(BLOCK_CACHE_LEN).unwrap())),
            meta,
            path,
        })
    }

    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn resources_dir(&self) -> PathBuf {
        self.path.join(RESOURCES_DIR)
    }

    /// Returns up to `limit` hits whose key starts with the normalized `query`,
    /// in key order (so an exact match comes first).
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        let query = normalize_key(query);
        let mut hits = Vec::new();
        if query.is_empty() || limit == 0 {
            return Ok(hits);
        }
        let mut seen = HashSet::new();
        let mut stream = self.keys.range().ge(&query).into_stream();
        while let Some((key, offset)) = stream.next() {
            if !key.starts_with(query.as_bytes()) {
                break;
            }
            let key = String::from_utf8_lossy(key).into_owned();
            for posting in self.postings_at(offset)? {
                if !seen.insert((posting.entry, posting.title)) {
                    continue;
                }
                hits.push(self.make_hit(&key, &posting)?);
                if hits.len() == limit {
                    return Ok(hits);
                }
            }
        }
        Ok(hits)
    }

    /// Returns all hits for exactly `word` (after normalization).
    pub fn lookup(&self, word: &str) -> Result<Vec<Hit>> {
        let key = normalize_key(word);
        let Some(offset) = self.keys.get(&key) else {
            return Ok(Vec::new());
        };
        self.postings_at(offset)?
            .iter()
            .map(|p| self.make_hit(&key, p))
            .collect()
    }

    /// Resolves a source entry id (as used by cross-reference links).
    pub fn entry_by_id(&self, id: &str) -> Option<u32> {
        self.ids.get(id).map(|n| n as u32)
    }

    /// Returns the display HTML of `entry`.
    pub fn entry_html(&self, entry: u32) -> Result<String> {
        if entry as usize >= self.meta.entry_count as usize {
            return Err(Error::NoSuchEntry(entry));
        }
        let at = IDX_HEADER_LEN + entry as usize * IDX_ENTRY_LEN;
        let block = read_u32(&self.index, at);
        let offset = read_u32(&self.index, at + 4) as usize;
        let len = read_u32(&self.index, at + 8) as usize;
        let data = self.block(block)?;
        let bytes = data
            .get(offset..offset + len)
            .ok_or_else(|| self.corrupt("entry outside its block"))?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn block(&self, block: u32) -> Result<Arc<[u8]>> {
        if let Some(data) = self.blocks.lock().unwrap().get(&block) {
            return Ok(data.clone());
        }
        if block as usize >= self.block_count {
            return Err(self.corrupt("block number out of range"));
        }
        let at = IDX_HEADER_LEN
            + self.meta.entry_count as usize * IDX_ENTRY_LEN
            + block as usize * IDX_BLOCK_LEN;
        let offset = read_u64(&self.index, at) as usize;
        let len = read_u32(&self.index, at + 8) as usize;
        let compressed = self
            .entries
            .get(offset..offset + len)
            .ok_or_else(|| self.corrupt("block outside entries.bin"))?;
        let data: Arc<[u8]> = zstd::stream::decode_all(compressed)?.into();
        self.blocks.lock().unwrap().put(block, data.clone());
        Ok(data)
    }

    fn postings_at(&self, offset: u64) -> Result<Vec<RawPosting>> {
        let offset = offset as usize;
        let count = self
            .postings
            .get(offset..offset + 4)
            .map(|_| read_u32(&self.postings, offset) as usize)
            .ok_or_else(|| self.corrupt("posting offset out of range"))?;
        let start = offset + 4;
        let bytes = self
            .postings
            .get(start..start + count * POSTING_LEN)
            .ok_or_else(|| self.corrupt("postings out of range"))?;
        Ok(bytes
            .as_chunks::<POSTING_LEN>()
            .0
            .iter()
            .map(|p| RawPosting {
                entry: read_u32(p, 0),
                priority: p[4],
                flags: p[5],
                title: read_u32(p, 6),
                anchor: read_u32(p, 10),
            })
            .collect())
    }

    fn make_hit(&self, key: &str, p: &RawPosting) -> Result<Hit> {
        Ok(Hit {
            key: key.to_owned(),
            title: self.string(p.title)?.unwrap_or_default(),
            entry: p.entry,
            anchor: self.string(p.anchor)?,
            priority: p.priority,
            parental: p.flags & FLAG_PARENTAL != 0,
        })
    }

    fn string(&self, offset: u32) -> Result<Option<String>> {
        if offset == NO_STRING {
            return Ok(None);
        }
        let offset = offset as usize;
        let len = self
            .strings
            .get(offset..offset + 4)
            .map(|_| read_u32(&self.strings, offset) as usize)
            .ok_or_else(|| self.corrupt("string offset out of range"))?;
        let bytes = self
            .strings
            .get(offset + 4..offset + 4 + len)
            .ok_or_else(|| self.corrupt("string out of range"))?;
        Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
    }

    fn corrupt(&self, reason: &'static str) -> Error {
        Error::Corrupt {
            path: self.path.clone(),
            reason,
        }
    }
}

struct RawPosting {
    entry: u32,
    priority: u8,
    flags: u8,
    title: u32,
    anchor: u32,
}

/// A memory-mapped file; empty files cannot be mapped, so they get no mapping.
enum Data {
    Mapped(Mmap),
    Empty,
}

impl Data {
    fn map(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        if file.metadata()?.len() == 0 {
            return Ok(Data::Empty);
        }
        // SAFETY: dictdb files are written once by the importer and replaced by
        // an atomic directory rename, never modified in place.
        Ok(Data::Mapped(unsafe { Mmap::map(&file)? }))
    }
}

impl AsRef<[u8]> for Data {
    fn as_ref(&self) -> &[u8] {
        match self {
            Data::Mapped(m) => m,
            Data::Empty => &[],
        }
    }
}

impl std::ops::Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}
