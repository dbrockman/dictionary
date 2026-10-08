use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use dictdb::{DictInfo, DictWriter, Dictionary, EntrySpec, KeySpec};

const ENTRIES: u32 = 100_000;
const KEYS_PER_ENTRY: u32 = 3;

/// Deterministic pseudo-words, so prefixes have a realistic spread.
fn word(mut n: u32) -> String {
    const SYLLABLES: [&str; 16] = [
        "ka", "lo", "ber", "ti", "na", "mor", "sel", "qu", "an", "di", "pe", "ru", "st", "e", "o",
        "ing",
    ];
    let mut s = String::new();
    loop {
        s.push_str(SYLLABLES[(n % 16) as usize]);
        n /= 16;
        if n == 0 {
            return s;
        }
    }
}

fn build(path: &Path) {
    let mut w = DictWriter::create(path, DictInfo::default()).unwrap();
    for i in 0..ENTRIES {
        let head = word(i * 7919);
        let html = format!(
            "<h1>{head}</h1><p><i>noun</i></p><ol><li>The first sense of {head}, with an example.</li>\
             <li>A second, longer sense of {head} that goes on for a while to resemble real entries.</li></ol>"
        );
        let id = format!("id{i}");
        let e = w
            .add_entry(EntrySpec {
                id: &id,
                html: &html,
                ..Default::default()
            })
            .unwrap();
        for k in 0..KEYS_PER_ENTRY {
            let keyword = format!("{head}{}", ["", "s", "ed"][k as usize]);
            w.add_key(
                e,
                KeySpec {
                    keyword: &keyword,
                    ..Default::default()
                },
            );
        }
    }
    w.finish().unwrap();
}

fn benches(c: &mut Criterion) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bench.dictdb");
    build(&path);

    c.bench_function("open", |b| {
        b.iter(|| Dictionary::open(black_box(&path)).unwrap())
    });

    let d = Dictionary::open(&path).unwrap();
    println!(
        "keys: {}, entries: {}",
        d.meta().key_count,
        d.meta().entry_count
    );
    for q in ["k", "ka", "kalo", "kaloberti", "zzz"] {
        c.bench_function(&format!("search {q:?} limit 200"), |b| {
            b.iter(|| d.search(black_box(q), 200).unwrap())
        });
    }
    c.bench_function("entry_html (cold block)", |b| {
        let mut i = 0u32;
        b.iter(|| {
            // Stride through entries so most fetches miss the block cache.
            i = (i + 4099) % ENTRIES;
            d.entry_html(black_box(i)).unwrap()
        })
    });
    c.bench_function("entry_html (cached block)", |b| {
        b.iter(|| d.entry_html(black_box(42)).unwrap())
    });
}

criterion_group!(group, benches);
criterion_main!(group);
