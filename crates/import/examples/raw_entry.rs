//! Prints the raw and simplified XHTML of entries in an Apple bundle whose
//! title equals TITLE.
//!
//! Usage: `cargo run -p import --example raw_entry -- <bundle.dictionary> <TITLE>`

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(bundle), Some(title)) = (args.next(), args.next()) else {
        return Err("usage: raw_entry <bundle.dictionary> <TITLE>".into());
    };
    let needle = format!("d:title=\"{title}\"");
    import::apple_bundle::for_each_raw_entry(bundle.as_ref(), |xhtml| {
        if xhtml.contains(&needle) {
            println!("--- raw ---\n{xhtml}\n");
            match import::simplify::parse_entry(xhtml) {
                Ok(e) => println!("--- simplified ---\n{}\n", e.html),
                Err(e) => println!("--- simplify error: {e}\n"),
            }
        }
        true
    })?;
    Ok(())
}
