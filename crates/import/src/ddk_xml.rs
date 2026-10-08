//! Importer for Dictionary Development Kit source projects: one XML file of
//! `<d:entry>` elements, usually next to an info plist and an
//! `OtherResources/` directory with images.

use std::fs;
use std::path::Path;

use dictdb::{DictWriter, KeySpec};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::bundle_info::{dict_info, read_plist};
use crate::error::{Error, IoContext, Result};
use crate::simplify::parse_entry;
use crate::{Import, copy_resources};

pub(crate) const SOURCE_KIND: &str = "ddk-xml";

/// Whether `path` looks like DDK source XML (a `d:dictionary` root element).
pub(crate) fn is_ddk_xml(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4096];
    let Ok(n) = fs::File::open(path).and_then(|mut f| f.read(&mut head)) else {
        return false;
    };
    let head = String::from_utf8_lossy(&head[..n]);
    head.contains("<d:dictionary") || head.contains("<d:entry")
}

pub(crate) fn import(xml_path: &Path, job: &mut Import<'_>) -> Result<()> {
    let dir = xml_path.parent().unwrap_or(Path::new("."));
    let stem = xml_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let plist = find_info_plist(dir);
    let info = dict_info(&plist.unwrap_or_default(), &stem, SOURCE_KIND);

    let source = fs::read_to_string(xml_path).at(xml_path)?;
    let mut writer = job.writer(info)?;
    let total = source.len().max(1) as f32;

    let mut reader = Reader::from_str(&source);
    let xml_error = |reader: &Reader<&[u8]>, source| Error::Xml {
        path: xml_path.to_owned(),
        position: reader.error_position(),
        source,
    };
    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event().map_err(|e| xml_error(&reader, e))? {
            Event::Start(e) if e.name().as_ref() == "d:entry" => {
                let end_name = e.name().as_ref().to_owned();
                reader
                    .read_to_end(quick_xml::name::QName(&end_name))
                    .map_err(|e| xml_error(&reader, e))?;
                let end = reader.buffer_position() as usize;
                add_entry(&mut writer, &source[start..end], job);
                (job.progress)(end as f32 / total);
            }
            Event::Eof => break,
            _ => {}
        }
    }

    copy_resources(&dir.join("OtherResources"), &mut writer)?;
    job.finish(writer)
}

fn add_entry(writer: &mut DictWriter, xhtml: &str, job: &mut Import<'_>) {
    let entry = match parse_entry(xhtml) {
        Ok(entry) => entry,
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
    for key in &entry.indexes {
        writer.add_key(
            entry_no,
            KeySpec {
                keyword: &key.value,
                title: &key.title,
                anchor: key.anchor.as_deref(),
                priority: 0,
                parental: key.parental,
            },
        );
        if let Some(yomi) = &key.yomi {
            writer.add_key(
                entry_no,
                KeySpec {
                    keyword: yomi,
                    title: &key.title,
                    anchor: key.anchor.as_deref(),
                    priority: 1,
                    parental: key.parental,
                },
            );
        }
    }
}

/// DDK projects keep their metadata in `MyInfo.plist` or similar.
fn find_info_plist(dir: &Path) -> Option<plist::Dictionary> {
    let mut candidates: Vec<_> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "plist"))
        .collect();
    candidates.sort();
    candidates
        .iter()
        .filter_map(|p| read_plist(p).ok())
        .find(|d| d.contains_key("CFBundleIdentifier"))
}
