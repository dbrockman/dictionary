use dictdb::{DictInfo, DictWriter, Dictionary, KeySpec};

fn info() -> DictInfo {
    DictInfo {
        name: "Test".into(),
        identifier: "com.example.test".into(),
        languages: vec!["en".into()],
        source_kind: "test".into(),
        copyright: None,
    }
}

fn key(keyword: &str) -> KeySpec<'_> {
    KeySpec {
        keyword,
        ..Default::default()
    }
}

#[test]
fn write_then_read() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.dictdb");

    let mut w = DictWriter::create(&path, info()).unwrap();
    let make = w
        .add_entry("make_1", "<h1>make</h1><p>to build</p>")
        .unwrap();
    let cafe = w
        .add_entry("cafe_1", "<h1>café</h1><p>a coffee house</p>")
        .unwrap();
    w.add_key(make, key("make"));
    w.add_key(
        make,
        KeySpec {
            keyword: "made",
            title: "made (make)",
            ..Default::default()
        },
    );
    w.add_key(
        make,
        KeySpec {
            keyword: "make it",
            anchor: Some("make_it"),
            parental: true,
            ..Default::default()
        },
    );
    w.add_key(cafe, key("Café"));
    w.add_key(cafe, key("café")); // duplicate after normalization
    let meta = w.finish().unwrap();
    assert_eq!(meta.entry_count, 2);
    assert_eq!(meta.key_count, 4);
    assert!(!tmp.path().join("test.dictdb.partial").exists());

    let d = Dictionary::open(&path).unwrap();
    assert_eq!(d.meta(), &meta);

    let titles: Vec<_> = d
        .search("MA", 10)
        .unwrap()
        .into_iter()
        .map(|h| h.title)
        .collect();
    assert_eq!(titles, ["made (make)", "make", "make it"]);

    let hits = d.search("make i", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].anchor.as_deref(), Some("make_it"));
    assert!(hits[0].parental);

    let hits = d.search("cafe", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Café");
    assert_eq!(
        d.entry_html(hits[0].entry).unwrap(),
        "<h1>café</h1><p>a coffee house</p>"
    );

    assert_eq!(d.search("m", 2).unwrap().len(), 2);
    assert!(d.search("x", 10).unwrap().is_empty());
    assert!(d.search("", 10).unwrap().is_empty());
    assert_eq!(d.lookup("Made").unwrap()[0].entry, make);
    assert_eq!(d.entry_by_id("cafe_1"), Some(cafe));
    assert_eq!(d.entry_by_id("nope"), None);
    assert!(d.entry_html(99).is_err());
}

#[test]
fn many_entries_span_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("big.dictdb");
    let mut w = DictWriter::create(&path, info()).unwrap();
    for i in 0..5000 {
        let word = format!("word{i:05}");
        let html = format!(
            "<h1>{word}</h1><p>{}</p>",
            "definition text ".repeat(i % 20)
        );
        let e = w.add_entry(&word, &html).unwrap();
        w.add_key(e, key(&word));
    }
    w.finish().unwrap();

    let d = Dictionary::open(&path).unwrap();
    for i in [0, 1, 2499, 4999] {
        let word = format!("word{i:05}");
        let hit = &d.lookup(&word).unwrap()[0];
        assert!(
            d.entry_html(hit.entry)
                .unwrap()
                .starts_with(&format!("<h1>{word}</h1>"))
        );
    }
    assert_eq!(d.search("word0", 10_000).unwrap().len(), 5000);
}

#[test]
fn empty_dictionary() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("empty.dictdb");
    DictWriter::create(&path, info()).unwrap().finish().unwrap();
    let d = Dictionary::open(&path).unwrap();
    assert!(d.search("a", 10).unwrap().is_empty());
}

#[test]
fn rewriting_replaces_previous_dictionary() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("d.dictdb");
    for word in ["first", "second"] {
        let mut w = DictWriter::create(&path, info()).unwrap();
        let e = w.add_entry("", word).unwrap();
        w.add_key(e, key(word));
        w.finish().unwrap();
    }
    let d = Dictionary::open(&path).unwrap();
    assert!(d.lookup("first").unwrap().is_empty());
    assert_eq!(d.lookup("second").unwrap().len(), 1);
}

#[test]
fn ranks_exact_matches_then_headwords_then_phrases() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rank.dictdb");
    let mut w = DictWriter::create(&path, info()).unwrap();
    let make = w.add_entry("make", "make").unwrap();
    let maker = w.add_entry("maker", "maker").unwrap();
    let making = w.add_entry("making", "making").unwrap();
    w.add_key(make, key("make"));
    for phrase in ["make a face", "make do", "make it"] {
        w.add_key(
            make,
            KeySpec {
                keyword: phrase,
                anchor: Some(phrase),
                ..Default::default()
            },
        );
    }
    w.add_key(maker, key("maker"));
    w.add_key(making, key("making"));
    // A multi-word headword is still a headword.
    let happy_hour = w.add_entry("happy_hour", "happy hour").unwrap();
    w.add_key(happy_hour, key("make hay"));
    w.finish().unwrap();

    let d = Dictionary::open(&path).unwrap();
    let titles = |q: &str, limit| -> Vec<String> {
        d.search(q, limit)
            .unwrap()
            .into_iter()
            .map(|h| h.title)
            .collect()
    };
    assert_eq!(
        titles("make", 10),
        [
            "make",
            "make hay",
            "maker",
            "make a face",
            "make do",
            "make it"
        ]
    );
    assert_eq!(
        titles("mak", 10),
        [
            "make",
            "make hay",
            "maker",
            "making",
            "make a face",
            "make do",
            "make it"
        ]
    );
    // An exact phrase match comes first.
    assert_eq!(titles("make do", 10), ["make do"]);
    // Limits keep the better tiers.
    assert_eq!(titles("mak", 3), ["make", "make hay", "maker"]);
    assert_eq!(
        titles("mak", 5),
        ["make", "make hay", "maker", "making", "make a face"]
    );
}
