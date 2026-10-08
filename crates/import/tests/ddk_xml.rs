use dictdb::Dictionary;

#[test]
fn imports_ddk_project() {
    let library = tempfile::tempdir().unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ddk");

    let mut last_progress = 0.0;
    let report = import::import(&source, library.path(), &mut |p| last_progress = p).unwrap();
    assert_eq!(last_progress, 1.0);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.meta.name, "My Dictionary");
    assert_eq!(report.meta.identifier, "com.example.MyDictionary");
    assert_eq!(
        report.meta.copyright.as_deref(),
        Some("Copyright © 2026 Example.")
    );
    assert_eq!(report.meta.entry_count, 3);
    assert_eq!(
        report.path,
        library.path().join("com.example.MyDictionary.dictdb")
    );

    let d = Dictionary::open(&report.path).unwrap();
    let titles: Vec<_> = d
        .search("ma", 10)
        .unwrap()
        .into_iter()
        .map(|h| h.title)
        .collect();
    assert_eq!(titles, ["made", "make", "makes", "make it"]);

    let make_it = &d.lookup("make it").unwrap()[0];
    assert_eq!(make_it.anchor.as_deref(), Some("make_it"));
    assert!(make_it.parental);

    let html = d.entry_html(make_it.entry).unwrap();
    assert!(
        html.starts_with("<h1>make</h1> | māk | <h2>Definition</h2> <ol><li>form"),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"dict:find/fabricate\">fabricate</a>"),
        "{html}"
    );

    // Accent-insensitive search finds the accented entry once.
    let cafe = d.search("cafe", 10).unwrap();
    assert_eq!(cafe.len(), 1);
    assert!(
        d.entry_html(cafe[0].entry)
            .unwrap()
            .contains("light meals &amp; drinks")
    );

    // Cross-reference ids resolve, and images are copied.
    let app = d.lookup("dictionary").unwrap();
    assert_eq!(app[0].title, "dictionary (application)");
    let html = d.entry_html(app[0].entry).unwrap();
    assert!(
        html.contains("<a href=\"dict:entry/make_1\">make</a>"),
        "{html}"
    );
    assert!(
        html.contains("<img src=\"dict:res/Images/dictionary.png\" alt=\"icon\">"),
        "{html}"
    );
    assert_eq!(d.entry_by_id("make_1"), Some(make_it.entry));
    assert!(d.resources_dir().join("Images/dictionary.png").is_file());
}

#[test]
fn detects_sources() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    assert_eq!(
        import::detect(&fixtures.join("ddk")).unwrap(),
        import::Source::DdkXml(fixtures.join("ddk/MyDictionary.xml"))
    );
    assert!(import::detect(&fixtures.join("ddk/MyInfo.plist")).is_err());
}
