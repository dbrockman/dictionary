# dictionary

A fast, native dictionary app in the spirit of macOS Dictionary.app, built with
Rust and [GPUI Kit](https://gpui-kit.com/). It reads the same dictionaries as
Dictionary.app: compiled Apple `.dictionary` bundles and Dictionary Development
Kit (DDK) source XML.

Dictionaries are imported once into the app's own memory-mapped format, so the
app opens instantly and searches as you type. No dictionaries are bundled; you
import your own.

## Building

On NixOS (or anywhere with Nix), the dev shell provides the libraries GPUI needs:

```sh
nix develop
cargo run --release
```

Common tasks are in the `justfile` (run `just` to list them): `just run`,
`just test`, `just lint`, `just import-data`, `just update` and so on. They
enter the Nix dev shell automatically.

Elsewhere, install the GPUI Linux prerequisites (Wayland/X11, xkbcommon,
Vulkan loader and fontconfig) and use `cargo` directly.

## Importing dictionaries

From the app, click **+** (or press Ctrl+O) and choose a `.dictionary` bundle or a
DDK project folder. Or use the command line:

```sh
dictionary import ~/Downloads/Oxford.dictionary MyProject/
```

On a Mac, Apple's dictionaries live in
`/System/Library/AssetsV2/com_apple_MobileAsset_DictionaryServices_dictionaryOSX/*/AssetData/*.dictionary`
and `/Library/Dictionaries/`. They are licensed content, so only copy them to
machines you're entitled to use them on.

Imported dictionaries are stored in `~/.local/share/dictionary/` (Linux),
`~/Library/Application Support/Dictionary/` (macOS) or `%APPDATA%\Dictionary`
(Windows). Override this with `--library DIR` or `DICTIONARY_LIBRARY`.

Settings are saved in `settings.json` under `$XDG_CONFIG_HOME/dictionary/` when
`XDG_CONFIG_HOME` is set (on any platform), otherwise `~/.config/dictionary/`
(Linux), `~/Library/Application Support/Dictionary/` (macOS) or
`%APPDATA%\Dictionary` (Windows).

## Using

| Key | Action |
| --- | --- |
| type anywhere | search |
| ↑ / ↓ | move through results |
| Esc | clear the search |
| Ctrl+L / Ctrl+F | focus the search field |
| Alt+← / Alt+→ (Ctrl+[ / Ctrl+]) | back / forward |
| Ctrl+O | import a dictionary |
| Ctrl+, | settings: enable, disable, add and delete dictionaries |
| Ctrl+Q | quit |

`dictionary WORD` opens the app showing WORD. `--timings` prints startup timings.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/dictdb` | The `.dictdb` storage format: an `fst` key index, postings, and zstd-compressed blocks of entry HTML, all memory-mapped. |
| `crates/import` | Importers for Apple bundles (`Body.data`/`KeyText.data`) and DDK XML, plus the simplifier that turns Apple's class-styled XHTML into plain semantic HTML at import time. |
| `crates/app` | The GPUI Kit application. |

The compiled Apple format is undocumented. The importer follows the community's
reverse-engineered description of it (see the module docs in
`crates/import/src/apple_bundle/`), and reports unsupported variants as errors
instead of guessing.

## Testing

```sh
cargo test --workspace                 # unit and integration tests
cargo bench -p dictdb                  # open/search/fetch benchmarks
cargo run -p dictdb --example dump -- ~/.local/share/dictionary/X.dictdb word
```

Put real bundles in `testdata/` (git-ignored) and `cargo test -p import` will
import each one.
