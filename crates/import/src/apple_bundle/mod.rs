//! Importer for compiled Apple `.dictionary` bundles.
//!
//! Apple does not document the compiled format. This implementation follows
//! the community reverse-engineering of it; see [`container`] and
//! [`keytext`] for the layouts. Only `Info.plist`, `Body.data` (entries) and
//! `KeyText.data` (search keys) are needed; the `*.index` tries are ignored
//! because the app builds its own index.

pub(crate) mod container;
pub(crate) mod keytext;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use dictdb::{DictWriter, KeySpec};
use plist::{Dictionary, Value};

use crate::bundle_info::{dict_info, read_plist};
use crate::error::{Error, IoContext, Result};
use crate::simplify::{anchor_id, parse_entry};
use crate::{Import, copy_resources};
use container::Address;
use keytext::KeyRecord;

pub(crate) const SOURCE_KIND: &str = "apple-bundle";

/// Share of the progress bar spent reading keys; the rest is entries.
const KEYTEXT_PROGRESS: f32 = 0.1;

/// Format details read from `Info.plist`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Properties {
    pub version: i64,
    pub body_compressed: bool,
    /// Whether KeyText addresses include an offset within the body section.
    pub body_chunk_offsets: bool,
    pub keytext_compressed: bool,
    pub key_fixed_fields: Vec<String>,
    pub key_variable_fields: Vec<String>,
}

impl Properties {
    pub(crate) fn from_plist(plist: &Dictionary) -> Self {
        let indexes: Vec<&Dictionary> = plist
            .get("IDXDictionaryIndexes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_dictionary)
            .collect();
        // The key index lists DCSKeyword among its fields; the body index is
        // conventionally the third.
        let key_index = indexes
            .iter()
            .copied()
            .find(|i| {
                field_names(i, "IDXVariableDataFields")
                    .iter()
                    .any(|f| f == "DCSKeyword")
            })
            .or_else(|| indexes.first().copied());
        let body_index = indexes.get(2).copied();

        let compression = |d: Option<&Dictionary>| {
            d.and_then(|d| d.get("HeapDataCompressionType"))
                .and_then(Value::as_signed_integer)
                .unwrap_or(0)
        };
        let body_compression = compression(body_index);
        let keytext_compression = compression(
            key_index
                .and_then(|i| i.get("TrieAuxiliaryDataOptions"))
                .and_then(Value::as_dictionary),
        );
        let external_size = key_index
            .and_then(|i| i.get("IDXIndexDataFields"))
            .and_then(Value::as_dictionary)
            .and_then(|f| f.get("IDXExternalDataFields"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_dictionary)
            .and_then(|f| f.get("IDXDataSize"))
            .and_then(Value::as_signed_integer);

        Properties {
            version: plist
                .get("IDXDictionaryVersion")
                .and_then(Value::as_signed_integer)
                .unwrap_or(-1),
            body_compressed: body_compression > 0,
            body_chunk_offsets: body_compression == 2 && external_size == Some(8),
            keytext_compressed: keytext_compression > 0,
            key_fixed_fields: key_index
                .map(|i| field_names(i, "IDXFixedDataFields"))
                .unwrap_or_default(),
            key_variable_fields: key_index
                .map(|i| field_names(i, "IDXVariableDataFields"))
                .unwrap_or_default(),
        }
    }
}

fn field_names(index: &Dictionary, kind: &str) -> Vec<String> {
    index
        .get("IDXIndexDataFields")
        .and_then(Value::as_dictionary)
        .and_then(|f| f.get(kind))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_dictionary)
        .filter_map(|f| f.get("IDXDataFieldName").and_then(Value::as_string))
        .map(str::to_owned)
        .collect()
}

/// Returns the bundle's `Contents` directory if `path` is a bundle, its
/// `Contents` directory or its `Resources` directory.
pub fn contents_dir(path: &Path) -> Option<PathBuf> {
    let candidates = [
        path.join("Contents"),
        path.to_owned(),
        path.parent().map(Path::to_owned).unwrap_or_default(),
    ];
    candidates
        .into_iter()
        .find(|c| c.join("Info.plist").is_file() && data_file(c, "Body.data").is_some())
}

fn data_file(contents: &Path, name: &str) -> Option<PathBuf> {
    data_dir(contents)
        .map(|dir| dir.join(name))
        .filter(|p| p.is_file())
}

/// The directory holding `Body.data` and friends: `Contents/`,
/// `Contents/Resources/`, or for dictionaries compiled once per language
/// (like Apple Dictionary) `Contents/Resources/<language>.lproj/`, chosen
/// to match the system language.
fn data_dir(contents: &Path) -> Option<PathBuf> {
    let resources = contents.join("Resources");
    for dir in [contents, &resources] {
        if dir.join("Body.data").is_file() {
            return Some(dir.to_owned());
        }
    }
    let mut localizations: Vec<String> = fs::read_dir(&resources)
        .ok()?
        .flatten()
        .filter(|e| e.path().join("Body.data").is_file())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_suffix(".lproj").map(str::to_owned)
        })
        .collect();
    localizations.sort();
    let pick = preferred_localization(&localizations, &system_language())?;
    Some(resources.join(format!("{pick}.lproj")))
}

/// The user's language from the POSIX locale variables, e.g. `sv_SE`.
fn system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .and_then(|v| v.split(['.', '@']).next().map(str::to_owned))
        .unwrap_or_else(|| "en".to_owned())
}

/// Picks the localization matching `language` (`sv_SE`, then `sv`), falling
/// back to English and then to any.
fn preferred_localization(available: &[String], language: &str) -> Option<String> {
    let base = language.split(['_', '-']).next().unwrap_or(language);
    [language, base, "en", "English"]
        .iter()
        .find_map(|want| available.iter().find(|a| a.as_str() == *want))
        .or_else(|| available.first())
        .cloned()
}

pub(crate) fn import(contents: &Path, job: &mut Import<'_>) -> Result<()> {
    let plist_path = contents.join("Info.plist");
    let mut plist = read_plist(&plist_path)?;
    // Per-language dictionaries carry their translated name in the language folder.
    if let Some(strings) = data_file(contents, "InfoPlist.strings")
        && let Ok(localized) = read_plist(&strings)
    {
        plist.extend(localized);
    }
    let props = Properties::from_plist(&plist);
    let bundle_name = contents
        .parent()
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let info = dict_info(&plist, &bundle_name, SOURCE_KIND);
    if !(1..=3).contains(&props.version) {
        job.warn(format!(
            "untested IDXDictionaryVersion {}; results may be incomplete",
            props.version
        ));
    }

    let body_path = data_file(contents, "Body.data").ok_or_else(|| Error::Unsupported {
        path: contents.to_owned(),
        reason: "no Body.data".into(),
    })?;
    let mut keys = read_keys(contents, &props, job)?;
    (job.progress)(KEYTEXT_PROGRESS);

    let body = fs::read(&body_path).at(&body_path)?;
    let mut writer = job.writer(info)?;
    let mut sections =
        container::body_sections(&body, props.body_compressed).map_err(|e| Error::Unsupported {
            path: body_path.clone(),
            reason: e.to_string(),
        })?;
    let mut entry_count = 0usize;
    while let Some(section) = sections.next() {
        let section = match section {
            Ok(s) => s,
            Err(e) if entry_count == 0 => {
                return Err(Error::Unsupported {
                    path: body_path,
                    reason: e.to_string(),
                });
            }
            Err(e) => {
                job.warn(format!("Body.data: stopped early: {e}"));
                break;
            }
        };
        for (chunk, xhtml) in container::section_entries(&section.data) {
            let address = Address {
                section: section.offset,
                chunk: chunk as u32,
            };
            let records = keys.remove(&address).unwrap_or_default();
            add_entry(&mut writer, &String::from_utf8_lossy(xhtml), &records, job);
            entry_count += 1;
        }
        (job.progress)(KEYTEXT_PROGRESS + sections.progress() * (1.0 - KEYTEXT_PROGRESS));
    }

    let unresolved: usize = keys.values().map(Vec::len).sum();
    if unresolved > 0 {
        job.warn(format!(
            "{unresolved} search keys point at entries that were not found in Body.data"
        ));
    }

    let resources = contents.join("Resources");
    copy_resources(
        if resources.is_dir() {
            &resources
        } else {
            contents
        },
        &mut writer,
    )?;
    job.finish(writer)
}

fn read_keys(
    contents: &Path,
    props: &Properties,
    job: &mut Import<'_>,
) -> Result<HashMap<Address, Vec<KeyRecord>>> {
    let mut by_address: HashMap<Address, Vec<KeyRecord>> = HashMap::new();
    let Some(path) = data_file(contents, "KeyText.data") else {
        job.warn("no KeyText.data; entries are searchable by title only".into());
        return Ok(by_address);
    };
    let file = fs::read(&path).at(&path)?;
    let (buf, start, limit) = match container::keytext_buffer(&file, props.keytext_compressed) {
        Ok(b) => b,
        Err(e) => {
            job.warn(format!(
                "KeyText.data unreadable ({e}); entries are searchable by title only"
            ));
            return Ok(by_address);
        }
    };
    let (records, error) = keytext::parse(&buf, start, limit, props);
    if let Some(e) = error {
        job.warn(format!(
            "KeyText.data: stopped after {} keys: {e}",
            records.len()
        ));
    }
    for record in records {
        by_address.entry(record.address).or_default().push(record);
    }
    Ok(by_address)
}

fn add_entry(writer: &mut DictWriter, xhtml: &str, keys: &[KeyRecord], job: &mut Import<'_>) {
    let entry = match parse_entry(xhtml) {
        Ok(e) => e,
        Err(e) => return job.warn(format!("skipped malformed entry: {e}")),
    };
    let entry_no = match writer.add_entry(&entry.id, &entry.html) {
        Ok(n) => n,
        Err(e) => return job.warn(format!("could not store entry {:?}: {e}", entry.id)),
    };
    writer.add_key(
        entry_no,
        KeySpec {
            keyword: &entry.title,
            ..Default::default()
        },
    );
    for key in keys {
        let anchor = (!key.anchor.is_empty()).then(|| anchor_id(&key.anchor));
        let title = if key.headword.is_empty() {
            &key.keyword
        } else {
            &key.headword
        };
        writer.add_key(
            entry_no,
            KeySpec {
                keyword: &key.keyword,
                title,
                anchor: anchor.as_deref(),
                priority: key.priority,
                parental: key.parental,
            },
        );
        if !key.yomi.is_empty() {
            writer.add_key(
                entry_no,
                KeySpec {
                    keyword: &key.yomi,
                    title,
                    anchor: anchor.as_deref(),
                    priority: key.priority.saturating_add(1),
                    parental: key.parental,
                },
            );
        }
    }
}

/// Calls `f` with the raw XHTML of every entry in the bundle, stopping when
/// it returns `false`. For debugging and tuning the simplifier.
pub fn for_each_raw_entry(bundle: &Path, mut f: impl FnMut(&str) -> bool) -> Result<()> {
    let contents = contents_dir(bundle).ok_or_else(|| Error::Unrecognized(bundle.to_owned()))?;
    let props = Properties::from_plist(&read_plist(&contents.join("Info.plist"))?);
    let body_path =
        data_file(&contents, "Body.data").ok_or_else(|| Error::Unrecognized(bundle.to_owned()))?;
    let body = fs::read(&body_path).at(&body_path)?;
    let unsupported = |e: container::ContainerError| Error::Unsupported {
        path: body_path.clone(),
        reason: e.to_string(),
    };
    for section in container::body_sections(&body, props.body_compressed).map_err(unsupported)? {
        let section = section.map_err(unsupported)?;
        for (_, xhtml) in container::section_entries(&section.data) {
            if !f(&String::from_utf8_lossy(xhtml)) {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::preferred_localization;

    #[test]
    fn picks_localization() {
        let available: Vec<String> = ["de", "en", "en_GB", "sv"].map(String::from).to_vec();
        assert_eq!(
            preferred_localization(&available, "en_GB").as_deref(),
            Some("en_GB")
        );
        assert_eq!(
            preferred_localization(&available, "sv_SE").as_deref(),
            Some("sv")
        );
        assert_eq!(
            preferred_localization(&available, "fi_FI").as_deref(),
            Some("en")
        );
        assert_eq!(
            preferred_localization(&["de".into()], "fi").as_deref(),
            Some("de")
        );
        assert_eq!(preferred_localization(&[], "en"), None);
    }
}
