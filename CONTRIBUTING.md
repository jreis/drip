# Contributing

Thanks for taking an interest in `drip`.

This is a **personal tool** published so others can use and fork it. There is no support SLA.

## Useful contributions

- Bug fixes (especially auth, sync merge, and TUI crashes)
- Docs clarifications
- Rate-limit / resume improvements for large libraries
- Tests around merge and CLI parsing

Less useful without discussion first: large refactors, push-to-Raindrop features, or new UI frameworks.

## Dev setup

```bash
git clone https://github.com/jreis/drip
cd drip
cargo build
cargo test
cargo run -- stats
```

Please run before opening a PR:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## Security

- Never commit `config.json`, tokens, client secrets, or real bookmark exports.
- If you find a security issue that could leak tokens, open a private advisory or email the maintainer rather than filing a public issue with secrets.

## Code style

- Prefer small, readable modules over clever abstractions.
- Keep the default path local-first: Raindrop is optional after CSV import.
