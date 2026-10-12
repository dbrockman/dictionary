//! The settings page: appearance, imported dictionaries, where data is
//! stored, and the app's version.
//!
//! There is a single page, so it is composed from `GroupBox`es directly.
//! GPUI Kit's `Settings` component would add a navigation sidebar and search
//! for that one page.

use std::sync::atomic::Ordering;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::group_box::GroupBox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, ThemeMode, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{AppView, UpdateCheck};
use crate::config::Appearance;
use crate::update::{REPOSITORY, VERSION};

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
                    .child(self.render_appearance_group(cx))
                    .child(self.render_dictionaries_group(cx))
                    .child(self.render_storage_group(cx))
                    .child(self.render_about_group(cx)),
            )
            .overflow_y_scrollbar()
    }

    fn render_appearance_group(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        const CHOICES: [(Appearance, &str); 3] = [
            (Appearance::System, "System"),
            (Appearance::Light, "Light"),
            (Appearance::Dark, "Dark"),
        ];
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        // A problem replaces the row's help text, in the danger color, so it
        // sits next to the control it is about.
        let help = |problem: &Option<String>, help: &'static str| match problem {
            Some(problem) => div().text_color(danger).child(problem.clone()),
            None => div().child(help),
        };

        let current = self.settings.settings.appearance;
        let mode = RadioGroup::horizontal("appearance")
            .children(CHOICES.map(|(_, label)| label))
            .selected_index(CHOICES.iter().position(|(choice, _)| *choice == current))
            .on_change(cx.listener(|this, ix: &usize, window, cx| {
                this.set_appearance(CHOICES[*ix].0, window, cx)
            }));
        let theme_row = |id: &'static str, title: &'static str, mode: ThemeMode, text| {
            let ix = usize::from(mode.is_dark());
            setting_row(
                id,
                Some(title.into()),
                help(&self.theme.problems[ix], text),
                // The select grows to fill its parent, so the parent sets its width.
                div()
                    .flex_none()
                    .w(rems(14.))
                    .child(Select::new(self.theme.select(mode))),
                cx,
            )
        };
        let light = theme_row(
            "light-theme",
            "Light theme",
            ThemeMode::Light,
            "Used when the app is light.",
        );
        let dark = theme_row(
            "dark-theme",
            "Dark theme",
            ThemeMode::Dark,
            "Used when the app is dark.",
        );

        let dir = self.theme.themes.dir().to_owned();
        let folder_description = v_flex().child(dir.display().to_string()).children(
            self.theme
                .themes
                .file_errors()
                .iter()
                .map(|error| div().text_color(danger).child(error.clone())),
        );
        let open = Button::new("open-themes")
            .outline()
            .label("Open folder")
            .on_click(move |_, _, cx| {
                std::fs::create_dir_all(&dir).ok();
                cx.open_with_system(&dir);
            });

        let title = v_flex().gap_1().child("Appearance").child(
            Label::new(
                "Theme files in the themes folder join the lists and reload when they change.",
            )
            .text_sm()
            .text_color(muted),
        );
        GroupBox::new()
            .id("appearance")
            .title(title)
            .gap_4()
            .child(setting_row(
                "appearance-mode",
                Some("Mode".into()),
                "System follows the operating system and switches with it.",
                mode,
                cx,
            ))
            .child(light)
            .child(dark)
            .child(setting_row(
                "themes-folder",
                Some("Themes folder".into()),
                folder_description,
                open,
                cx,
            ))
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
            let name = SharedString::from(self.library.name(ix).to_owned());
            let description = v_flex()
                .child(format!(
                    "{} entries · {} on disk",
                    format_count(meta.entry_count),
                    format_size(self.library.size_on_disk(ix))
                ))
                .child(dict.path().display().to_string());
            let controls = self.render_dictionary_controls(
                meta.identifier.clone().into(),
                name.clone(),
                meta.name.clone().into(),
                self.library.is_enabled(ix),
                cx,
            );
            group = group.child(setting_row(
                SharedString::from(format!("dictionary-{}", meta.identifier)),
                Some(name),
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
        original_name: SharedString,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let toggle = cx.listener({
            let identifier = identifier.clone();
            move |this, enabled: &bool, window, cx| {
                this.set_dictionary_enabled(&identifier, *enabled, window, cx)
            }
        });
        let rename = cx.listener({
            let identifier = identifier.clone();
            let name = name.clone();
            move |this, _: &ClickEvent, window, cx| {
                window.focus(&this.focus_handle, cx);
                open_rename_dialog(
                    cx.weak_entity(),
                    identifier.clone(),
                    name.clone(),
                    original_name.clone(),
                    window,
                    cx,
                )
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
                Button::new(SharedString::from(format!("rename-{identifier}")))
                    .outline()
                    .label("Rename…")
                    .on_click(rename),
            )
            .child(
                Button::new(SharedString::from(format!("delete-{identifier}")))
                    .outline()
                    .label("Delete")
                    .on_click(delete),
            )
    }

    fn render_storage_group(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Neither folder exists until something is first saved there.
        let open_folder = |id: &'static str, dir: &std::path::Path| {
            let dir = dir.to_owned();
            Button::new(id)
                .outline()
                .label("Open folder")
                .on_click(move |_, _, cx| {
                    std::fs::create_dir_all(&dir).ok();
                    cx.open_with_system(&dir);
                })
        };
        let open_library = open_folder("open-library", self.library.dir());
        let settings_dir = self
            .settings
            .path()
            .parent()
            .unwrap_or(self.settings.path());
        let open_settings = open_folder("open-settings", settings_dir);
        GroupBox::new()
            .id("storage")
            .title("Storage")
            .gap_4()
            .child(setting_row(
                "library-folder",
                Some("Library folder".into()),
                self.library.dir().display().to_string(),
                open_library,
                cx,
            ))
            .child(setting_row(
                "settings-file",
                Some("Settings file".into()),
                self.settings.path().display().to_string(),
                open_settings,
                cx,
            ))
    }

    fn render_about_group(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let status = match &self.update_check {
            UpdateCheck::NotChecked => None,
            UpdateCheck::Checking => Some(div().child("Checking for updates…")),
            UpdateCheck::Done(Ok(Some(release))) if release.is_newer() => {
                Some(div().child(format!("Version {} is available.", release.version)))
            }
            UpdateCheck::Done(Ok(Some(_))) => Some(div().child("This is the latest version.")),
            UpdateCheck::Done(Ok(None)) => Some(div().child("No releases are published yet.")),
            UpdateCheck::Done(Err(error)) => Some(div().text_color(danger).child(error.clone())),
            UpdateCheck::Installing { version, progress } => Some(div().child(format!(
                "Downloading version {version}… {:.0}%",
                f32::from_bits(progress.load(Ordering::Relaxed)) * 100.
            ))),
            UpdateCheck::Installed { version } => Some(div().child(format!(
                "Version {version} is installed. Restart to use it."
            ))),
        };
        let description = v_flex()
            .child(format!("Dictionary {VERSION}"))
            .children(status);

        // Once a newer release is known, the AppImage can install it; other
        // installs only lead to its page.
        let controls = match &self.update_check {
            UpdateCheck::Done(Ok(Some(release))) if release.is_newer() => {
                let url = release.url.clone();
                let view_release = Button::new("view-release")
                    .label("View release")
                    .on_click(move |_, _, cx| cx.open_url(&url));
                if self.can_install(release) {
                    let release = release.clone();
                    h_flex()
                        .gap_2()
                        .child(view_release.outline())
                        .child(
                            Button::new("install-update")
                                .primary()
                                .label("Install update")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.install_update(release.clone(), cx)
                                })),
                        )
                        .into_any_element()
                } else {
                    view_release.primary().into_any_element()
                }
            }
            UpdateCheck::Installing { .. } => Button::new("install-update")
                .primary()
                .label("Install update")
                .loading(true)
                .into_any_element(),
            UpdateCheck::Installed { .. } => Button::new("restart")
                .primary()
                .label("Restart")
                .on_click(cx.listener(|this, _, _, cx| this.restart_after_update(cx)))
                .into_any_element(),
            checking => Button::new("check-for-updates")
                .outline()
                .label("Check for updates")
                .loading(matches!(checking, UpdateCheck::Checking))
                .on_click(cx.listener(|this, _, _, cx| this.check_for_updates(cx)))
                .into_any_element(),
        };

        let open_repository = Button::new("open-repository")
            .outline()
            .label("Open on GitHub")
            .on_click(|_, _, cx| cx.open_url(REPOSITORY));
        GroupBox::new()
            .id("about")
            .title("About")
            .gap_4()
            .child(setting_row(
                "version",
                Some("Version".into()),
                description,
                controls,
                cx,
            ))
            .child(setting_row(
                "repository",
                Some("Source code".into()),
                REPOSITORY,
                open_repository,
                cx,
            ))
    }
}

/// A row laid out like GPUI Kit's `SettingItem`: a title and a muted
/// description on the leading side, the controls on the trailing side.
fn setting_row(
    id: impl Into<ElementId>,
    title: Option<SharedString>,
    description: impl IntoElement,
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
                        .child(description),
                ),
        )
        .child(controls)
}

/// Asks for a new name for a dictionary. Clearing the field restores the
/// name it came with, which the field shows as its placeholder.
fn open_rename_dialog(
    view: WeakEntity<AppView>,
    identifier: SharedString,
    name: SharedString,
    original_name: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(original_name)
            .default_value(name.clone())
    });
    window.open_dialog(cx, {
        let input = input.clone();
        move |dialog, _, cx| {
            let view = view.clone();
            let identifier = identifier.clone();
            let input = input.clone();
            dialog
                .title(SharedString::from(format!("Rename “{name}”")))
                .child(
                    v_flex().gap_2().child(Input::new(&input)).child(
                        Label::new("Leave it empty to use the dictionary’s own name.")
                            .text_sm()
                            .text_color(cx.theme().muted_foreground),
                    ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel-rename").outline().label("Cancel")),
                        )
                        .child(
                            DialogAction::new()
                                .child(Button::new("rename").primary().label("Rename")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name = input.read(cx).value();
                    view.update(cx, |this, cx| {
                        this.rename_dictionary(&identifier, &name, window, cx)
                    })
                    .ok();
                    true
                })
        }
    });
    input.update(cx, |state, cx| {
        state.focus(window, cx);
        state.select_all(window, cx);
    });
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
