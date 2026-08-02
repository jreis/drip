# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] — 2026-08-02

### Added

- TUI browser for Raindrop bookmarks (ratatui): search, filters, detail pane
- CSV import with merge of local status / opens / link health by id
- OAuth login (`drip auth`) with refresh tokens; Test token and `RAINDROP_TOKEN` support
- Pull-only API sync (`drip sync`) — incremental by default; `--full` / `--prune`
- Rate-limit aware API client (throttle + retry on HTTP 429)
- Local reading status: unread / reading / done / skipped
- Dead-link checks (`drip check` and TUI `c` / `C`)
- Today’s dig, domain browser, year scrub, random pick
- Stats CLI and auto-save of dirty library on quit

### Notes

- Not affiliated with Raindrop.io
- First public release
