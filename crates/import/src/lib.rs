//! Imports source dictionaries into the app's `dictdb` format.
//!
//! Supported sources:
//! - compiled Apple `.dictionary` bundles (see [`apple_bundle`]),
//! - Dictionary Development Kit source XML (see [`ddk_xml`]).

pub mod apple_bundle;
mod bundle_info;
pub mod ddk_xml;
mod error;
pub mod simplify;

use std::fs;
use std::path::{Path, PathBuf};

use dictdb::{DictInfo, DictWriter, Meta};

use error::IoContext;
pub use error::{Error, Result};

/// Number of warnings kept in a [`Report`]; the rest are only counted.
const MAX_WARNINGS: usize = 50;

/// File types copied from a source's resources into the dictionary.
const RESOURCE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];

/// Outcome of a successful import.
#[derive(Debug)]
pub struct Report {
    pub path: PathBuf,
    pub meta: Meta,
    /// Problems that did not stop the import, e.g. skipped entries.
    pub warnings: Vec<String>,
    pub warning_count: usize,
}

/// The kind of source found at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A compiled bundle; the path is its `Contents` directory.
    AppleBundle(PathBuf),
    /// DDK source; the path is the XML file.
    DdkXml(PathBuf),
}

/// Works out what kind of dictionary source `path` is.
pub fn detect(path: &Path) -> Result<Source> {
    if path.is_dir() {
        if let Some(contents) = apple_bundle::contents_dir(path) {
            return Ok(Source::AppleBundle(contents));
        }
        let mut xml: Vec<_> = fs::read_dir(path)
            .at(path)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml")))
            .filter(|p| ddk_xml::is_ddk_xml(p))
            .collect();
        xml.sort();
        if let Some(xml) = xml.into_iter().next() {
            return Ok(Source::DdkXml(xml));
        }
    } else if path.is_file() {
        if path.file_name().is_some_and(|n| n == "Body.data")
            && let Some(contents) = path.parent().and_then(apple_bundle::contents_dir)
        {
            return Ok(Source::AppleBundle(contents));
        }
        if ddk_xml::is_ddk_xml(path) {
            return Ok(Source::DdkXml(path.to_owned()));
        }
    }
    Err(Error::Unrecognized(path.to_owned()))
}

/// Imports the dictionary at `source` into `library_dir`, replacing any
/// earlier import of the same dictionary. `progress` receives values in 0..=1.
pub fn import(source: &Path, library_dir: &Path, progress: &mut dyn FnMut(f32)) -> Result<Report> {
    let mut job = Import {
        library_dir,
        progress,
        warnings: Vec::new(),
        warning_count: 0,
        output: None,
    };
    match detect(source)? {
        Source::AppleBundle(contents) => apple_bundle::import(&contents, &mut job)?,
        Source::DdkXml(xml) => ddk_xml::import(&xml, &mut job)?,
    }
    let (path, meta) = job.output.expect("importers finish their writer");
    (job.progress)(1.0);
    Ok(Report {
        path,
        meta,
        warnings: job.warnings,
        warning_count: job.warning_count,
    })
}

/// State shared by the importers during one import.
pub(crate) struct Import<'a> {
    library_dir: &'a Path,
    progress: &'a mut dyn FnMut(f32),
    warnings: Vec<String>,
    warning_count: usize,
    output: Option<(PathBuf, Meta)>,
}

impl Import<'_> {
    fn writer(&mut self, info: DictInfo) -> Result<DictWriter> {
        fs::create_dir_all(self.library_dir).at(self.library_dir)?;
        let path = self
            .library_dir
            .join(format!("{}.dictdb", file_stem(&info.identifier)));
        self.output = Some((path.clone(), Meta::default()));
        Ok(DictWriter::create(path, info)?)
    }

    fn finish(&mut self, writer: DictWriter) -> Result<()> {
        let meta = writer.finish()?;
        if let Some(output) = &mut self.output {
            output.1 = meta;
        }
        Ok(())
    }

    fn warn(&mut self, message: String) {
        self.warning_count += 1;
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(message);
        }
    }
}

/// A file-system-safe name derived from a dictionary identifier.
fn file_stem(identifier: &str) -> String {
    let stem: String = identifier
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim_matches('.');
    if stem.is_empty() {
        "dictionary".into()
    } else {
        stem.into()
    }
}

/// Recursively copies image files under `dir` into the dictionary's resources.
pub(crate) fn copy_resources(dir: &Path, writer: &mut DictWriter) -> Result<()> {
    fn walk(root: &Path, dir: &Path, writer: &mut DictWriter) -> Result<()> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, writer)?;
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| RESOURCE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                let bytes = fs::read(&path).at(&path)?;
                writer.add_resource(rel, &bytes)?;
            }
        }
        Ok(())
    }
    walk(dir, dir, writer)
}

#[cfg(test)]
mod tests {
    use super::file_stem;

    #[test]
    fn file_stems() {
        assert_eq!(
            file_stem("com.apple.dictionary.NOAD"),
            "com.apple.dictionary.NOAD"
        );
        assert_eq!(file_stem("My Dict/2"), "My_Dict_2");
        assert_eq!(file_stem(".."), "dictionary");
    }
}
