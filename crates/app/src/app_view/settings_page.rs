//! The settings page: imported dictionaries and where their data is stored.
//!
//! There is a single page, so it is composed from `GroupBox`es directly.
//! GPUI Kit's `Settings` component would add a navigation sidebar and search
//! for that one page.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::group_box::GroupBox;
use gpui_kit::component::label::Label;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::AppView;

/// Line length that keeps a row's title and its controls easy to connect.
const PAGE_MAX_WIDTH: f32 = 40.;

impl AppView {
    pub(super) fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings")
            .size_full()
            .child(
                v_flex()
                    .w_full()
                    .max_w(rems(PAGE_MAX_WIDTH))
                    .px_4()
                    .py_6()
                    .gap_6()
                    .child(self.render_dictionaries_group(cx))
                    .child(self.render_storage_group(cx)),
            )
            .overflow_y_scrollbar()
    }

    fn render_dictionaries_group(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        // The description says what the switches on each row do, the way
        // GPUI Kit's `SettingGroup` shows a group description.
        let title =
            v_flex()
                .gap_1()
                .child("Dictionaries")
                .when(!self.library.is_empty(), |title| {
                    title.child(
                        Label::new("Dictionaries that are switched off are left out of searches.")
                            .text_sm()
                            .text_color(muted),
                    )
                });
        let mut group = GroupBox::new().id("dictionaries").title(title).gap_4();
        if self.library.is_empty() {
            group = group.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No dictionaries imported yet."),
            );
        }
        for (ix, dict) in self.library.dicts().iter().enumerate() {
            let meta = dict.meta();
            let description = format!(
                "{} entries · {} on disk",
                format_count(meta.entry_count),
                format_size(self.library.size_on_disk(ix))
            );
            let controls = self.render_dictionary_controls(
                meta.identifier.clone().into(),
                meta.name.clone().into(),
                self.library.is_enabled(ix),
                cx,
            );
            group = group.child(setting_row(
                SharedString::from(format!("dictionary-{}", meta.identifier)),
                Some(meta.name.clone().into()),
                description,
                controls,
                cx,
            ));
        }

        let importing = self.import.as_ref().map(|job| job.progress());
        let description = match importing {
            Some(progress) => format!("Importing… {:.0}%", progress * 100.),
            None => "Import a .dictionary bundle or a Dictionary Development Kit folder.".into(),
        };
        let add = Button::new("add-dictionary")
            .outline()
            .label("Add dictionary…")
            .loading(importing.is_some())
            .on_click(cx.listener(|this, _, window, cx| this.add_dictionary(window, cx)));
        group.child(setting_row(
            "add-dictionary-row",
            None,
            description,
            add,
            cx,
        ))
    }

    fn render_dictionary_controls(
        &self,
        identifier: SharedString,
        name: SharedString,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let toggle = cx.listener({
            let identifier = identifier.clone();
            move |this, enabled: &bool, window, cx| {
                this.set_dictionary_enabled(&identifier, *enabled, window, cx)
            }
        });
        let delete = cx.listener({
            let identifier = identifier.clone();
            move |this, _: &ClickEvent, window, cx| {
                // The dialog gives focus back to whatever had it when it
                // opened. Make that the view rather than the Delete button,
                // which is gone by then.
                window.focus(&this.focus_handle, cx);
                confirm_delete(
                    cx.weak_entity(),
                    identifier.clone(),
                    name.clone(),
                    window,
                    cx,
                )
            }
        });
        h_flex()
            .gap_3()
            .child(
                Switch::new(SharedString::from(format!("enabled-{identifier}")))
                    .checked(enabled)
                    .tooltip("Include in searches")
                    .on_click(toggle),
            )
            .child(
                Button::new(SharedString::from(format!("delete-{identifier}")))
                    .outline()
                    .label("Delete")
                    .on_click(delete),
            )
    }

    fn render_storage_group(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let library_dir = self.library.dir().to_owned();
        let open = Button::new("open-library")
            .outline()
            .label("Open folder")
            .on_click(move |_, _, cx| {
                // It does not exist until the first import.
                std::fs::create_dir_all(&library_dir).ok();
                cx.open_with_system(&library_dir);
            });
        GroupBox::new()
            .id("storage")
            .title("Storage")
            .gap_4()
            .child(setting_row(
                "library-folder",
                Some("Library folder".into()),
                self.library.dir().display().to_string(),
                open,
                cx,
            ))
            .child(setting_row(
                "settings-file",
                Some("Settings file".into()),
                self.settings.path().display().to_string(),
                div(),
                cx,
            ))
    }
}

/// A row laid out like GPUI Kit's `SettingItem`: a title and a muted
/// description on the leading side, the controls on the trailing side.
fn setting_row(
    id: impl Into<ElementId>,
    title: Option<SharedString>,
    description: impl Into<SharedString>,
    controls: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .id(id)
        .w_full()
        .justify_between()
        .items_center()
        .gap_3()
        .child(
            v_flex()
                .flex_1()
                .max_w_3_5()
                .when_some(title, |el, title| el.child(Label::new(title).text_sm()))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(description.into()),
                ),
        )
        .child(controls)
}

fn confirm_delete(
    view: WeakEntity<AppView>,
    identifier: SharedString,
    name: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let view = view.clone();
        let identifier = identifier.clone();
        alert
            .title(SharedString::from(format!("Delete “{name}”?")))
            .description(
                "This removes the imported copy from your library. The original \
                 dictionary files are not touched, so you can import it again later.",
            )
            .confirm()
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                view.update(cx, |this, cx| {
                    this.delete_dictionary(&identifier, window, cx)
                })
                .ok();
                true
            })
    });
}

/// `113953` → `113,953`.
pub(super) fn format_count(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Bytes in decimal units, as file managers show them: `26.4 MB`.
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = "";
    for u in UNITS {
        value /= 1000.;
        unit = u;
        if value < 1000. {
            break;
        }
    }
    format!("{value:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::{format_count, format_size};

    #[test]
    fn formats_counts_and_sizes() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(113_953), "113,953");
        assert_eq!(format_count(1_234_567), "1,234,567");
        assert_eq!(format_size(512), "512 bytes");
        assert_eq!(format_size(92_000), "92.0 KB");
        assert_eq!(format_size(26_400_000), "26.4 MB");
        assert_eq!(format_size(3_100_000_000), "3.1 GB");
    }
}
