//! User settings, stored as JSON in the platform's config directory.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const SETTINGS_FILE: &str = "settings.json";

/// The directory holding the settings file:
///
/// - `$XDG_CONFIG_HOME/dictionary` when `XDG_CONFIG_HOME` is set, on any platform;
/// - otherwise `~/.config/dictionary` on Linux and other Unixes,
///   `~/Library/Application Support/Dictionary` on macOS and
///   `%APPDATA%\Dictionary` on Windows.
pub fn config_dir() -> PathBuf {
    config_dir_from(|name| std::env::var_os(name))
}

fn config_dir_from(var: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    // The XDG spec says relative paths are invalid and must be ignored.
    let path_var = |name| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(dir) = path_var("XDG_CONFIG_HOME") {
        return dir.join("dictionary");
    }
    let home = path_var("HOME").unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Dictionary")
    } else if cfg!(windows) {
        path_var("APPDATA").unwrap_or(home).join("Dictionary")
    } else {
        home.join(".config/dictionary")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Identifiers of imported dictionaries left out of searches.
    pub disabled_dictionaries: BTreeSet<String>,
    /// Width of the result list in rems, once the user has resized it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_width: Option<f32>,
    /// Whether the app is light, dark or follows the system.
    pub appearance: Appearance,
    /// Name of the theme used when the app is light (see `themes.rs`), or
    /// `None` for GPUI Kit's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_theme: Option<String>,
    /// Name of the theme used when the app is dark.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dark_theme: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// Light or dark as the operating system is, switching when it does.
    #[default]
    System,
    Light,
    Dark,
}

/// The themes folder, next to the settings file.
pub fn themes_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("themes")
}

/// [`Settings`] together with the file they are saved to.
pub struct SettingsStore {
    path: PathBuf,
    pub settings: Settings,
}

impl SettingsStore {
    /// Loads settings from `dir`. A missing file gives the defaults; an
    /// unreadable one gives the defaults plus a message saying why.
    pub fn load(dir: &Path) -> (Self, Option<String>) {
        let path = dir.join(SETTINGS_FILE);
        let (settings, error) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(settings) => (settings, None),
                Err(e) => (
                    Settings::default(),
                    Some(format!("{}: {e}; using default settings", path.display())),
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (
                Settings::default(),
                Some(format!("{}: {e}; using default settings", path.display())),
            ),
        };
        (Self { path, settings }, error)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes the settings, replacing the file atomically.
    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let mut json = serde_json::to_vec_pretty(&self.settings).map_err(io::Error::other)?;
        json.push(b'\n');
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(vars: &[(&str, &str)]) -> PathBuf {
        config_dir_from(|name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        })
    }

    #[test]
    fn xdg_config_home_wins_on_every_platform() {
        assert_eq!(
            dir_with(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/me")]),
            Path::new("/xdg/dictionary")
        );
        // Relative values are ignored, as the XDG spec requires.
        assert_ne!(
            dir_with(&[("XDG_CONFIG_HOME", "xdg"), ("HOME", "/home/me")]),
            Path::new("xdg/dictionary")
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn defaults_to_dot_config_on_linux() {
        assert_eq!(
            dir_with(&[("HOME", "/home/me")]),
            Path::new("/home/me/.config/dictionary")
        );
    }

    #[test]
    fn round_trips_and_tolerates_missing_or_bad_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nested");

        let (mut store, error) = SettingsStore::load(&dir);
        assert!(error.is_none());
        assert_eq!(store.settings, Settings::default());

        store
            .settings
            .disabled_dictionaries
            .insert("com.example".into());
        store.settings.appearance = Appearance::Dark;
        store.settings.dark_theme = Some("Ayu Dark".into());
        store.save().unwrap();
        let json = std::fs::read_to_string(store.path()).unwrap();
        assert!(json.contains(r#""appearance": "dark""#), "{json}");
        let (reloaded, error) = SettingsStore::load(&dir);
        assert!(error.is_none());
        assert_eq!(reloaded.settings, store.settings);

        // Unknown keys from newer versions are ignored.
        std::fs::write(
            store.path(),
            r#"{"future": 1, "disabled_dictionaries": ["a"]}"#,
        )
        .unwrap();
        let (reloaded, _) = SettingsStore::load(&dir);
        assert_eq!(reloaded.settings.disabled_dictionaries.len(), 1);

        std::fs::write(store.path(), "not json").unwrap();
        let (reloaded, error) = SettingsStore::load(&dir);
        assert!(error.is_some());
        assert_eq!(reloaded.settings, Settings::default());
    }
}
