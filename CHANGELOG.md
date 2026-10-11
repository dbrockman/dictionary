# Changelog

All notable changes to this project are documented in this file. The project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2](https://github.com/dbrockman/dictionary/compare/v0.1.1...v0.1.2) - 2026-10-11

### Added

- Add custom dictionary names and show dictionary paths in settings

## [0.1.1](https://github.com/dbrockman/dictionary/compare/v0.1.0...v0.1.1) - 2026-10-11

### Added

- Add typo-tolerant search for misspelled queries

### Changed

- Bold the matched part of result labels

## [0.1.0](https://github.com/dbrockman/dictionary/releases/tag/v0.1.0) - 2026-10-11

### Added

- Add native dictionary app that imports Apple dictionaries
- Add settings page for managing dictionaries
- Show the entry in the title bar and add context menus
- Add theme settings with system appearance and a watched themes folder
- Show the version in settings and check GitHub for newer releases

### Changed

- Rank headwords above phrases in search results
- Import dictionaries only from the settings page
- Label results by entry and make the result list resizable
- Use scope tabs and a single-page settings layout
- Make result selection visible and explain dictionary switches
- Replace the status footer with notifications and domain row ids
- Name the query in empty states and show shortcuts in tooltips
- Release with release-plz and a git-cliff changelog
