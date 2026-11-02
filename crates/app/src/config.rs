//! User settings, stored as JSON in the platform's config directory, and app
//! state that persists across sessions, stored in its state directory.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "settings.json";
pub const STATE_FILE: &str = "state.json";

/// The directory holding the settings file:
///
/// - `$XDG_CONFIG_HOME/dictionary` when `XDG_CONFIG_HOME` is set, on any platform;
/// - otherwise `~/.config/dictionary` on Linux and other Unixes,
///   `~/Library/Application Support/Dictionary` on macOS and
///   `%APPDATA%\Dictionary` on Windows.
pub fn config_dir() -> PathBuf {
    config_dir_from(|name| std::env::var_os(name))
}

/// The directory holding the state file:
///
/// - `$XDG_STATE_HOME/dictionary` when `XDG_STATE_HOME` is set, on any platform;
/// - otherwise `~/.local/state/dictionary` on Linux and other Unixes,
///   `~/Library/Application Support/Dictionary` on macOS and
///   `%LOCALAPPDATA%\Dictionary` on Windows.
pub fn state_dir() -> PathBuf {
    state_dir_from(|name| std::env::var_os(name))
}

fn config_dir_from(var: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    app_dir_from(var, "XDG_CONFIG_HOME", ".config", "APPDATA")
}

fn state_dir_from(var: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    app_dir_from(var, "XDG_STATE_HOME", ".local/state", "LOCALAPPDATA")
}

/// The app's directory under the XDG base directory `xdg_var`, which defaults
/// to `unix_default` under the home directory, or the platform's equivalent.
fn app_dir_from(
    var: impl Fn(&str) -> Option<OsString>,
    xdg_var: &str,
    unix_default: &str,
    windows_var: &str,
) -> PathBuf {
    // The XDG spec says relative paths are invalid and must be ignored.
    let path_var = |name| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(dir) = path_var(xdg_var) {
        return dir.join("dictionary");
    }
    let home = path_var("HOME").unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Dictionary")
    } else if cfg!(windows) {
        path_var(windows_var).unwrap_or(home).join("Dictionary")
    } else {
        home.join(unix_default).join("dictionary")
    }
}

/// What the user chose in the settings page, or by editing the file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Identifiers of imported dictionaries left out of searches.
    pub disabled_dictionaries: BTreeSet<String>,
    /// Names the user gave imported dictionaries, by identifier, shown in
    /// place of the names they came with.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub dictionary_names: BTreeMap<String, String>,
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

/// How the user left the window, restored at the next start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Identifier of the dictionary searches were last limited to, or `None`
    /// for all of them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Width of the result list in rems, once the user has resized it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_width: Option<f32>,
}

/// The themes folder, next to the settings file.
pub fn themes_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("themes")
}

pub type SettingsStore = Store<Settings>;
pub type StateStore = Store<State>;

/// A value together with the JSON file it is saved to. It dereferences to
/// the value.
pub struct Store<T> {
    path: PathBuf,
    value: T,
}

impl<T: Default + Serialize + DeserializeOwned> Store<T> {
    /// Loads the value from `path`. A missing file gives the default; an
    /// unreadable one gives the default plus a message saying why.
    pub fn load(path: PathBuf) -> (Self, Option<String>) {
        let error =
            |e: &dyn std::fmt::Display| Some(format!("{}: {e}; using defaults", path.display()));
        let (value, error) = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(value) => (value, None),
                Err(e) => (T::default(), error(&e)),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (T::default(), None),
            Err(e) => (T::default(), error(&e)),
        };
        (Self { path, value }, error)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes the value, replacing the file atomically.
    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let mut json = serde_json::to_vec_pretty(&self.value).map_err(io::Error::other)?;
        json.push(b'\n');
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)
    }
}

impl<T> Deref for Store<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Store<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn xdg_dirs_win_on_every_platform() {
        let env = [
            ("XDG_CONFIG_HOME", "/xdg"),
            ("XDG_STATE_HOME", "/xdg-state"),
            ("HOME", "/home/me"),
        ];
        assert_eq!(config_dir_from(vars(&env)), Path::new("/xdg/dictionary"));
        assert_eq!(
            state_dir_from(vars(&env)),
            Path::new("/xdg-state/dictionary")
        );
        // Relative values are ignored, as the XDG spec requires.
        assert_ne!(
            config_dir_from(vars(&[("XDG_CONFIG_HOME", "xdg"), ("HOME", "/home/me")])),
            Path::new("xdg/dictionary")
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn defaults_to_dot_config_and_dot_local_state_on_linux() {
        let env = [("HOME", "/home/me")];
        assert_eq!(
            config_dir_from(vars(&env)),
            Path::new("/home/me/.config/dictionary")
        );
        assert_eq!(
            state_dir_from(vars(&env)),
            Path::new("/home/me/.local/state/dictionary")
        );
    }

    #[test]
    fn round_trips_and_tolerates_missing_or_bad_files() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join(SETTINGS_FILE);

        let (mut store, error) = SettingsStore::load(path.clone());
        assert!(error.is_none());
        assert_eq!(*store, Settings::default());

        store.disabled_dictionaries.insert("com.example".into());
        store
            .dictionary_names
            .insert("com.example".into(), "Example".into());
        store.appearance = Appearance::Dark;
        store.dark_theme = Some("Ayu Dark".into());
        store.save().unwrap();
        let json = std::fs::read_to_string(store.path()).unwrap();
        assert!(json.contains(r#""appearance": "dark""#), "{json}");
        let (reloaded, error) = SettingsStore::load(path.clone());
        assert!(error.is_none());
        assert_eq!(*reloaded, *store);

        // Unknown keys from newer versions are ignored.
        std::fs::write(
            store.path(),
            r#"{"future": 1, "disabled_dictionaries": ["a"]}"#,
        )
        .unwrap();
        let (reloaded, _) = SettingsStore::load(path.clone());
        assert_eq!(reloaded.disabled_dictionaries.len(), 1);

        std::fs::write(store.path(), "not json").unwrap();
        let (reloaded, error) = SettingsStore::load(path);
        assert!(error.is_some());
        assert_eq!(*reloaded, Settings::default());
    }

    #[test]
    fn state_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(STATE_FILE);
        let (mut store, _) = StateStore::load(path.clone());
        store.scope = Some("com.example".into());
        store.results_width = Some(18.5);
        store.save().unwrap();
        let (reloaded, error) = StateStore::load(path);
        assert!(error.is_none());
        assert_eq!(*reloaded, *store);
    }
}
