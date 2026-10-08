//! The set of imported dictionaries and searching across them.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
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
    /// Size on disk of each dictionary, parallel to `dicts`.
    sizes: Vec<u64>,
    /// Identifiers of dictionaries left out of searches across all dictionaries.
    disabled: BTreeSet<String>,
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
        let sizes = dicts.iter().map(|d| dir_size(d.path())).collect();
        (
            Self {
                dir: dir.to_owned(),
                dicts,
                sizes,
                disabled: BTreeSet::new(),
                images: RefCell::default(),
            },
            errors,
        )
    }

    pub fn reload(&mut self) -> Vec<String> {
        let (mut fresh, errors) = Self::open(&self.dir);
        fresh.disabled = std::mem::take(&mut self.disabled);
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

    /// Total size of the dictionary's files in bytes.
    pub fn size_on_disk(&self, dict: usize) -> u64 {
        self.sizes.get(dict).copied().unwrap_or(0)
    }

    pub fn position(&self, identifier: &str) -> Option<usize> {
        self.dicts
            .iter()
            .position(|d| d.meta().identifier == identifier)
    }

    pub fn is_enabled(&self, dict: usize) -> bool {
        self.dicts
            .get(dict)
            .is_some_and(|d| !self.disabled.contains(&d.meta().identifier))
    }

    /// Sets which dictionaries (by identifier) are left out of searches.
    pub fn set_disabled(&mut self, disabled: BTreeSet<String>) {
        self.disabled = disabled;
    }

    /// Removes a dictionary from the library and deletes its files.
    pub fn delete(&mut self, dict: usize) -> std::io::Result<()> {
        if dict >= self.dicts.len() {
            return Ok(());
        }
        let removed = self.dicts.remove(dict);
        self.sizes.remove(dict);
        let path = removed.path().to_owned();
        // Unmap the files first; Windows cannot delete mapped files.
        drop(removed);
        std::fs::remove_dir_all(path)
    }

    /// Prefix search over one dictionary (`scope`) or all enabled ones: exact
    /// matches, then headwords, then phrases (see [`Hit::tier`]), each by key
    /// and then by dictionary.
    pub fn search(&self, query: &str, scope: Option<usize>) -> Vec<Row> {
        let mut rows = Vec::new();
        for (index, dict) in self.dicts.iter().enumerate() {
            let included = match scope {
                Some(s) => s == index,
                None => !self.disabled.contains(&dict.meta().identifier),
            };
            if !included {
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

fn dir_size(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(m) if m.is_dir() => dir_size(&entry.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_dict(library: &Path, identifier: &str, word: &str) {
        let info = dictdb::DictInfo {
            name: identifier.to_uppercase(),
            identifier: identifier.into(),
            ..Default::default()
        };
        let mut w =
            dictdb::DictWriter::create(library.join(format!("{identifier}.dictdb")), info).unwrap();
        let entry = w.add_entry("", word).unwrap();
        w.add_key(
            entry,
            dictdb::KeySpec {
                keyword: word,
                ..Default::default()
            },
        );
        w.finish().unwrap();
    }

    #[test]
    fn disabled_dictionaries_are_only_searched_when_scoped() {
        let tmp = tempfile::tempdir().unwrap();
        write_dict(tmp.path(), "a", "apple");
        write_dict(tmp.path(), "b", "apricot");
        let (mut library, errors) = Library::open(tmp.path());
        assert!(errors.is_empty());
        assert!(library.size_on_disk(0) > 0);
        assert_eq!(library.search("ap", None).len(), 2);

        library.set_disabled(["b".to_owned()].into());
        assert!(library.is_enabled(0) && !library.is_enabled(1));
        let rows = library.search("ap", None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hit.title, "apple");
        assert_eq!(library.search("ap", Some(1)).len(), 1);

        // Reloading keeps the disabled set.
        library.reload();
        assert!(!library.is_enabled(1));
    }

    #[test]
    fn deletes_dictionary_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_dict(tmp.path(), "a", "apple");
        write_dict(tmp.path(), "b", "banana");
        let (mut library, _) = Library::open(tmp.path());
        let b = library.position("b").unwrap();
        library.delete(b).unwrap();
        assert!(!tmp.path().join("b.dictdb").exists());
        assert_eq!(library.dicts().len(), 1);
        assert_eq!(library.position("b"), None);
        assert!(library.search("banana", None).is_empty());
        assert_eq!(library.search("apple", None).len(), 1);
    }

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
