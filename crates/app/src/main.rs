//! Dictionary: a fast, native dictionary app.
//!
//! `dictionary [WORD]` opens the window (optionally looking up WORD);
//! `dictionary import <PATH>...` imports dictionaries without a GUI.

mod app_view;
mod config;
mod history;
mod library;

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::*;

use crate::app_view::{AppView, Quit};
use crate::library::Library;

#[derive(Parser)]
#[command(version, about = "A fast native dictionary")]
struct Cli {
    /// Directory holding imported dictionaries.
    #[arg(long, global = true, value_name = "DIR")]
    library: Option<PathBuf>,
    /// Print startup timings to stderr.
    #[arg(long, global = true)]
    timings: bool,
    #[command(subcommand)]
    command: Option<Command>,
    /// Word to look up on start.
    word: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Import `.dictionary` bundles or Dictionary Development Kit XML.
    Import {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let started = Instant::now();
    let cli = Cli::parse();
    let library_dir = cli.library.clone().unwrap_or_else(Library::default_dir);
    match cli.command {
        Some(Command::Import { paths }) => import_cli(&paths, &library_dir),
        None => {
            run_gui(library_dir, cli.word, cli.timings.then_some(started));
            Ok(())
        }
    }
}

fn import_cli(paths: &[PathBuf], library_dir: &std::path::Path) -> anyhow::Result<()> {
    for path in paths {
        let start = Instant::now();
        let mut last = -1i32;
        let report = import::import(path, library_dir, &mut |p| {
            let percent = (p * 100.) as i32;
            if percent / 10 != last / 10 {
                eprint!("\r\x1b[2K{}: {percent}%", path.display());
                last = percent;
            }
        })
        .with_context(|| format!("importing {}", path.display()))?;
        eprintln!(
            "\r\x1b[2K{}: {} entries, {} keys → {} ({:.1?})",
            report.meta.name,
            report.meta.entry_count,
            report.meta.key_count,
            report.path.display(),
            start.elapsed()
        );
        for warning in &report.warnings {
            eprintln!("  warning: {warning}");
        }
        if report.warning_count > report.warnings.len() {
            eprintln!(
                "  … and {} more warnings",
                report.warning_count - report.warnings.len()
            );
        }
    }
    Ok(())
}

fn run_gui(library_dir: PathBuf, word: Option<String>, started: Option<Instant>) {
    let (library, errors) = Library::open(&library_dir);
    let (settings, settings_error) = config::SettingsStore::load(&config::config_dir());
    for error in errors.into_iter().chain(settings_error) {
        eprintln!("{error}");
    }
    if let Some(t) = started {
        eprintln!("dictionaries opened: {:?}", t.elapsed());
    }

    application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            load_fonts(cx);
            gpui_kit::init(cx);
            app_view::bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());

            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Dictionary".into()),
                    ..TitleBar::title_bar_options()
                }),
                window_bounds: Some(WindowBounds::centered(size(px(960.), px(640.)), cx)),
                app_id: Some("dictionary".into()),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
                window.set_window_title("Dictionary");
                if let Some(t) = started {
                    window.on_next_frame(move |_, _| eprintln!("first frame: {:?}", t.elapsed()));
                }
                cx.new(|cx| AppView::new(library, settings, word, window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}

/// GPUI's Linux UI font is IBM Plex Sans; bundle it so text renders the same
/// (with real bold and italic faces) whether or not it is installed.
fn load_fonts(cx: &mut App) {
    if cfg!(target_os = "linux") {
        let fonts = vec![
            include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")
                .as_slice()
                .into(),
            include_bytes!("../assets/fonts/IBMPlexSans-Italic.ttf")
                .as_slice()
                .into(),
            include_bytes!("../assets/fonts/IBMPlexSans-Bold.ttf")
                .as_slice()
                .into(),
            include_bytes!("../assets/fonts/IBMPlexSans-BoldItalic.ttf")
                .as_slice()
                .into(),
        ];
        if let Err(e) = cx.text_system().add_fonts(fonts) {
            eprintln!("could not load bundled fonts: {e}");
        }
    }
}
