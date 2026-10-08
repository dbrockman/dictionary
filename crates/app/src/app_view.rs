//! The main window: search field, result list and definition pane.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Escape, Input, InputEvent, InputState, MoveDown, MoveUp, SelectAll,
};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, TitleBar, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::history::{History, Page};
use crate::library::{Library, Row};

actions!(
    dictionary,
    [FocusSearch, Back, Forward, AddDictionary, Quit]
);

const KEY_CONTEXT: &str = "Dictionary";
const ROW_HEIGHT: f32 = 30.;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-l", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-left", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-right", Forward, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-[", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-]", Forward, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-o", AddDictionary, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
}

/// The entry currently shown in the definition pane.
struct Shown {
    page: Page,
    html: SharedString,
}

/// A running import, shared with its background thread.
struct ImportJob {
    progress: Arc<AtomicU32>,
    label: SharedString,
}

pub struct AppView {
    library: Library,
    search: Entity<InputState>,
    rows: Vec<Row>,
    selected: Option<usize>,
    /// Restricts searches to one dictionary.
    scope: Option<usize>,
    shown: Option<Shown>,
    history: History,
    list_scroll: UniformListScrollHandle,
    /// Keeps app shortcuts working when focus is not in the search field.
    focus_handle: FocusHandle,
    status: Option<SharedString>,
    import: Option<ImportJob>,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    pub fn new(
        library: Library,
        initial_query: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search")
                .clean_on_escape()
        });
        let subscriptions = vec![cx.subscribe_in(&search, window, Self::on_search_event)];
        search.update(cx, |state, cx| state.focus(window, cx));

        let mut this = Self {
            library,
            search,
            rows: Vec::new(),
            selected: None,
            scope: None,
            shown: None,
            history: History::default(),
            list_scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            status: None,
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => self.run_search(window, cx),
            InputEvent::PressEnter { .. } => {
                if let Some(ix) = self.selected {
                    self.open_row(ix, true, cx);
                }
            }
            _ => {}
        }
    }

    fn query(&self, cx: &App) -> SharedString {
        self.search.read(cx).value()
    }

    /// Replaces the search text and shows its results.
    fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |state, cx| {
            state.set_value(query.to_owned(), window, cx)
        });
        self.run_search(window, cx);
    }

    fn run_search(&mut self, _: &mut Window, cx: &mut Context<Self>) {
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
        let html = match self.library.entry_html(page.dict, page.entry) {
            Ok(html) => html,
            Err(e) => {
                self.status = Some(format!("Could not load entry: {e}").into());
                cx.notify();
                return;
            }
        };
        if record && let Some(previous) = self.shown.take() {
            self.history.push(previous.page);
        }
        self.sync_selection(&page);
        self.shown = Some(Shown {
            page,
            html: html.into(),
        });
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
                None => {
                    self.status = Some(format!("Entry {id:?} not found").into());
                    cx.notify();
                }
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
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
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

    fn set_scope(&mut self, scope: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.scope = scope;
        self.run_search(window, cx);
    }

    fn add_dictionary(&mut self, _: &AddDictionary, _: &mut Window, cx: &mut Context<Self>) {
        if self.import.is_some() {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update(cx, |this, cx| this.start_import(paths, cx))
                .ok();
        })
        .detach();
    }

    /// Imports `paths` one after another on a background thread.
    fn start_import(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let progress = Arc::new(AtomicU32::new(0));
        let outcome: Arc<Mutex<Option<Vec<String>>>> = Arc::default();
        self.import = Some(ImportJob {
            progress: progress.clone(),
            label: "Importing…".into(),
        });
        cx.notify();

        let library_dir = self.library.dir().to_owned();
        let task_outcome = outcome.clone();
        let total = paths.len() as f32;
        std::thread::spawn(move || {
            let mut messages = Vec::new();
            for (i, path) in paths.iter().enumerate() {
                let mut report = |p: f32| {
                    progress.store(((i as f32 + p) / total).to_bits(), Ordering::Relaxed);
                };
                match import::import(path, &library_dir, &mut report) {
                    Ok(r) => messages.push(format!(
                        "Imported {} ({} entries{})",
                        r.meta.name,
                        r.meta.entry_count,
                        if r.warning_count > 0 {
                            format!(", {} warnings", r.warning_count)
                        } else {
                            String::new()
                        }
                    )),
                    Err(e) => messages.push(format!("Import failed: {e}")),
                }
            }
            *task_outcome.lock().unwrap() = Some(messages);
        });

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let done = outcome.lock().unwrap().take();
                let keep_going = this
                    .update(cx, |this, cx| {
                        if let Some(messages) = done {
                            this.finish_import(messages, cx);
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

    fn finish_import(&mut self, mut messages: Vec<String>, cx: &mut Context<Self>) {
        self.import = None;
        messages.extend(self.library.reload());
        self.rows.clear();
        self.selected = None;
        self.shown = None;
        self.scope = None;
        self.history.clear();
        self.status = Some(messages.join(" · ").into());
        let query = self.query(cx);
        self.rows = self.library.search(&query, None);
        if !self.rows.is_empty() {
            self.select(0, false, cx);
        }
        cx.notify();
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
                Button::new("add")
                    .ghost()
                    .icon(IconName::Plus)
                    .tooltip("Add Dictionary…")
                    .loading(self.import.is_some())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_dictionary(&AddDictionary, window, cx)
                    })),
            )
    }

    fn render_scopes(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.library.dicts().len() < 2 {
            return None;
        }
        let mut row = h_flex()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("scope-all")
                    .small()
                    .ghost()
                    .label("All")
                    .selected(self.scope.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.set_scope(None, window, cx))),
            );
        for (ix, dict) in self.library.dicts().iter().enumerate() {
            row =
                row.child(
                    Button::new(("scope", ix))
                        .small()
                        .ghost()
                        .label(dict.meta().name.clone())
                        .selected(self.scope == Some(ix))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_scope(Some(ix), window, cx)
                        })),
                );
        }
        Some(row)
    }

    fn render_results(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let show_dict = self.scope.is_none() && self.library.dicts().len() > 1;
        div()
            .w(px(280.))
            .h_full()
            .border_r_1()
            .border_color(theme.border)
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
                                h_flex()
                                    .id(ix)
                                    .w_full()
                                    .h(px(ROW_HEIGHT))
                                    .px_3()
                                    .gap_2()
                                    .justify_between()
                                    .text_sm()
                                    .when(selected, |el| {
                                        el.bg(theme.list_active).text_color(theme.foreground)
                                    })
                                    .when(!selected, |el| el.hover(|el| el.bg(theme.list_hover)))
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(row.hit.title.clone()),
                                    )
                                    .when(show_dict, |el| {
                                        el.child(
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(
                                                    this.library.dicts()[row.dict]
                                                        .meta()
                                                        .name
                                                        .clone(),
                                                ),
                                        )
                                    })
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
    }

    fn render_definition(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(shown) = &self.shown else {
            let message = if self.library.is_empty() {
                "No dictionaries yet. Use + to import a .dictionary bundle or Dictionary Development Kit XML."
            } else if self.query(cx).is_empty() {
                "Type a word to look it up."
            } else {
                "No results."
            };
            return v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(theme.muted_foreground)
                .child(IconName::BookOpen)
                .child(message)
                .into_any_element();
        };
        let this = cx.weak_entity();
        let id = SharedString::from(format!("entry-{}-{}", shown.page.dict, shown.page.entry));
        div()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .text_base()
            .child(
                TextView::html(id, shown.html.clone())
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

    fn render_status(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let text: SharedString = if let Some(job) = &self.import {
            let p = f32::from_bits(job.progress.load(Ordering::Relaxed));
            format!("{} {:.0}%", job.label, p * 100.).into()
        } else {
            self.status.clone()?
        };
        Some(
            div()
                .px_3()
                .py_1()
                .text_xs()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_color(cx.theme().muted_foreground)
                .child(text),
        )
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::add_dictionary))
            .on_key_down(cx.listener(Self::type_to_search))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child(div().text_sm().child("Dictionary")))
            .child(self.render_toolbar(cx))
            .children(self.render_scopes(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_results(cx))
                    .child(self.render_definition(cx)),
            )
            .children(self.render_status(cx))
    }
}
