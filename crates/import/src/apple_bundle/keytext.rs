//! Search keys from `KeyText.data`.
//!
//! The record buffer (see [`super::container::keytext_buffer`]) is a run of
//! records `[u32 next]` (+ `[u32 len]` when uncompressed) `[u16 count]`,
//! followed by `count` word forms. The first form is preceded by an unknown
//! `u16`. Each form is:
//!
//! - one or more `u16`s, the first non-zero of which is the form's byte length
//!   counted from the end of that `u16`;
//! - the Body address: `[u32 chunk][u32 section]` when the dictionary uses
//!   chunk offsets, else `[u32 section]`;
//! - a `u16` `DCSPrivateFlag` when the plist lists it as a fixed field:
//!   `priority * 2 + parental`, plus `0x20` per language direction;
//! - the variable fields, in plist order, each `[u16 byte_len]` + UTF-16LE.
//!
//! The next record starts `next + 4` bytes after the current one.

use super::Properties;
use super::container::{Address, ContainerError, read_u16, read_u32};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct KeyRecord {
    pub address: Address,
    pub keyword: String,
    pub headword: String,
    pub entry_title: String,
    pub anchor: String,
    pub yomi: String,
    pub priority: u8,
    pub parental: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum KeyTextError {
    #[error(transparent)]
    Container(#[from] ContainerError),
    #[error("unexpected DCSPrivateFlag value {value:#x} at offset {offset:#x}")]
    BadFlag { value: u16, offset: usize },
    #[error("unsupported fixed key fields {0:?}")]
    FixedFields(Vec<String>),
    #[error("malformed record at offset {0:#x}")]
    Malformed(usize),
}

/// Parses all records in `buf[start..limit]`.
///
/// On an error, returns the records parsed so far alongside it, since a
/// partial key list is still useful.
pub(crate) fn parse(
    buf: &[u8],
    start: usize,
    limit: usize,
    props: &Properties,
) -> (Vec<KeyRecord>, Option<KeyTextError>) {
    let mut records = Vec::new();
    let error = parse_into(buf, start, limit, props, &mut records).err();
    (records, error)
}

fn parse_into(
    buf: &[u8],
    start: usize,
    limit: usize,
    props: &Properties,
    records: &mut Vec<KeyRecord>,
) -> Result<(), KeyTextError> {
    let has_flag = match props.key_fixed_fields.len() {
        0 => false,
        1 => true,
        _ => return Err(KeyTextError::FixedFields(props.key_fixed_fields.clone())),
    };
    let mut offset = start;
    while offset < limit {
        let next = read_u32(buf, offset)? as usize;
        let mut p = offset + 4;
        if !props.keytext_compressed {
            p += 4;
        }
        let count = read_u16(buf, p)?;
        p += 2;
        let mut next_form = None;
        for _ in 0..count {
            p = next_form.unwrap_or(p + 2);
            let mut form_len = 0;
            while form_len == 0 {
                form_len = read_u16(buf, p)? as usize;
                p += 2;
            }
            let form_end = p + form_len;
            if form_end > buf.len() {
                return Err(KeyTextError::Malformed(offset));
            }
            next_form = Some(form_end);

            let address = if props.body_chunk_offsets {
                let chunk = read_u32(buf, p)?;
                let section = read_u32(buf, p + 4)?;
                p += 8;
                Address { section, chunk }
            } else {
                let section = read_u32(buf, p)?;
                p += 4;
                Address { section, chunk: 0 }
            };

            let mut record = KeyRecord {
                address,
                ..Default::default()
            };
            if has_flag {
                let mut value = read_u16(buf, p)?;
                if value >= 0x40 {
                    return Err(KeyTextError::BadFlag { value, offset: p });
                }
                value %= 0x20;
                record.parental = value & 1 == 1;
                record.priority = (value >> 1) as u8;
                p += 2;
            }

            let mut field = 0;
            while p < form_end {
                let len = read_u16(buf, p)? as usize;
                p += 2;
                let bytes = buf.get(p..p + len).ok_or(KeyTextError::Malformed(offset))?;
                p += len;
                let value = decode_utf16le(bytes);
                match props.key_variable_fields.get(field).map(String::as_str) {
                    Some("DCSKeyword") => record.keyword = value,
                    Some("DCSHeadword") => record.headword = value,
                    Some("DCSEntryTitle") => record.entry_title = value,
                    Some("DCSAnchor") => record.anchor = value,
                    Some("DCSYomiWord") => record.yomi = value,
                    _ => {}
                }
                field += 1;
            }
            records.push(record);
        }
        offset += next + 4;
    }
    Ok(())
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}
