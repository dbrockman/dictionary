//! The main window: search field, result list and definition pane.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Escape, Input, InputEvent, InputState, MoveDown, MoveUp, SelectAll,
};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{
    ResizablePanelEvent, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, StyledExt as _, TitleBar,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::Deserialize;

use crate::config::SettingsStore;
use crate::history::{History, Page};
use crate::library::{Library, Row};

mod settings_page;

actions!(
    dictionary,
    [
        FocusSearch,
        Back,
        Forward,
        ToggleSettings,
        CloseSettings,
        NextScope,
        PreviousScope,
        Quit
    ]
);

/// Selects the scope tab at this position: 0 searches all dictionaries, 1 and
/// up only the first, second, … enabled one.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = dictionary, no_json)]
pub struct SelectScope(pub usize);

const KEY_CONTEXT: &str = "Dictionary";
const ROW_HEIGHT: f32 = 30.;
/// Default width of the result list, in rems.
const RESULTS_WIDTH: f32 = 18.;
/// Widths the result list can be resized to, in rems.
const RESULTS_WIDTH_RANGE: Range<f32> = 12.0..32.0;
/// The narrowest the definition pane gets, in rems.
const DEFINITION_MIN_WIDTH: f32 = 20.;
/// Longer dictionary names are truncated in the scope tabs, in rems.
const SCOPE_TAB_MAX_WIDTH: f32 = 14.;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-l", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-left", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-right", Forward, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-[", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-]", Forward, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-,", ToggleSettings, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CloseSettings, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-tab", NextScope, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PreviousScope, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
    // Secondary-1 is "All", like the first tab in a browser.
    cx.bind_keys((1..=9).map(|n| {
        KeyBinding::new(
            &format!("secondary-{n}"),
            SelectScope(n - 1),
            Some(KEY_CONTEXT),
        )
    }));
}

/// The entry currently shown in the definition pane.
struct Shown {
    page: Page,
    /// The entry's HTML, or why it could not be loaded.
    html: Result<SharedString, SharedString>,
}

/// What importing one path came to: a success or an error message.
type ImportOutcome = Result<String, String>;

/// A running import, shared with its background thread.
struct ImportJob {
    progress: Arc<AtomicU32>,
}

impl ImportJob {
    /// Fraction done, in 0..=1.
    fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Dictionary,
    Settings,
}

pub struct AppView {
    mode: Mode,
    library: Library,
    settings: SettingsStore,
    search: Entity<InputState>,
    rows: Vec<Row>,
    selected: Option<usize>,
    /// Restricts searches to one dictionary.
    scope: Option<usize>,
    shown: Option<Shown>,
    history: History,
    list_scroll: UniformListScrollHandle,
    scope_scroll: ScrollHandle,
    /// Sizes of the result list and definition panes.
    split: Entity<ResizableState>,
    /// Keeps app shortcuts working when focus is not in the search field.
    focus_handle: FocusHandle,
    import: Option<ImportJob>,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    pub fn new(
        mut library: Library,
        settings: SettingsStore,
        initial_query: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .clean_on_escape()
        });
        let split = cx.new(|_| ResizableState::default());
        let subscriptions = vec![
            cx.subscribe_in(&search, window, Self::on_search_event),
            cx.subscribe_in(&split, window, Self::on_split_resized),
        ];
        search.update(cx, |state, cx| state.focus(window, cx));

        library.set_disabled(settings.settings.disabled_dictionaries.clone());
        let mut this = Self {
            mode: Mode::Dictionary,
            library,
            settings,
            search,
            rows: Vec::new(),
            selected: None,
            scope: None,
            shown: None,
            history: History::default(),
            list_scroll: UniformListScrollHandle::new(),
            scope_scroll: ScrollHandle::new(),
            split,
            focus_handle: cx.focus_handle(),
            import: None,
            _subscriptions: subscriptions,
        };
        if let Some(query) = initial_query {
            this.set_query(&query, window, cx);
        }
        this
    }

    fn on_search_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => self.run_search(cx),
            InputEvent::PressEnter { .. } => {
                if let Some(ix) = self.selected {
                    self.open_row(ix, true, cx);
                }
            }
            _ => {}
        }
    }

    /// Remembers the result list width the user dragged it to.
    fn on_split_resized(
        &mut self,
        split: &Entity<ResizableState>,
        _: &ResizablePanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(&width) = split.read(cx).sizes().first() else {
            return;
        };
        let rems = (width / window.rem_size() * 10.).round() / 10.;
        self.settings.settings.results_width = Some(rems);
        self.save_settings(window, cx);
    }

    fn query(&self, cx: &App) -> SharedString {
        self.search.read(cx).value()
    }

    /// Replaces the search text and shows its results.
    fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |state, cx| {
            state.set_value(query.to_owned(), window, cx)
        });
        self.run_search(cx);
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.rows = self.library.search(&query, self.scope);
        self.selected = None;
        if self.rows.is_empty() {
            self.shown = None;
        } else {
            self.select(0, false, cx);
        }
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Selects a result row and shows its entry. `explicit` selections (a
    /// click or Enter) are recorded in the history.
    fn select(&mut self, ix: usize, explicit: bool, cx: &mut Context<Self>) {
        if ix >= self.rows.len() {
            return;
        }
        self.selected = Some(ix);
        self.list_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        self.open_row(ix, explicit, cx);
    }

    fn open_row(&mut self, ix: usize, record: bool, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        let page = Page {
            dict: row.dict,
            entry: row.hit.entry,
        };
        self.show(page, record, cx);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let ix = match self.selected {
            Some(ix) => ix.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.select(ix, false, cx);
    }

    /// Shows `page`; when `record` is set the previous page goes into history.
    fn show(&mut self, page: Page, record: bool, cx: &mut Context<Self>) {
        if self.shown.as_ref().is_some_and(|s| s.page == page) {
            return;
        }
        // A failure is shown in the definition pane, where the entry would be.
        let html = self
            .library
            .entry_html(page.dict, page.entry)
            .map(SharedString::from)
            .map_err(SharedString::from);
        if record && let Some(previous) = self.shown.take() {
            self.history.push(previous.page);
        }
        self.sync_selection(&page);
        self.shown = Some(Shown { page, html });
        cx.notify();
    }

    /// Keeps the highlighted result in step with the entry on screen, e.g.
    /// after going back or following a cross-reference.
    fn sync_selection(&mut self, page: &Page) {
        let shows = |row: &Row| row.dict == page.dict && row.hit.entry == page.entry;
        if self
            .selected
            .and_then(|ix| self.rows.get(ix))
            .is_some_and(shows)
        {
            return;
        }
        self.selected = self.rows.iter().position(shows);
        if let Some(ix) = self.selected {
            self.list_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
    }

    fn follow_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        // The clicked view is replaced by the new entry, taking focus with it.
        if !self.search.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }
        use import::simplify::{ENTRY_LINK, FIND_LINK};
        if let Some(word) = url.strip_prefix(FIND_LINK) {
            self.remember_current();
            self.set_query(word, window, cx);
        } else if let Some(id) = url.strip_prefix(ENTRY_LINK) {
            let prefer = self.shown.as_ref().map(|s| s.page.dict);
            match self.library.entry_by_id(id, prefer) {
                Some((dict, entry)) => self.show(Page { dict, entry }, true, cx),
                None => window.push_notification(
                    Notification::error("Couldn’t find the linked entry in any dictionary."),
                    cx,
                ),
            }
        } else if url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("mailto:")
        {
            cx.open_url(url);
        }
    }

    /// Moves the current page into history before a navigation that does not
    /// go through [`Self::show`] with `record` set.
    fn remember_current(&mut self) {
        if let Some(shown) = self.shown.take() {
            self.history.push(shown.page);
        }
    }

    fn go_back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        let current = self.shown.as_ref().map(|s| s.page.clone());
        if let Some(page) = self.history.back(current) {
            self.shown = None;
            self.show(page, false, cx);
        }
    }

    fn go_forward(&mut self, _: &Forward, _: &mut Window, cx: &mut Context<Self>) {
        let current = self.shown.as_ref().map(|s| s.page.clone());
        if let Some(page) = self.history.forward(current) {
            self.shown = None;
            self.show(page, false, cx);
        }
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Dictionary;
        self.search.update(cx, |state, cx| state.focus(window, cx));
        window.dispatch_action(Box::new(SelectAll), cx);
    }

    /// Typing while the search field is unfocused starts a new search.
    fn type_to_search(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = &event.keystroke.modifiers;
        if self.mode != Mode::Dictionary
            || modifiers.control
            || modifiers.alt
            || modifiers.platform
            || modifiers.function
        {
            return;
        }
        let Some(text) = event
            .keystroke
            .key_char
            .as_deref()
            .filter(|t| !t.trim().is_empty() && !t.chars().any(char::is_control))
        else {
            return;
        };
        if self.search.focus_handle(cx).is_focused(window) {
            return;
        }
        let text = text.to_owned();
        self.search.update(cx, |state, cx| state.focus(window, cx));
        self.set_query(&text, window, cx);
        cx.stop_propagation();
    }

    /// The scope tab that is selected: 0 for "All", then one per dictionary
    /// in [`Self::enabled_dicts`].
    fn scope_tab(&self, enabled: &[usize]) -> usize {
        self.scope
            .and_then(|scope| enabled.iter().position(|&ix| ix == scope))
            .map_or(0, |position| position + 1)
    }

    fn select_scope(&mut self, action: &SelectScope, _: &mut Window, cx: &mut Context<Self>) {
        let enabled = self.enabled_dicts();
        if self.mode != Mode::Dictionary || enabled.len() < 2 || action.0 > enabled.len() {
            return;
        }
        let tab = action.0;
        self.scope = tab.checked_sub(1).map(|position| enabled[position]);
        self.scope_scroll.scroll_to_item(tab);
        self.run_search(cx);
    }

    fn next_scope(&mut self, _: &NextScope, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = self.enabled_dicts();
        let tab = (self.scope_tab(&enabled) + 1) % (enabled.len() + 1);
        self.select_scope(&SelectScope(tab), window, cx);
    }

    fn previous_scope(&mut self, _: &PreviousScope, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = self.enabled_dicts();
        let tabs = enabled.len() + 1;
        let tab = (self.scope_tab(&enabled) + tabs - 1) % tabs;
        self.select_scope(&SelectScope(tab), window, cx);
    }

    /// Asks for dictionaries to import, then imports them in the background.
    fn add_dictionary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.import.is_some() {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.start_import(paths, window, cx))
                .ok();
        })
        .detach();
    }

    /// Imports `paths` one after another on a background thread. Progress
    /// shows next to the Add button; the outcome arrives as notifications.
    fn start_import(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let progress = Arc::new(AtomicU32::new(0));
        let outcome: Arc<Mutex<Option<Vec<ImportOutcome>>>> = Arc::default();
        self.import = Some(ImportJob {
            progress: progress.clone(),
        });
        cx.notify();

        let library_dir = self.library.dir().to_owned();
        let task_outcome = outcome.clone();
        let total = paths.len() as f32;
        std::thread::spawn(move || {
            let mut outcomes = Vec::new();
            for (i, path) in paths.iter().enumerate() {
                let mut report = |p: f32| {
                    progress.store(((i as f32 + p) / total).to_bits(), Ordering::Relaxed);
                };
                let file_name = path.file_name().unwrap_or(path.as_os_str());
                outcomes.push(match import::import(path, &library_dir, &mut report) {
                    Ok(r) => Ok(format!(
                        "Imported {} with {} entries{}.",
                        r.meta.name,
                        settings_page::format_count(r.meta.entry_count),
                        match r.warning_count {
                            0 => String::new(),
                            1 => " (1 entry skipped)".into(),
                            n => format!(
                                " ({} entries skipped)",
                                settings_page::format_count(n as u32)
                            ),
                        }
                    )),
                    Err(e) => Err(format!("Couldn’t import {}: {e}", file_name.display())),
                });
            }
            *task_outcome.lock().unwrap() = Some(outcomes);
        });

        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let done = outcome.lock().unwrap().take();
                let keep_going = this
                    .update_in(cx, |this, window, cx| {
                        if let Some(outcomes) = done {
                            this.finish_import(outcomes, window, cx);
                            false
                        } else {
                            cx.notify();
                            true
                        }
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_import(
        &mut self,
        outcomes: Vec<ImportOutcome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.import = None;
        let reload_errors = self.library.reload().into_iter().map(Err);
        for outcome in outcomes.into_iter().chain(reload_errors) {
            let note = match outcome {
                Ok(message) => Notification::success(message),
                // Failures stay until dismissed, so they are not missed.
                Err(message) => Notification::error(message).autohide(false),
            };
            window.push_notification(note, cx);
        }
        self.library_changed(cx);
    }

    /// Resets everything that refers to dictionaries by index, after the set
    /// of dictionaries changed.
    fn library_changed(&mut self, cx: &mut Context<Self>) {
        self.shown = None;
        self.scope = None;
        self.history.clear();
        self.run_search(cx);
    }

    fn toggle_settings(&mut self, _: &ToggleSettings, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Dictionary => {
                self.mode = Mode::Settings;
                window.focus(&self.focus_handle, cx);
                cx.notify();
            }
            Mode::Settings => self.close_settings(&CloseSettings, window, cx),
        }
    }

    fn close_settings(&mut self, _: &CloseSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Settings {
            cx.propagate();
            return;
        }
        self.mode = Mode::Dictionary;
        self.search.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn set_dictionary_enabled(
        &mut self,
        identifier: &str,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let disabled = &mut self.settings.settings.disabled_dictionaries;
        if enabled {
            disabled.remove(identifier);
        } else {
            disabled.insert(identifier.to_owned());
        }
        self.library.set_disabled(disabled.clone());
        self.save_settings(window, cx);
        if self.scope.is_some_and(|s| !self.library.is_enabled(s)) {
            self.scope = None;
        }
        self.run_search(cx);
    }

    /// Deletes a dictionary. Success needs no message: the dictionary leaves
    /// the list.
    fn delete_dictionary(&mut self, identifier: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.library.position(identifier) else {
            return;
        };
        let name = self.library.dicts()[ix].meta().name.clone();
        if let Err(e) = self.library.delete(ix) {
            let message = format!("Couldn’t delete all of {name}’s files: {e}");
            window.push_notification(Notification::error(message).autohide(false), cx);
        }
        if self
            .settings
            .settings
            .disabled_dictionaries
            .remove(identifier)
        {
            self.save_settings(window, cx);
        }
        self.library_changed(cx);
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut App) {
        if let Err(e) = self.settings.save() {
            let message = format!(
                "Couldn’t save settings to {}: {e}",
                self.settings.path().display()
            );
            window.push_notification(Notification::error(message).autohide(false), cx);
        }
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("back")
                    .ghost()
                    .icon(IconName::ArrowLeft)
                    .tooltip("Back")
                    .disabled(!self.history.can_go_back())
                    .on_click(cx.listener(|this, _, window, cx| this.go_back(&Back, window, cx))),
            )
            .child(
                Button::new("forward")
                    .ghost()
                    .icon(IconName::ArrowRight)
                    .tooltip("Forward")
                    .disabled(!self.history.can_go_forward())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.go_forward(&Forward, window, cx)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    // Arrow keys move through the results instead of the text.
                    .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                        this.move_selection(-1, cx);
                        cx.stop_propagation();
                    }))
                    .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                        this.move_selection(1, cx);
                        cx.stop_propagation();
                    }))
                    .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                        if this.query(cx).is_empty() {
                            cx.stop_propagation();
                        } else {
                            this.set_query("", window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Input::new(&self.search)
                            .prefix(IconName::Search)
                            .cleanable(true),
                    ),
            )
            .child(
                Button::new("settings")
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip("Settings")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_settings(&ToggleSettings, window, cx)
                    })),
            )
    }

    fn render_settings_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("close-settings")
                    .ghost()
                    .icon(IconName::ArrowLeft)
                    .tooltip("Back to Dictionary")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_settings(&CloseSettings, window, cx)
                    })),
            )
            .child(div().text_sm().font_semibold().child("Settings"))
    }

    /// Indices of the dictionaries included in searches across all of them.
    fn enabled_dicts(&self) -> Vec<usize> {
        (0..self.library.dicts().len())
            .filter(|&ix| self.library.is_enabled(ix))
            .collect()
    }

    /// Tabs that restrict the search to one dictionary. Tabs that do not fit
    /// scroll, and the overflow menu lists them all.
    fn render_scopes(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<TabBar> {
        let enabled = self.enabled_dicts();
        if enabled.len() < 2 {
            return None;
        }
        let names = enabled
            .iter()
            .map(|&ix| self.library.dicts()[ix].meta().name.clone());
        Some(
            TabBar::new("scopes")
                .underline()
                .small()
                // Labels start on the same edge as the result rows.
                .px_3()
                .menu(true)
                .max_width(window.rem_size() * SCOPE_TAB_MAX_WIDTH)
                .track_scroll(&self.scope_scroll)
                .selected_index(self.scope_tab(&enabled))
                .child(Tab::new().label("All"))
                .children(names.map(|name| Tab::new().label(name)))
                .on_click(cx.listener(|this, tab: &usize, window, cx| {
                    this.select_scope(&SelectScope(*tab), window, cx)
                })),
        )
    }

    /// The result list and definition pane, with a draggable divider between.
    fn render_panes(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rem = window.rem_size();
        let width = self
            .settings
            .settings
            .results_width
            .unwrap_or(RESULTS_WIDTH);
        h_resizable("panes")
            .with_state(&self.split)
            .child(
                resizable_panel()
                    .size(rem * width)
                    .size_range(rem * RESULTS_WIDTH_RANGE.start..rem * RESULTS_WIDTH_RANGE.end)
                    .flex_none()
                    .child(self.render_results(cx)),
            )
            .child(
                resizable_panel()
                    .size_range(rem * DEFINITION_MIN_WIDTH..Pixels::MAX)
                    .child(self.render_definition(cx)),
            )
    }

    fn render_results(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let show_dict = self.scope.is_none() && self.enabled_dicts().len() > 1;
        div()
            .size_full()
            .relative()
            .bg(theme.sidebar)
            .child(
                uniform_list(
                    "results",
                    self.rows.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        let theme = cx.theme();
                        range
                            .map(|ix| {
                                let row = &this.rows[ix];
                                let selected = this.selected == Some(ix);
                                let muted = |text: String| {
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(text)
                                };
                                let dict_name = show_dict
                                    .then(|| this.library.dicts()[row.dict].meta().name.clone());
                                // Identity comes from what the row opens, not its position, so
                                // element state does not pass to another row as results change.
                                let meta = this.library.dicts()[row.dict].meta();
                                let row_id = SharedString::from(match &row.hit.anchor {
                                    Some(anchor) => {
                                        format!("{}/{}#{anchor}", meta.identifier, row.hit.entry)
                                    }
                                    None => format!("{}/{}", meta.identifier, row.hit.entry),
                                });
                                // Selected and hovered rows are inset, rounded surfaces like
                                // sidebar menu items; the text stays on the 12px spine.
                                let surface = h_flex()
                                    // Without an id GPUI keeps no hover state for
                                    // the element, so the hover style would only
                                    // update on some later, unrelated redraw.
                                    .id("surface")
                                    .size_full()
                                    .px_2()
                                    .rounded(theme.radius)
                                    .text_sm()
                                    .when(selected, |el| {
                                        el.bg(theme.sidebar_accent)
                                            .text_color(theme.sidebar_accent_foreground)
                                            .font_medium()
                                    })
                                    .when(!selected, |el| {
                                        el.hover(|el| el.bg(theme.sidebar_accent.opacity(0.5)))
                                    })
                                    // Label, detail and dictionary name share one
                                    // baseline even though their sizes differ. The
                                    // dictionary name gives way before the label.
                                    .child(
                                        h_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .items_baseline()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .max_w(relative(0.7))
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(row.label().to_owned()),
                                            )
                                            .children(row.detail().map(|d| muted(d.to_owned())))
                                            .children(dict_name.map(|name| {
                                                muted(name)
                                                    .flex_shrink(1.)
                                                    .min_w_0()
                                                    .ml_auto()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                            })),
                                    );
                                div()
                                    .id(row_id)
                                    .w_full()
                                    .h(px(ROW_HEIGHT))
                                    .px_1()
                                    .child(surface)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select(ix, true, cx);
                                        this.search.update(cx, |state, cx| state.focus(window, cx));
                                    }))
                            })
                            .collect()
                    }),
                )
                .size_full()
                .track_scroll(&self.list_scroll),
            )
            .vertical_scrollbar(&self.list_scroll)
    }

    fn render_definition(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(shown) = &self.shown else {
            let message = if self.library.is_empty() {
                "No dictionaries yet. Import a .dictionary bundle or Dictionary Development Kit XML in Settings (Ctrl+,)."
            } else if self.query(cx).is_empty() {
                "Type a word to look it up."
            } else {
                "No results."
            };
            return pane_message(IconName::BookOpen, message, None, theme).into_any_element();
        };
        let html = match &shown.html {
            Ok(html) => html.clone(),
            Err(error) => {
                let detail = format!("{error}. Importing the dictionary again may fix it.");
                return pane_message(
                    IconName::CircleX,
                    "Couldn’t load this entry.",
                    Some(detail.into()),
                    theme,
                )
                .into_any_element();
            }
        };
        let this = cx.weak_entity();
        let identifier = &self.library.dicts()[shown.page.dict].meta().identifier;
        let id = SharedString::from(format!("entry-{identifier}-{}", shown.page.entry));
        div()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .text_base()
            .child(
                TextView::html(id, html)
                    .scrollable(true)
                    .selectable(true)
                    .px_6()
                    .py_4()
                    .size_full()
                    .on_link_click(move |url, _, window, cx| {
                        let url = url.clone();
                        this.update(cx, |this, cx| this.follow_link(&url, window, cx))
                            .ok();
                    }),
            )
            .into_any_element()
    }
}

/// A centered message in the definition pane: empty states and errors.
fn pane_message(
    icon: IconName,
    message: &'static str,
    detail: Option<SharedString>,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .px_6()
        .text_color(theme.muted_foreground)
        .child(icon)
        .child(message)
        .children(detail.map(|detail| div().max_w(rems(30.)).text_sm().text_center().child(detail)))
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::close_settings))
            .on_action(cx.listener(Self::select_scope))
            .on_action(cx.listener(Self::next_scope))
            .on_action(cx.listener(Self::previous_scope))
            .on_key_down(cx.listener(Self::type_to_search))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child(div().text_sm().child("Dictionary")))
            .map(|el| match self.mode {
                Mode::Dictionary => el
                    .child(self.render_toolbar(cx))
                    .children(self.render_scopes(window, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_panes(window, cx)),
                    ),
                Mode::Settings => el
                    .child(self.render_settings_header(cx))
                    .child(div().flex_1().min_h_0().child(self.render_settings(cx))),
            })
    }
}
