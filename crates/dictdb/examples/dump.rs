//! Prints a dictionary's metadata, then the hits and entry HTML for a word.
//!
//! Usage: `cargo run -p dictdb --example dump -- <path.dictdb> [word]`

use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: dump <path.dictdb> [word]")?;
    let word = args.next();

    let start = Instant::now();
    let dict = dictdb::Dictionary::open(&path)?;
    println!("opened in {:?}", start.elapsed());
    println!("{:#?}", dict.meta());

    let Some(word) = word else { return Ok(()) };
    let start = Instant::now();
    let hits = dict.search(&word, 20)?;
    let first = start.elapsed();
    let start = Instant::now();
    dict.search(&word, 20)?;
    println!(
        "\nsearch {word:?}: {} hits in {first:?} (again: {:?})",
        hits.len(),
        start.elapsed()
    );
    for hit in &hits {
        println!(
            "  {:<30} entry {:>7} {:<20} priority {}  anchor {:?}",
            hit.title,
            hit.entry,
            format!(
                "{} {}",
                hit.entry_title.as_deref().unwrap_or("-"),
                hit.entry_detail.as_deref().unwrap_or("")
            ),
            hit.priority,
            hit.anchor
        );
    }
    if let Some(hit) = hits.first() {
        let start = Instant::now();
        let html = dict.entry_html(hit.entry)?;
        println!(
            "\nentry {} ({} bytes, {:?}):\n{html}",
            hit.entry,
            html.len(),
            start.elapsed()
        );
    }
    Ok(())
}
