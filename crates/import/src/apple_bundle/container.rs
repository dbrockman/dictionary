//! The section container shared by `Body.data` and `KeyText.data`.
//!
//! Layout, as reverse-engineered from shipped dictionaries:
//!
//! - The first [`HEADER_LEN`] bytes are a header; all offsets below are
//!   relative to its end.
//! - At `0x40` is an `i32` *limit*: the end of the data. If the 8 bytes after
//!   it are `00000000 FFFFFFFF`, data starts at offset `0x20`, otherwise at `0x4`.
//! - `Body.data` is a run of sections `[i32 next][i32 len]`, followed by either
//!   `len` raw bytes or (when compressed) `[i32 decompressed_len]` and a zlib
//!   stream of `len - 4` bytes. The next section starts `next + 4` bytes after
//!   the current one.
//! - Compressed `KeyText.data` starts with an `i32` stride, then chunks
//!   `[i32 compressed_len][i32 decompressed_len]` + zlib (`compressed_len - 4`
//!   bytes); the decompressed chunks concatenate into one record buffer.

use std::io::Read;

use flate2::read::ZlibDecoder;

pub(crate) const HEADER_LEN: usize = 0x40;

/// Address of an entry in `Body.data`: the section's offset and the entry's
/// offset within the (decompressed) section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct Address {
    pub section: u32,
    pub chunk: u32,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ContainerError {
    #[error("file too short or truncated at offset {0:#x}")]
    Truncated(usize),
    #[error(
        "zlib data at offset {offset:#x} could not be decompressed ({source}); the dictionary may be encrypted or use an unsupported compression"
    )]
    Decompress {
        offset: usize,
        source: std::io::Error,
    },
}

type Result<T> = std::result::Result<T, ContainerError>;

pub(crate) fn read_i32(data: &[u8], at: usize) -> Result<i32> {
    data.get(at..at + 4)
        .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
        .ok_or(ContainerError::Truncated(at))
}

pub(crate) fn read_u32(data: &[u8], at: usize) -> Result<u32> {
    read_i32(data, at).map(|v| v as u32)
}

pub(crate) fn read_u16(data: &[u8], at: usize) -> Result<u16> {
    data.get(at..at + 2)
        .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
        .ok_or(ContainerError::Truncated(at))
}

fn slice(data: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    data.get(
        start
            ..start
                .checked_add(len)
                .ok_or(ContainerError::Truncated(start))?,
    )
    .ok_or(ContainerError::Truncated(start))
}

/// Returns `(data_start, limit)`, both relative to the end of the header.
pub(crate) fn bounds(file: &[u8]) -> Result<(usize, usize)> {
    let limit = read_i32(file, HEADER_LEN)?.max(0) as usize;
    let marker = (
        read_i32(file, HEADER_LEN + 4)?,
        read_i32(file, HEADER_LEN + 8)?,
    );
    let start = if marker == (0, -1) { 0x20 } else { 0x4 };
    Ok((start, limit))
}

fn inflate(data: &[u8], offset: usize, size_hint: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(size_hint.min(64 << 20));
    ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|source| ContainerError::Decompress { offset, source })?;
    Ok(out)
}

/// One section of `Body.data`.
pub(crate) struct Section {
    pub offset: u32,
    pub data: Vec<u8>,
}

/// Iterates over the sections of `Body.data`, decompressing each.
pub(crate) fn body_sections(file: &[u8], compressed: bool) -> Result<BodySections<'_>> {
    let (start, limit) = bounds(file)?;
    Ok(BodySections {
        file,
        compressed,
        offset: start,
        limit,
    })
}

pub(crate) struct BodySections<'a> {
    file: &'a [u8],
    compressed: bool,
    offset: usize,
    limit: usize,
}

impl BodySections<'_> {
    pub fn progress(&self) -> f32 {
        self.offset as f32 / self.limit.max(1) as f32
    }

    fn read(&mut self) -> Result<Section> {
        let at = HEADER_LEN + self.offset;
        let next = read_i32(self.file, at)?.max(0) as usize;
        let len = read_i32(self.file, at + 4)?.max(0) as usize;
        let data = if self.compressed {
            let decompressed_len = read_i32(self.file, at + 8)?.max(0) as usize;
            let payload = slice(self.file, at + 12, len.saturating_sub(4))?;
            inflate(payload, at + 12, decompressed_len)?
        } else {
            slice(self.file, at + 8, len)?.to_vec()
        };
        let section = Section {
            offset: self.offset as u32,
            data,
        };
        self.offset += next + 4;
        Ok(section)
    }
}

impl Iterator for BodySections<'_> {
    type Item = Result<Section>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.limit {
            return None;
        }
        let item = self.read();
        if item.is_err() {
            // Section framing is lost; nothing after this can be trusted.
            self.offset = self.limit;
        }
        Some(item)
    }
}

const ENTRY_START: &[u8] = b"<d:entry";
const ENTRY_END: &[u8] = b"</d:entry>";

/// Splits a decompressed body section into `(offset, entry XHTML)` pairs.
///
/// Entries are normally prefixed by their `u32` length; some dictionaries
/// omit the prefix, in which case an entry runs to its closing tag.
pub(crate) fn section_entries(section: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        if pos >= section.len() {
            return None;
        }
        let window = &section[pos..section.len().min(pos + 12)];
        let Some(tag_at) = find(window, ENTRY_START) else {
            // Padding, or framing we do not understand: skip the rest.
            pos = section.len();
            return None;
        };
        let start = pos;
        let body_start = pos + tag_at;
        let len = if tag_at >= 4 {
            read_u32(section, body_start - 4).ok().map(|l| l as usize)
        } else {
            None
        };
        let body_end = match len {
            Some(len) if body_start + len <= section.len() => {
                pos = body_start + len;
                pos
            }
            _ => {
                let end = find(&section[body_start..], ENTRY_END)
                    .map_or(section.len(), |i| body_start + i + ENTRY_END.len());
                // Unprefixed entries are newline-terminated; the next
                // entry's address starts after the newline.
                pos = end + usize::from(section.get(end) == Some(&b'\n'));
                end
            }
        };
        Some((start, &section[body_start..body_end]))
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Returns the KeyText record buffer and the `(start, limit)` range of
/// records within it.
pub(crate) fn keytext_buffer(file: &[u8], compressed: bool) -> Result<(Vec<u8>, usize, usize)> {
    let (start, limit) = bounds(file)?;
    if !compressed {
        let body = file
            .get(HEADER_LEN..)
            .ok_or(ContainerError::Truncated(HEADER_LEN))?;
        return Ok((body.to_vec(), start, limit.min(body.len())));
    }
    let mut at = HEADER_LEN + start;
    let stride = read_i32(file, at)?.max(0) as usize;
    at += 4;
    let mut chunk_start = at;
    let mut out = Vec::new();
    while at < HEADER_LEN + limit {
        let compressed_len = read_i32(file, at)?.max(0) as usize;
        let decompressed_len = read_i32(file, at + 4)?.max(0) as usize;
        if compressed_len == 0 && decompressed_len == 0 {
            break;
        }
        let payload = slice(file, at + 8, compressed_len.saturating_sub(4))?;
        out.extend_from_slice(&inflate(payload, at + 8, decompressed_len)?);
        chunk_start += stride.max(compressed_len + 4);
        at = chunk_start;
    }
    let len = out.len();
    Ok((out, 0, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_length_prefixed_and_bare_entries() {
        let mut section = Vec::new();
        let a = b"<d:entry id=\"a\">A</d:entry>";
        section.extend_from_slice(&(a.len() as u32).to_le_bytes());
        section.extend_from_slice(a);
        let b_at = section.len();
        section.extend_from_slice(b"<d:entry id=\"b\">B</d:entry>\n");
        section.extend_from_slice(&[0; 7]);

        let entries: Vec<_> = section_entries(&section).collect();
        assert_eq!(
            entries,
            [(0, &a[..]), (b_at, &b"<d:entry id=\"b\">B</d:entry>"[..])]
        );
    }
}
