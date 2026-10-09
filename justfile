# Development tasks. Run `just` to list them.
#
# Recipes run inside the Nix dev shell automatically, so they also work from a
# plain terminal; inside `nix develop` they run directly.

nix := if env("IN_NIX_SHELL", "") == "" { "nix develop --command" } else { "" }

# Where `just import-data` puts dictionaries (defaults to the app's library).
library := env("DICTIONARY_LIBRARY", "")
library_arg := if library == "" { "" } else { "--library " + quote(library) }

# List the recipes
default:
    @just --list --unsorted

# Build everything (debug)
build:
    {{ nix }} cargo build --workspace --all-targets

# Build the optimized app binary (target/release/dictionary)
release:
    {{ nix }} cargo build --release -p app

# Run the app; extra arguments go to it, e.g. `just run hello` or `just run --timings`
[positional-arguments]
run *args:
    {{ nix }} cargo run --release -p app -- "$@"

# Import dictionaries into the library, e.g. `just import "data/Swedish.dictionary"`
[positional-arguments]
import +paths:
    {{ nix }} cargo run --release -p app -- {{ library_arg }} import "$@"

# Import every bundle in data/ into the library
import-data:
    {{ nix }} cargo run --release -p app -- {{ library_arg }} import data/*.dictionary

# Run all tests (real bundles in data/ are imported too, when present)
[positional-arguments]
test *args:
    {{ nix }} cargo test --release --workspace "$@"

# Import the real bundles in data/ and print their statistics
test-data:
    {{ nix }} cargo test --release -p import --test apple_bundle real_bundles -- --nocapture

# Run the dictdb open/search/fetch benchmarks
bench:
    {{ nix }} cargo bench -p dictdb

# Format the code
fmt:
    {{ nix }} cargo fmt --all

# Check formatting and lints without changing anything
lint:
    {{ nix }} cargo fmt --all --check
    {{ nix }} cargo clippy --workspace --all-targets -- -D warnings

# Everything CI should run: lint, then test
check: lint test

# Update Cargo dependencies within their semver ranges, and the Nix flake inputs
update:
    {{ nix }} cargo update
    nix flake update

# Show dependency updates: compatible ones (`just update` applies them) and ones Cargo.toml blocks
outdated:
    {{ nix }} cargo update --dry-run --verbose 2>&1 | grep -E "Updating .* ->|available:" || echo "Everything is up to date."

# Print a dictionary's metadata, search hits and first entry, e.g. `just dump ~/.local/share/dictionary/com.apple.dictionary.ODE.dictdb make`
[positional-arguments]
dump db *word:
    {{ nix }} cargo run --release -p dictdb --example dump -- "$@"

# Print the raw and simplified XHTML of an entry in an Apple bundle, e.g. `just raw-entry data/Swedish.dictionary hus`
[positional-arguments]
raw-entry bundle title:
    {{ nix }} cargo run --release -p import --example raw_entry -- "$@"

# Remove build artifacts
clean:
    {{ nix }} cargo clean
