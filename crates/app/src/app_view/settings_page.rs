//! The settings page, built with GPUI Kit's `Settings` component.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::setting::{
    RenderOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex};
use gpui_kit::*;

use super::{AddDictionary, AppView};

impl AppView {
    pub(super) fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        Settings::new("settings").pages([self.dictionaries_page(cx.weak_entity())])
    }

    fn dictionaries_page(&self, view: WeakEntity<Self>) -> SettingPage {
        let mut imported = SettingGroup::new().title("Imported Dictionaries");
        if self.library.is_empty() {
            imported = imported.item(SettingItem::render(|_, _, cx| {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No dictionaries imported yet.")
            }));
        }
        for (ix, dict) in self.library.dicts().iter().enumerate() {
            let meta = dict.meta();
            let row = DictionaryRow {
                view: view.clone(),
                ix,
                identifier: meta.identifier.clone().into(),
                name: meta.name.clone().into(),
                enabled: self.library.is_enabled(ix),
            };
            let description = format!(
                "{} entries · {} on disk",
                format_count(meta.entry_count),
                format_size(self.library.size_on_disk(ix))
            );
            imported = imported.item(
                SettingItem::new(
                    row.name.clone(),
                    SettingField::render(move |options, _, _| row.render_actions(options)),
                )
                .description(SharedString::from(description)),
            );
        }

        let importing = self.import.as_ref().map(|job| job.progress());
        let add = SettingItem::new(
            "Add a Dictionary",
            SettingField::render({
                let view = view.clone();
                move |options, _, _| {
                    let view = view.clone();
                    Button::new("add-dictionary")
                        .outline()
                        .label("Add…")
                        .loading(importing.is_some())
                        .with_size(options.size())
                        .on_click(move |_, window, cx| {
                            view.update(cx, |this, cx| {
                                this.add_dictionary(&AddDictionary, window, cx)
                            })
                            .ok();
                        })
                }
            }),
        )
        .description(SharedString::from(match importing {
            Some(progress) => format!("Importing… {:.0}%", progress * 100.),
            None => "Import a .dictionary bundle or a Dictionary Development Kit folder.".into(),
        }));
        imported = imported.item(add);

        let library_dir = self.library.dir().to_owned();
        let storage = SettingGroup::new().title("Storage").items([
            SettingItem::new(
                "Library Folder",
                SettingField::render(move |options, _, _| {
                    let dir = library_dir.clone();
                    Button::new("open-library")
                        .outline()
                        .label("Open Folder")
                        .with_size(options.size())
                        .on_click(move |_, _, cx| {
                            // It does not exist until the first import.
                            std::fs::create_dir_all(&dir).ok();
                            cx.open_with_system(&dir);
                        })
                }),
            )
            .description(SharedString::from(self.library.dir().display().to_string())),
            SettingItem::new("Settings File", SettingField::render(|_, _, _| div())).description(
                SharedString::from(self.settings.path().display().to_string()),
            ),
        ]);

        SettingPage::new("Dictionaries")
            .icon(IconName::BookOpen)
            .groups([imported, storage])
    }
}

/// What a dictionary's row in the list needs to render and act.
struct DictionaryRow {
    view: WeakEntity<AppView>,
    ix: usize,
    identifier: SharedString,
    name: SharedString,
    enabled: bool,
}

impl DictionaryRow {
    fn render_actions(&self, options: &RenderOptions) -> impl IntoElement + use<> {
        let toggle = {
            let view = self.view.clone();
            let identifier = self.identifier.clone();
            move |enabled: &bool, window: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| {
                    this.set_dictionary_enabled(&identifier, *enabled, window, cx)
                })
                .ok();
            }
        };
        let delete = {
            let view = self.view.clone();
            let identifier = self.identifier.clone();
            let name = self.name.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                confirm_delete(view.clone(), identifier.clone(), name.clone(), window, cx)
            }
        };
        h_flex()
            .gap_3()
            .child(
                Switch::new(("dictionary-enabled", self.ix))
                    .checked(self.enabled)
                    .tooltip("Include in searches")
                    .on_click(toggle),
            )
            .child(
                Button::new(("dictionary-delete", self.ix))
                    .outline()
                    .label("Delete")
                    .with_size(options.size())
                    .on_click(delete),
            )
    }
}

fn confirm_delete(
    view: WeakEntity<AppView>,
    identifier: SharedString,
    name: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    // The dialog gives focus back to whatever had it when it opened. Make
    // that the view rather than the Delete button, which is gone by then.
    if let Some(view) = view.upgrade() {
        let focus = view.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    }
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
fn format_count(n: u32) -> String {
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
