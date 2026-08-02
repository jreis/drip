# drip

Local-first TUI for [Raindrop.io](https://raindrop.io) bookmarks.

Import a CSV export or pull via the API, fuzzy-search with `/`, filter the pile, open links, mark what you’ve actually read — and stop treating bookmarks as a write-only archive.

**Not affiliated with Raindrop.io.** Personal open-source tool, MIT licensed, no warranty.

## Why

Most of us capture faster than we revisit. `drip` is a terminal surface for *revisit*: local status, dead-link checks, domain/year scrubbing, and a daily dig of forgotten saves. Raindrop stays the capture hub; drip is the local reading desk.

## Features

- **TUI** with vim-ish navigation and fuzzy search
- **Local status** — unread → reading → done → skipped (not pushed back to Raindrop)
- **CSV import** and **OAuth API sync** (pull-only; merge preserves local progress)
- **Dead-link detection** — concurrent HEAD/GET, filter by health
- **Today’s dig** — a few never-opened, not-dead picks on launch
- **Domain browser** and **year scrub** for cleaning large libraries
- **Serendipity** — random never-opened pick (`r`)

## Install

```bash
# from source (this repo)
cargo install --path .

# once published
cargo install --git https://github.com/jreis/drip
```

Requires a recent stable Rust toolchain.

## Quick start

**Bootstrap (recommended for large libraries):** export CSV from Raindrop, then import once. API full sync is rate-limited (~120 req/min) and slow for tens of thousands of items.

```bash
drip import ~/Downloads/raindrop-export.csv
drip stats
drip
```

**Ongoing sync** (new/changed bookmarks only):

```bash
# one-time OAuth — create an app at https://app.raindrop.io/settings/integrations
# set redirect URL exactly to:  http://127.0.0.1:8787/callback
drip auth --client-id YOUR_ID --client-secret YOUR_SECRET

drip sync              # incremental (uses last sync or CSV import time)
# drip sync --full     # full re-crawl — only when you need it; respects rate limits
```

In the TUI, press **`S`** for an incremental pull.

## Auth & sync

| Command | Purpose |
|---------|---------|
| `drip auth` | Interactive OAuth (saves client id/secret + tokens) |
| `drip auth --status` | Show config path and token state |
| `drip auth --token …` | Use a long-lived Test token instead of OAuth |
| `drip auth --logout` | Clear tokens (keeps app credentials) |
| `drip sync` | Incremental pull |
| `drip sync --full` | Full library pull (throttled + retries on 429) |
| `drip sync --full --prune` | Full pull and drop local ids missing remotely |

- Sync is **pull-only**. Local reading status, open counts, and link-health results are **kept** (merge by Raindrop id).
- After a CSV import, incremental sync uses import time as the watermark if no API sync has run yet.
- Raindrop rate limit is roughly **120 requests/minute**; full sync of ~20k bookmarks needs hundreds of pages — drip throttles and backs off on HTTP 429.

Config is stored with mode `0600` (Unix). Paths:

| | macOS (typical) |
|--|-----------------|
| Config | `~/Library/Application Support/io.drip.drip/config.json` |
| Library | `~/Library/Application Support/io.drip.drip/library.json` |

Run `drip auth --status` / `drip stats` to print the resolved paths on your machine.

Env override: `RAINDROP_TOKEN` (test/access token).

## Dead-link detection

```bash
drip check --limit 100              # unchecked only (default)
drip check --only dead              # recheck known dead
drip check --only all --concurrency 16
```

| Key (TUI) | Action |
|-----------|--------|
| `c` | Check selected |
| `C` | Recheck all in current view |
| `ctrl-c` | Check unchecked in view |
| `l` | Cycle link filter |
| `x` | Cancel running checks |

Glyphs: `?` unchecked · `●` alive · `✗` dead · `!` error · `↗` redirect

## Keybindings

| Key | Action |
|-----|--------|
| `j` / `k` · `↑`/`↓` | Move |
| `ctrl-d` / `ctrl-u` | Page |
| `g` / `G` | Top / bottom |
| `/` | Fuzzy search |
| `esc` | Clear search / close overlay / clear filters |
| `f` | Cycle folder |
| `s` | Cycle status filter |
| `l` | Cycle link-health filter |
| `D` | Domain browser |
| `.` | Filter to domain of selection |
| `Y` | Year scrub picker |
| `0` | Clear all filters |
| `enter` | Open URL |
| `y` | Copy URL |
| `space` | Cycle local status |
| `d` | Mark done |
| `n` | Skip |
| `r` | Random never-opened (skips dead) |
| `z` | Today’s dig |
| `c` / `C` | Link check selected / view |
| `S` | Sync from Raindrop |
| `w` | Save library now |
| `?` | Help |
| `q` | Quit (auto-saves if dirty) |

## Privacy

- Bookmarks and progress live **on your machine**.
- OAuth tokens and client secret stay in local config — **never commit them**.
- Each user registers their **own** Raindrop integration app (or uses a Test token).
- `drip` does not send your library anywhere except Raindrop’s API when you run sync/auth.

## Status

This is a **personal project** shared as open source. Best-effort maintenance; no SLA. Bug reports and small PRs welcome.

See [CHANGELOG](CHANGELOG.md) and [CONTRIBUTING](CONTRIBUTING.md).

## License

[MIT](LICENSE) © Jason Reis
