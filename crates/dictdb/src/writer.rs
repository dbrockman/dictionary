use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::format::*;
use crate::normalize::normalize_key;

/// A search key pointing at an entry.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeySpec<'a> {
    /// Text the user types to find the entry. Normalized before indexing.
    pub keyword: &'a str,
    /// Text shown in the result list. Falls back to `keyword` when empty.
    pub title: &'a str,
    /// Element id inside the entry to scroll to, if any.
    pub anchor: Option<&'a str>,
    /// Lower values rank first among postings of the same key.
    pub priority: u8,
    pub parental: bool,
}

struct PendingKey {
    key: String,
    entry: u32,
    priority: u8,
    flags: u8,
    title: u32,
    anchor: u32,
}

/// Builds a `.dictdb` directory.
///
/// Entries are streamed to disk as they are added; keys and ids are kept in
/// memory and indexed in [`DictWriter::finish`]. Output goes to a temporary
/// sibling directory that replaces `dir` only once everything is written, so
/// an interrupted import never leaves a half-written dictionary behind.
pub struct DictWriter {
    dir: PathBuf,
    tmp_dir: PathBuf,
    info: DictInfo,
    entries_out: BufWriter<File>,
    entries_pos: u64,
    block: Vec<u8>,
    blocks: Vec<(u64, u32)>,
    entry_locs: Vec<(u32, u32, u32)>,
    keys: Vec<PendingKey>,
    ids: Vec<(String, u32)>,
    strings: HashMap<String, u32>,
    strings_buf: Vec<u8>,
}

impl DictWriter {
    pub fn create(dir: impl Into<PathBuf>, info: DictInfo) -> Result<Self> {
        let dir = dir.into();
        let mut tmp_name = dir.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".partial");
        let tmp_dir = dir.with_file_name(tmp_name);
        if tmp_dir.exists() {
            fs::remove_dir_all(&tmp_dir)?;
        }
        fs::create_dir_all(&tmp_dir)?;
        let entries_out = BufWriter::new(File::create(tmp_dir.join(ENTRIES_FILE))?);
        Ok(Self {
            dir,
            tmp_dir,
            info,
            entries_out,
            entries_pos: 0,
            block: Vec::with_capacity(BLOCK_TARGET_LEN * 2),
            blocks: Vec::new(),
            entry_locs: Vec::new(),
            keys: Vec::new(),
            ids: Vec::new(),
            strings: HashMap::new(),
            strings_buf: Vec::new(),
        })
    }

    /// Adds an entry's display HTML and returns its entry number.
    pub fn add_entry(&mut self, id: &str, html: &str) -> Result<u32> {
        let entry = u32::try_from(self.entry_locs.len()).map_err(|_| Error::TooLarge)?;
        let block = u32::try_from(self.blocks.len()).map_err(|_| Error::TooLarge)?;
        let len = u32::try_from(html.len()).map_err(|_| Error::TooLarge)?;
        self.entry_locs.push((block, self.block.len() as u32, len));
        self.block.extend_from_slice(html.as_bytes());
        if !id.is_empty() {
            self.ids.push((id.to_owned(), entry));
        }
        if self.block.len() >= BLOCK_TARGET_LEN {
            self.flush_block()?;
        }
        Ok(entry)
    }

    /// Makes `entry` findable by `key.keyword`.
    pub fn add_key(&mut self, entry: u32, key: KeySpec<'_>) {
        let normalized = normalize_key(key.keyword);
        if normalized.is_empty() {
            return;
        }
        let title = if key.title.is_empty() {
            key.keyword
        } else {
            key.title
        };
        let title = self.intern(title.trim());
        let anchor = key
            .anchor
            .filter(|a| !a.is_empty())
            .map_or(NO_STRING, |a| self.intern(a));
        self.keys.push(PendingKey {
            key: normalized,
            entry,
            priority: key.priority,
            flags: if key.parental { FLAG_PARENTAL } else { 0 },
            title,
            anchor,
        });
    }

    /// Copies a resource file (e.g. an image) referenced by entries.
    pub fn add_resource(&mut self, rel_path: &Path, bytes: &[u8]) -> Result<()> {
        let path = self.tmp_dir.join(RESOURCES_DIR).join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
        Ok(())
    }

    pub fn entry_count(&self) -> u32 {
        self.entry_locs.len() as u32
    }

    pub fn finish(mut self) -> Result<Meta> {
        self.flush_block()?;
        self.entries_out.flush()?;

        self.write_entries_index()?;
        let key_count = self.write_keys()?;
        self.write_ids()?;
        fs::write(self.tmp_dir.join(STRINGS_FILE), &self.strings_buf)?;

        let meta = Meta {
            format_version: FORMAT_VERSION,
            name: self.info.name.clone(),
            identifier: self.info.identifier.clone(),
            languages: self.info.languages.clone(),
            source_kind: self.info.source_kind.clone(),
            copyright: self.info.copyright.clone(),
            entry_count: self.entry_locs.len() as u32,
            key_count,
        };
        fs::write(
            self.tmp_dir.join(META_FILE),
            serde_json::to_vec_pretty(&meta)?,
        )?;

        if self.dir.exists() {
            fs::remove_dir_all(&self.dir)?;
        }
        fs::rename(&self.tmp_dir, &self.dir)?;
        Ok(meta)
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&offset) = self.strings.get(s) {
            return offset;
        }
        let offset = self.strings_buf.len() as u32;
        self.strings_buf
            .extend_from_slice(&(s.len() as u32).to_le_bytes());
        self.strings_buf.extend_from_slice(s.as_bytes());
        self.strings.insert(s.to_owned(), offset);
        offset
    }

    fn flush_block(&mut self) -> Result<()> {
        if self.block.is_empty() {
            return Ok(());
        }
        let compressed = zstd::bulk::compress(&self.block, ZSTD_LEVEL)?;
        self.entries_out.write_all(&compressed)?;
        self.blocks
            .push((self.entries_pos, compressed.len() as u32));
        self.entries_pos += compressed.len() as u64;
        self.block.clear();
        Ok(())
    }

    fn write_entries_index(&self) -> Result<()> {
        let mut out = BufWriter::new(File::create(self.tmp_dir.join(ENTRIES_IDX_FILE))?);
        out.write_all(IDX_MAGIC)?;
        out.write_all(&(self.entry_locs.len() as u32).to_le_bytes())?;
        out.write_all(&(self.blocks.len() as u32).to_le_bytes())?;
        for &(block, offset, len) in &self.entry_locs {
            out.write_all(&block.to_le_bytes())?;
            out.write_all(&offset.to_le_bytes())?;
            out.write_all(&len.to_le_bytes())?;
        }
        for &(offset, len) in &self.blocks {
            out.write_all(&offset.to_le_bytes())?;
            out.write_all(&len.to_le_bytes())?;
        }
        out.flush()?;
        Ok(())
    }

    /// Writes `postings.bin` and `keys.fst`; returns the number of distinct keys.
    fn write_keys(&mut self) -> Result<u32> {
        self.keys.sort_unstable_by(|a, b| {
            (&a.key, a.priority, a.entry, a.title, a.anchor)
                .cmp(&(&b.key, b.priority, b.entry, b.title, b.anchor))
        });
        // Variants that normalize to the same key (e.g. "Café" and "café")
        // would otherwise list the same entry twice.
        self.keys
            .dedup_by(|b, a| a.key == b.key && a.entry == b.entry && a.anchor == b.anchor);

        let mut postings = BufWriter::new(File::create(self.tmp_dir.join(POSTINGS_FILE))?);
        let mut fst =
            fst::MapBuilder::new(BufWriter::new(File::create(self.tmp_dir.join(KEYS_FILE))?))?;
        let mut pos = 0u64;
        let mut key_count = 0u32;
        for group in self.keys.chunk_by(|a, b| a.key == b.key) {
            fst.insert(&group[0].key, pos)?;
            postings.write_all(&(group.len() as u32).to_le_bytes())?;
            for p in group {
                postings.write_all(&p.entry.to_le_bytes())?;
                postings.write_all(&[p.priority, p.flags])?;
                postings.write_all(&p.title.to_le_bytes())?;
                postings.write_all(&p.anchor.to_le_bytes())?;
            }
            pos += 4 + (group.len() * POSTING_LEN) as u64;
            key_count += 1;
        }
        fst.finish()?;
        postings.flush()?;
        Ok(key_count)
    }

    fn write_ids(&mut self) -> Result<()> {
        self.ids.sort_unstable();
        self.ids.dedup_by(|b, a| a.0 == b.0);
        let mut fst =
            fst::MapBuilder::new(BufWriter::new(File::create(self.tmp_dir.join(IDS_FILE))?))?;
        for (id, entry) in &self.ids {
            fst.insert(id, u64::from(*entry))?;
        }
        fst.finish()?;
        Ok(())
    }
}
