//! Light/dark appearance and color themes: applying the configured themes,
//! following the operating system's appearance, and reloading theme files
//! when they change.

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::{
    IndexPath, Theme, ThemeConfig, ThemeMode, ThemeRegistry, WindowExt as _,
};
use gpui_kit::*;
use notify::Watcher as _;

use super::AppView;
use crate::config::Appearance;
use crate::themes::Themes;

/// Saving a file often produces several events in quick succession, and the
/// file can be incomplete in between. Changes are applied once this long
/// after the last event.
const RELOAD_DELAY: Duration = Duration::from_millis(200);

pub(super) type ThemeSelect = Entity<SelectState<Vec<SharedString>>>;

/// Theme state owned by [`AppView`].
pub(super) struct ThemeState {
    pub(super) themes: Themes,
    /// Why the configured light and dark themes are not in use, if so.
    pub(super) problems: [Option<String>; 2],
    /// Problems last shown as notifications, so each one appears once.
    reported: HashSet<String>,
    pub(super) light_select: ThemeSelect,
    pub(super) dark_select: ThemeSelect,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl ThemeState {
    pub(super) fn select(&self, mode: ThemeMode) -> &ThemeSelect {
        if mode.is_dark() {
            &self.dark_select
        } else {
            &self.light_select
        }
    }
}

fn default_themes(cx: &App) -> [Rc<ThemeConfig>; 2] {
    let registry = ThemeRegistry::global(cx);
    [
        registry.default_light_theme().clone(),
        registry.default_dark_theme().clone(),
    ]
}

impl AppView {
    /// Loads the themes, applies the configured ones and starts watching the
    /// themes folder. Problems are reported once the window can show them.
    pub(super) fn init_themes(
        themes_dir: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (ThemeState, Vec<Subscription>) {
        let themes = Themes::load(themes_dir, default_themes(cx));
        let new_select = |mode: ThemeMode, window: &mut Window, cx: &mut Context<Self>| {
            let select = cx.new(|cx| SelectState::new(Vec::new(), None, window, cx));
            let subscription = cx.subscribe_in(
                &select,
                window,
                move |this, _, event: &SelectEvent<Vec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(name)) = event {
                        this.set_theme(mode, name, window, cx);
                    }
                },
            );
            (select, subscription)
        };
        let (light_select, light_subscription) = new_select(ThemeMode::Light, window, cx);
        let (dark_select, dark_subscription) = new_select(ThemeMode::Dark, window, cx);
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            if this.settings.settings.appearance == Appearance::System {
                Theme::change(window.appearance(), Some(window), cx);
            }
        });
        let state = ThemeState {
            themes,
            problems: [None, None],
            reported: HashSet::new(),
            light_select,
            dark_select,
            _watcher: Self::watch_themes(themes_dir, window, cx),
        };
        // Notifications need the window's root, which exists only once this
        // view is built; a task runs after that.
        cx.spawn_in(window, async move |this, cx| {
            this.update_in(cx, |this, window, cx| {
                this.report_theme_problems(window, cx)
            })
            .ok();
        })
        .detach();
        (
            state,
            vec![light_subscription, dark_subscription, appearance],
        )
    }

    /// Calls [`Self::reload_themes`] whenever a file in `dir` changes. On
    /// failure the themes are still loaded at startup, just not reloaded.
    fn watch_themes(
        dir: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<notify::RecommendedWatcher> {
        // The folder is watched rather than each file, because many editors
        // save by replacing the file. It must exist to be watched.
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("{}: {e}; theme changes need a restart", dir.display());
            return None;
        }
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|event| !event.kind.is_access()) {
                tx.unbounded_send(()).ok();
            }
        })
        .and_then(|mut watcher| {
            watcher.watch(dir, notify::RecursiveMode::NonRecursive)?;
            Ok(watcher)
        });
        let watcher = match watcher {
            Ok(watcher) => watcher,
            Err(e) => {
                eprintln!("{}: {e}; theme changes need a restart", dir.display());
                return None;
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            while rx.next().await.is_some() {
                // Wait for the burst of events from one save to settle.
                loop {
                    cx.background_executor().timer(RELOAD_DELAY).await;
                    let mut more = false;
                    while rx.try_recv().is_ok() {
                        more = true;
                    }
                    if !more {
                        break;
                    }
                }
                if this
                    .update_in(cx, |this, window, cx| this.reload_themes(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Some(watcher)
    }

    fn reload_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.theme.themes.dir().to_owned();
        self.theme.themes = Themes::load(&dir, default_themes(cx));
        self.apply_themes(window, cx);
        self.sync_theme_selects(window, cx);
        self.report_theme_problems(window, cx);
    }

    /// The mode the app is in now: the configured one, or the system's.
    fn theme_mode(&self, window: &Window) -> ThemeMode {
        match self.settings.settings.appearance {
            Appearance::System => window.appearance().into(),
            Appearance::Light => ThemeMode::Light,
            Appearance::Dark => ThemeMode::Dark,
        }
    }

    /// Resolves the configured light and dark themes, falling back to the
    /// defaults, and makes them the active ones.
    pub(super) fn apply_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme.problems = [None, None];
        let settings = &self.settings.settings;
        let themes = &self.theme.themes;
        let mut resolve = |mode: ThemeMode, name: Option<&str>| {
            themes.resolve(name, mode).unwrap_or_else(|problem| {
                let default = themes.default_for(mode);
                self.theme.problems[usize::from(mode.is_dark())] =
                    Some(format!("{problem} Using {} instead.", default.name));
                default.clone()
            })
        };
        let light = resolve(ThemeMode::Light, settings.light_theme.as_deref());
        let dark = resolve(ThemeMode::Dark, settings.dark_theme.as_deref());
        let theme = Theme::global_mut(cx);
        theme.light_theme = light;
        theme.dark_theme = dark;
        Theme::change(self.theme_mode(window), Some(window), cx);
        cx.notify();
    }

    /// Shows theme problems that were not shown before as notifications.
    /// The settings page lists all current ones next to their controls.
    fn report_theme_problems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current: HashSet<String> = self
            .theme
            .themes
            .file_errors()
            .iter()
            .chain(self.theme.problems.iter().flatten())
            .cloned()
            .collect();
        for problem in current.difference(&self.theme.reported) {
            window.push_notification(Notification::error(problem.clone()), cx);
        }
        self.theme.reported = current;
    }

    /// Fills the theme pickers. Listing every theme parses the built-in
    /// ones, so it is done when the settings page opens, not at startup.
    pub(super) fn sync_theme_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let names = self.theme.themes.names(mode);
            let active = Theme::global(cx);
            let current = if mode.is_dark() {
                active.dark_theme.name.clone()
            } else {
                active.light_theme.name.clone()
            };
            let selected = names.iter().position(|name| *name == current);
            self.theme.select(mode).update(cx, |select, cx| {
                select.set_items(names, window, cx);
                select.set_selected_index(selected.map(IndexPath::new), window, cx);
            });
        }
    }

    pub(super) fn set_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.settings.appearance = appearance;
        self.save_settings(window, cx);
        Theme::change(self.theme_mode(window), Some(window), cx);
        cx.notify();
    }

    fn set_theme(
        &mut self,
        mode: ThemeMode,
        name: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The default needs no setting, which keeps the file minimal.
        let name = (*name != self.theme.themes.default_for(mode).name).then(|| name.to_string());
        let settings = &mut self.settings.settings;
        if mode.is_dark() {
            settings.dark_theme = name;
        } else {
            settings.light_theme = name;
        }
        self.save_settings(window, cx);
        self.apply_themes(window, cx);
        self.report_theme_problems(window, cx);
    }
}
