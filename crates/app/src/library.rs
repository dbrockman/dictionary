//! The set of imported dictionaries and searching across them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use base64::Engine as _;

use dictdb::{Dictionary, Hit};

/// Maximum number of results shown for a query.
pub const RESULT_LIMIT: usize = 300;

/// One search result.
#[derive(Debug, Clone)]
pub struct Row {
    /// Index of the dictionary in [`Library::dicts`].
    pub dict: usize,
    pub hit: Hit,
}

pub struct Library {
    dir: PathBuf,
    dicts: Vec<Dictionary>,
    /// Images inlined as `data:` URLs, by file path.
    images: RefCell<HashMap<PathBuf, Option<Rc<str>>>>,
}

impl Library {
    /// Where imported dictionaries are stored, following platform conventions.
    pub fn default_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("DICTIONARY_LIBRARY") {
            return dir.into();
        }
        let home = || {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
        };
        if cfg!(target_os = "macos") {
            home().join("Library/Application Support/Dictionary")
        } else if cfg!(windows) {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(home)
                .join("Dictionary")
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".local/share"))
                .join("dictionary")
        }
    }

    /// Opens every `*.dictdb` in `dir`. Dictionaries that fail to open are
    /// skipped and reported in the returned messages.
    pub fn open(dir: &Path) -> (Self, Vec<String>) {
        let mut errors = Vec::new();
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "dictdb") && p.is_dir())
            .collect();
        paths.sort();
        let mut dicts = Vec::new();
        for path in paths {
            match Dictionary::open(&path) {
                Ok(d) => dicts.push(d),
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }
        dicts.sort_by(|a, b| {
            a.meta()
                .name
                .to_lowercase()
                .cmp(&b.meta().name.to_lowercase())
        });
        (
            Self {
                dir: dir.to_owned(),
                dicts,
                images: RefCell::default(),
            },
            errors,
        )
    }

    pub fn reload(&mut self) -> Vec<String> {
        let (fresh, errors) = Self::open(&self.dir);
        *self = fresh;
        errors
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn dicts(&self) -> &[Dictionary] {
        &self.dicts
    }

    pub fn is_empty(&self) -> bool {
        self.dicts.is_empty()
    }

    /// Prefix search over one dictionary (`scope`) or all of them: exact
    /// matches, then headwords, then phrases (see [`Hit::tier`]), each by key
    /// and then by dictionary.
    pub fn search(&self, query: &str, scope: Option<usize>) -> Vec<Row> {
        let mut rows = Vec::new();
        for (index, dict) in self.dicts.iter().enumerate() {
            if scope.is_some_and(|s| s != index) {
                continue;
            }
            match dict.search(query, RESULT_LIMIT) {
                Ok(hits) => rows.extend(hits.into_iter().map(|hit| Row { dict: index, hit })),
                Err(e) => eprintln!("{}: search failed: {e}", dict.path().display()),
            }
        }
        if scope.is_none() && self.dicts.len() > 1 {
            let query = dictdb::normalize_key(query);
            // Stable, so equal keys keep dictionary order.
            rows.sort_by(|a, b| {
                (a.hit.tier(&query), &a.hit.key).cmp(&(b.hit.tier(&query), &b.hit.key))
            });
            rows.truncate(RESULT_LIMIT);
        }
        rows
    }

    /// Finds the entry with source id `id`, preferring dictionary `prefer`.
    pub fn entry_by_id(&self, id: &str, prefer: Option<usize>) -> Option<(usize, u32)> {
        let preferred = prefer.into_iter();
        let others = (0..self.dicts.len()).filter(|&i| Some(i) != prefer);
        preferred
            .chain(others)
            .find_map(|i| self.dicts.get(i)?.entry_by_id(id).map(|e| (i, e)))
    }

    /// The entry's display HTML, with images from the dictionary's resources
    /// inlined (the text view only loads `data:` and HTTP images).
    pub fn entry_html(&self, dict: usize, entry: u32) -> Result<String, String> {
        let d = self
            .dicts
            .get(dict)
            .ok_or("dictionary no longer available")?;
        let html = d.entry_html(entry).map_err(|e| e.to_string())?;
        if !html.contains(import::simplify::RESOURCE_LINK) {
            return Ok(html);
        }
        Ok(self.inline_images(&html, &d.resources_dir()))
    }

    fn inline_images(&self, html: &str, resources: &Path) -> String {
        let link = import::simplify::RESOURCE_LINK;
        let mut out = String::with_capacity(html.len());
        let mut rest = html;
        while let Some(start) = rest.find(link) {
            out.push_str(&rest[..start]);
            let after = &rest[start + link.len()..];
            let end = after.find('"').unwrap_or(after.len());
            match self.image_data_url(resources, &after[..end]) {
                Some(url) => out.push_str(&url),
                None => out.push_str(&rest[start..start + link.len() + end]),
            }
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    }

    fn image_data_url(&self, resources: &Path, rel: &str) -> Option<Rc<str>> {
        let rel = Path::new(rel);
        // Only files inside the dictionary's own resources directory.
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return None;
        }
        let path = resources.join(rel);
        self.images
            .borrow_mut()
            .entry(path)
            .or_insert_with_key(|path| {
                let mime = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
                    "png" => "image/png",
                    "jpg" | "jpeg" => "image/jpeg",
                    "gif" => "image/gif",
                    "webp" => "image/webp",
                    "svg" => "image/svg+xml",
                    "bmp" => "image/bmp",
                    _ => return None,
                };
                let bytes = std::fs::read(path).ok()?;
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                Some(format!("data:{mime};base64,{data}").into())
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inlines_resource_images_only_from_the_resources_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let resources = tmp.path().join("resources");
        std::fs::create_dir_all(resources.join("Images")).unwrap();
        std::fs::write(resources.join("Images/a.png"), b"\x89PNG").unwrap();
        std::fs::write(tmp.path().join("secret.png"), b"secret").unwrap();
        let (library, _) = Library::open(tmp.path());

        let html = library.inline_images(
            r#"<img src="dict:res/Images/a.png"><img src="dict:res/../secret.png"><img src="dict:res/missing.png">"#,
            &resources,
        );
        assert_eq!(
            html,
            r#"<img src="data:image/png;base64,iVBORw=="><img src="dict:res/../secret.png"><img src="dict:res/missing.png">"#
        );
    }
}
