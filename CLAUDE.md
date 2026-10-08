# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

A native dictionary app in the spirit of macOS Dictionary.app, built with Rust and GPUI Kit (`gpui-kit` 0.7, on the `gpui-pre` crates). It reads Apple's dictionaries, compiled `.dictionary` bundles and Dictionary Development Kit (DDK) source XML, by importing them once into its own format.

**Product constraints:** performance comes first (fast startup and per-keystroke search). **Never use a WebView.** Entries render natively; "functional and looks OK" beats matching Apple's styling. Put expensive work at import time, not at runtime.

## Commands

Development happens on NixOS. GPUI needs Wayland/X11, xkbcommon, Vulkan and fontconfig from `flake.nix`. `just` recipes wrap themselves in `nix develop` automatically; plain `cargo` must run inside `nix develop`.

```sh
just                      # list recipes
just run [args]           # release build of the app; e.g. `just run hello`, `just run --timings`
just test                 # all tests (release profile; imports the real bundles in data/ too)
just lint                 # cargo fmt --check + clippy -D warnings (both must be clean)
just check                # lint + test
just import-data          # import data/*.dictionary into the app's library
just bench                # dictdb benchmarks (open / prefix search / entry fetch)
just raw-entry data/Swedish.dictionary hus   # raw vs simplified XHTML of an Apple entry
just dump <path.dictdb> <word>               # inspect an imported dictionary
```

Running a single test: `just test -p import --test apple_bundle modern_compressed_bundle`, or `nix develop -c cargo test -p dictdb normalize`.

The binary is `dictionary` (package `app`). `dictionary import <paths>…` imports without a GUI, `--library DIR` (or `DICTIONARY_LIBRARY`) overrides the library location, and `--timings` prints startup timings.

## Architecture

The workspace has three crates, forming a one-way pipeline: **import → dictdb → app**.

- **`crates/dictdb`**: the app's own storage format. `format.rs` documents the on-disk layout. `DictWriter` streams entries into zstd blocks and builds `fst` maps for keys and entry ids. `Dictionary::open` memory-maps every file, so opening is constant-time. Search is an fst prefix range scan over keys produced by `normalize_key`, ranked by `Hit::tier` (exact match, then headwords, then phrases, i.e. keys with an anchor into an entry). `Library::search` applies the same ranking when merging dictionaries, which strips Latin, Greek and Cyrillic diacritics but keeps marks that change meaning (e.g. Japanese dakuten). The same normalization must be used when writing and when searching. Any layout change needs a `FORMAT_VERSION` bump.
- **`crates/import`**: turns sources into dictdb directories.
  - `apple_bundle/` reads compiled bundles. The format is **undocumented** and reverse-engineered; the layouts are described in the module docs of `container.rs` and `keytext.rs`. Only `Info.plist`, `Body.data` and `KeyText.data` are used. KeyText records point at Body entries by `(section offset, offset within decompressed section)`. Some bundles (Apple Dictionary) store their data per language in `Resources/<lang>.lproj/`; the importer picks one from the POSIX locale. The format notes came from pyglossary, which is GPL-3; this repo is MIT, so never copy pyglossary code.
  - `ddk_xml.rs` reads DDK source.
  - `simplify.rs` is shared by both importers. It converts Apple's class-styled XHTML (nested `<span class=…>` that relies on CSS) into plain semantic HTML using the `CLASS_RULES` table. It also rewrites links into app-internal schemes: `dict:find/<word>`, `dict:entry/<id>` and `dict:res/<path>`.
- **`crates/app`**: the GPUI Kit UI. `library.rs` opens every `*.dictdb` and merges search results across dictionaries. `config.rs` loads and saves `settings.json` (which dictionaries are disabled, the result list width) from the XDG-style config dir. `app_view.rs` is the single main view (search field, scope `TabBar`, a resizable split of the `uniform_list` of results and the `TextView::html` definition pane) and resolves the `dict:` links. It switches between dictionary mode and a settings mode. The settings page (`app_view/settings_page.rs`) lives in a child module so it can reach the view's private state. It is one page, so it is composed from `GroupBox`es and rows laid out like GPUI Kit's `SettingItem`, not the `Settings` component, which would add a sidebar for that single page. Its controls use `cx.listener`; dialog callbacks are `'static` and capture a `WeakEntity<AppView>`.

### Non-obvious constraints

- **GPUI Kit's HTML renderer** (`gpui-base` text/format/html.rs) supports only semantic tags (headings, p/div, lists, tables, blockquote, b/i/u/s/code/mark, a, img, br). It ignores classes and CSS. Its minifier also drops whitespace at the start or end of an element's content. That is why the simplifier's `Output` holds spaces back and writes them outside tags, and why `SPACED_CLASSES` adds soft spaces where Apple relies on CSS margins. Check spacing changes against real data with `just raw-entry`.
- **Images:** the component `TextView` only loads `data:` and HTTP images. `Library::entry_html` therefore inlines `dict:res/…` images from the dictionary's `resources/` directory as cached data URLs.
- **Arrow keys:** the input binds up/down (`MoveUp`/`MoveDown`) itself. The search container intercepts them with `capture_action` to move the result selection.
- **Focus after link clicks:** following a link replaces the `TextView` and drops focus. The view keeps its own `focus_handle` (tracked on the root div) so the `Dictionary` key context and its shortcuts keep working. Likewise, GPUI Kit dialogs restore focus to whatever was focused when they opened, so focus the view before opening one from a control that may disappear.
- **Fonts:** GPUI's Linux UI font is IBM Plex Sans. It is bundled in `crates/app/assets/fonts` (OFL) and loaded at startup; otherwise bold and italic silently fall back to regular. Icons need `application().with_assets(gpui_kit::assets::Assets)`.

## Data and testing

- `data/` holds real Apple dictionary bundles copied from a Mac. It is licensed content and **git-ignored; never commit it.** `tests/apple_bundle.rs::real_bundles_in_data` imports everything in it when present.
- The other Apple-format tests build synthetic bundles with a test-side writer, and DDK tests use `crates/import/tests/fixtures/ddk`, so CI needs no Apple content.
- To look at the GUI without touching the user's desktop, run the release binary under `Xvfb` with `WAYLAND_DISPLAY` unset and `DISPLAY` pointing at it. Set `XDG_CONFIG_HOME` and `--library` to scratch directories so tests do not touch the real settings or library. Drive it with `xdotool` and capture with ImageMagick `import` (all available via `nix shell nixpkgs#xorg-server nixpkgs#xdotool nixpkgs#imagemagick`).

## GPUI Kit skills

`.claude/skills/gpui-kit` and `.claude/skills/gpui-kit-design-guides` are copied unmodified from the GPUI Kit repository at tag `v0.7.1` (commit `87d10ae`), matching the `gpui-kit = "0.7.1"` dependency. They are Apache-2.0 licensed (`.claude/skills/LICENSE-gpui-kit-APACHE`). When upgrading `gpui-kit`, replace them with the `skills/` directory from the matching release tag of https://github.com/longbridge/gpui-kit.
