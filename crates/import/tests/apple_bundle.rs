//! Imports synthetic compiled bundles written in the layout the importer
//! expects, plus any real bundles placed in `testdata/` at the repo root.

use std::io::Write;
use std::path::{Path, PathBuf};

use dictdb::Dictionary;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use plist::{Dictionary as PDict, Value};

const ENTRIES: &[(&str, &str, &str)] = &[
    (
        "m_en_1",
        "make",
        r#"<span class="hg x_xh0"><span class="hw">make</span> <span class="prx">| māk |</span></span><span class="sg"><span class="se1 x_xd0"><span class="pos">verb</span> <span class="msDict x_xd1"><span class="sn">1</span> <span class="df">form by putting parts together</span></span><span id="m_en_1.007" class="subEntry x_xo1"><span class="l">make it</span> succeed</span></span></span>"#,
    ),
    (
        "m_en_2",
        "maker",
        r#"<span class="hg"><span class="hw">maker</span></span><span class="se1">a person who makes; see <a href="x-dictionary:r:m_en_1:com.example.dict">make</a></span>"#,
    ),
    (
        "m_en_3",
        "zebra",
        r#"<span class="hg"><span class="hw">zebra</span></span><span class="se1">a striped animal</span>"#,
    ),
];

/// (keyword, headword, entry index, anchor, flag)
const KEYS: &[(&str, &str, usize, &str, u16)] = &[
    ("make", "make", 0, "", 0),
    ("makes", "makes", 0, "", 0),
    ("made", "made (make)", 0, "", 0),
    (
        "make it",
        "make it",
        0,
        "xpointer(//*[@id='m_en_1.007'])",
        0b11,
    ), // priority 1, parental
    ("maker", "", 1, "", 0),
    ("zebra", "zebra", 2, "", 0x20), // second-language flag bit
];

#[derive(Clone, Copy)]
struct Layout {
    compressed: bool,
    chunk_offsets: bool,
    /// Header marker that moves data to offset 0x20.
    modern_header: bool,
    entries_per_section: usize,
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn i32le(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Wraps record data in the file header; returns (file, data start).
fn with_header(data: Vec<u8>, layout: Layout) -> Vec<u8> {
    let start: usize = if layout.modern_header { 0x20 } else { 0x4 };
    let mut file = vec![0u8; 0x40];
    file.extend_from_slice(&i32le((start + data.len()) as i32));
    if layout.modern_header {
        file.extend_from_slice(&i32le(0));
        file.extend_from_slice(&i32le(-1));
        file.resize(0x40 + start, 0);
    }
    file.extend_from_slice(&data);
    file
}

/// Writes Body.data; returns it and each entry's address.
fn body(layout: Layout) -> (Vec<u8>, Vec<(u32, u32)>) {
    let start = if layout.modern_header { 0x20 } else { 0x4 };
    let mut data = Vec::new();
    let mut addresses = Vec::new();
    for group in ENTRIES.chunks(layout.entries_per_section) {
        let section_offset = (start + data.len()) as u32;
        let mut section = Vec::new();
        for (id, title, body) in group {
            let xhtml = format!(
                r#"<d:entry xmlns:d="http://www.apple.com/DTDs/DictionaryService-1.0.rng" id="{id}" d:title="{title}" class="entry">{body}</d:entry>"#
            );
            addresses.push((section_offset, section.len() as u32));
            section.extend_from_slice(&(xhtml.len() as u32).to_le_bytes());
            section.extend_from_slice(xhtml.as_bytes());
        }
        let (payload, len) = if layout.compressed {
            let z = zlib(&section);
            let mut p = i32le(section.len() as i32).to_vec();
            p.extend_from_slice(&z);
            (p, z.len() + 4)
        } else {
            (section.clone(), section.len())
        };
        data.extend_from_slice(&i32le((8 + payload.len() - 4) as i32));
        data.extend_from_slice(&i32le(len as i32));
        data.extend_from_slice(&payload);
    }
    (with_header(data, layout), addresses)
}

fn utf16(s: &str) -> Vec<u8> {
    let bytes: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = (bytes.len() as u16).to_le_bytes().to_vec();
    out.extend_from_slice(&bytes);
    out
}

fn keytext(layout: Layout, addresses: &[(u32, u32)]) -> Vec<u8> {
    // One record per key, each with a single form.
    let mut records = Vec::new();
    for (keyword, headword, entry, anchor, flag) in KEYS {
        let (section, chunk) = addresses[*entry];
        let mut form = Vec::new();
        if layout.chunk_offsets {
            form.extend_from_slice(&chunk.to_le_bytes());
        }
        form.extend_from_slice(&section.to_le_bytes());
        form.extend_from_slice(&flag.to_le_bytes());
        for field in [*keyword, *headword, "", *anchor] {
            form.extend_from_slice(&utf16(field));
        }
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_le_bytes()); // form count
        body.extend_from_slice(&0u16.to_le_bytes()); // unknown
        body.extend_from_slice(&(form.len() as u16).to_le_bytes());
        body.extend_from_slice(&form);
        let mut record = Vec::new();
        if layout.compressed {
            record.extend_from_slice(&(body.len() as u32).to_le_bytes());
        } else {
            record.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
            record.extend_from_slice(&0u32.to_le_bytes());
        }
        record.extend_from_slice(&body);
        records.extend_from_slice(&record);
    }
    if !layout.compressed {
        return with_header(records, layout);
    }
    let mut data = i32le(0).to_vec(); // stride: chunks are packed
    for chunk in records.chunks(40) {
        let z = zlib(chunk);
        data.extend_from_slice(&i32le((z.len() + 4) as i32));
        data.extend_from_slice(&i32le(chunk.len() as i32));
        data.extend_from_slice(&z);
    }
    with_header(data, layout)
}

fn field(name: &str) -> Value {
    let mut d = PDict::new();
    d.insert("IDXDataFieldName".into(), name.into());
    Value::Dictionary(d)
}

fn info_plist(layout: Layout) -> Value {
    let mut fields = PDict::new();
    fields.insert(
        "IDXVariableDataFields".into(),
        Value::Array(
            ["DCSKeyword", "DCSHeadword", "DCSEntryTitle", "DCSAnchor"]
                .iter()
                .map(|n| field(n))
                .collect(),
        ),
    );
    fields.insert(
        "IDXFixedDataFields".into(),
        Value::Array(vec![field("DCSPrivateFlag")]),
    );
    let mut external = PDict::new();
    external.insert("IDXDataFieldName".into(), "DCSExternalBodyID".into());
    external.insert(
        "IDXDataSize".into(),
        Value::Integer(if layout.chunk_offsets { 8 } else { 4 }.into()),
    );
    fields.insert(
        "IDXExternalDataFields".into(),
        Value::Array(vec![Value::Dictionary(external)]),
    );

    let mut key_index = PDict::new();
    key_index.insert("IDXIndexName".into(), "DCSKeywordIndex".into());
    key_index.insert("IDXIndexDataFields".into(), Value::Dictionary(fields));
    let mut trie = PDict::new();
    trie.insert(
        "HeapDataCompressionType".into(),
        Value::Integer(i64::from(layout.compressed).into()),
    );
    key_index.insert("TrieAuxiliaryDataOptions".into(), Value::Dictionary(trie));

    let mut body_index = PDict::new();
    let body_compression = match (layout.compressed, layout.chunk_offsets) {
        (false, _) => 0,
        (true, false) => 1,
        (true, true) => 2,
    };
    body_index.insert(
        "HeapDataCompressionType".into(),
        Value::Integer(body_compression.into()),
    );

    let mut lang = PDict::new();
    lang.insert("DCSDictionaryIndexLanguage".into(), "en".into());
    lang.insert("DCSDictionaryDescriptionLanguage".into(), "en".into());

    let mut root = PDict::new();
    root.insert("CFBundleIdentifier".into(), "com.example.dict".into());
    root.insert("CFBundleDisplayName".into(), "Example Dictionary".into());
    root.insert("IDXDictionaryVersion".into(), Value::Integer(3.into()));
    root.insert(
        "IDXDictionaryIndexes".into(),
        Value::Array(vec![
            Value::Dictionary(key_index),
            Value::Dictionary(PDict::new()),
            Value::Dictionary(body_index),
        ]),
    );
    root.insert(
        "DCSDictionaryLanguages".into(),
        Value::Array(vec![Value::Dictionary(lang)]),
    );
    Value::Dictionary(root)
}

fn write_bundle(dir: &Path, layout: Layout) -> PathBuf {
    let bundle = dir.join("Example.dictionary");
    let resources = bundle.join("Contents/Resources");
    std::fs::create_dir_all(&resources).unwrap();
    info_plist(layout)
        .to_file_binary(bundle.join("Contents/Info.plist"))
        .unwrap();
    let (body, addresses) = body(layout);
    std::fs::write(resources.join("Body.data"), body).unwrap();
    std::fs::write(resources.join("KeyText.data"), keytext(layout, &addresses)).unwrap();
    bundle
}

fn check(layout: Layout) {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = write_bundle(tmp.path(), layout);
    let library = tmp.path().join("library");

    let report = import::import(&bundle, &library, &mut |_| {}).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.meta.name, "Example Dictionary");
    assert_eq!(report.meta.languages, ["en"]);
    assert_eq!(report.meta.entry_count, 3);

    let d = Dictionary::open(&report.path).unwrap();
    let titles: Vec<_> = d
        .search("ma", 10)
        .unwrap()
        .into_iter()
        .map(|h| h.title)
        .collect();
    assert_eq!(titles, ["made (make)", "make", "maker", "makes", "make it"]);

    let make_it = &d.lookup("make it").unwrap()[0];
    assert_eq!(make_it.anchor.as_deref(), Some("m_en_1.007"));
    assert_eq!(make_it.priority, 1);
    assert!(make_it.parental);
    assert_eq!(d.lookup("zebra").unwrap()[0].priority, 0);

    let html = d.entry_html(make_it.entry).unwrap();
    assert_eq!(
        html,
        "<div><h1>make</h1> | māk |</div><div><div><i>verb</i> <div><b>1</b> \
         form by putting parts together</div><div><b>make it</b> succeed</div></div></div>"
    );
    let maker = d.lookup("maker").unwrap();
    assert_eq!(maker[0].title, "maker");
    let html = d.entry_html(maker[0].entry).unwrap();
    assert!(
        html.contains("<a href=\"dict:entry/m_en_1\">make</a>"),
        "{html}"
    );
    assert_eq!(d.entry_by_id("m_en_1"), Some(make_it.entry));
}

#[test]
fn modern_compressed_bundle() {
    check(Layout {
        compressed: true,
        chunk_offsets: true,
        modern_header: true,
        entries_per_section: 2,
    });
}

#[test]
fn legacy_uncompressed_bundle() {
    check(Layout {
        compressed: false,
        chunk_offsets: false,
        modern_header: false,
        entries_per_section: 1,
    });
}

#[test]
fn detects_bundle_from_any_level() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = Layout {
        compressed: true,
        chunk_offsets: true,
        modern_header: true,
        entries_per_section: 2,
    };
    let bundle = write_bundle(tmp.path(), layout);
    let contents = bundle.join("Contents");
    let expected = import::Source::AppleBundle(contents.clone());
    assert_eq!(import::detect(&bundle).unwrap(), expected);
    assert_eq!(import::detect(&contents).unwrap(), expected);
    assert_eq!(
        import::detect(&contents.join("Resources/Body.data")).unwrap(),
        expected
    );
}

#[test]
fn rejects_undecodable_body() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = Layout {
        compressed: true,
        chunk_offsets: true,
        modern_header: true,
        entries_per_section: 2,
    };
    let bundle = write_bundle(tmp.path(), layout);
    let body_path = bundle.join("Contents/Resources/Body.data");
    let mut body = std::fs::read(&body_path).unwrap();
    // Scramble the first section's zlib stream, as an encrypted body would be.
    for b in &mut body[0x40 + 0x20 + 12..0x40 + 0x20 + 40] {
        *b ^= 0x5a;
    }
    std::fs::write(&body_path, body).unwrap();
    let err = import::import(&bundle, &tmp.path().join("lib"), &mut |_| {}).unwrap_err();
    assert!(matches!(err, import::Error::Unsupported { .. }), "{err}");
}

/// Imports every real bundle in `<repo>/data/` (git-ignored).
#[test]
fn real_bundles_in_data() {
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let Ok(dir) = std::fs::read_dir(&testdata) else {
        eprintln!("no data/ directory; skipping");
        return;
    };
    let library = tempfile::tempdir().unwrap();
    for bundle in dir.flatten().map(|e| e.path()) {
        if bundle.extension().is_none_or(|e| e != "dictionary") {
            continue;
        }
        let start = std::time::Instant::now();
        let report = import::import(&bundle, library.path(), &mut |_| {})
            .unwrap_or_else(|e| panic!("{}: {e}", bundle.display()));
        eprintln!(
            "{}: {} entries, {} keys in {:.1?}; {} warnings {:?}",
            bundle.display(),
            report.meta.entry_count,
            report.meta.key_count,
            start.elapsed(),
            report.warning_count,
            report.warnings
        );
        assert!(report.meta.entry_count > 0);
        assert!(report.meta.key_count > 0);
    }
}
