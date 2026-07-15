# hackmd

[![crates.io](https://img.shields.io/crates/v/hackmd.svg)](https://crates.io/crates/hackmd)
[![docs.rs](https://img.shields.io/docsrs/hackmd)](https://docs.rs/hackmd)
[![license](https://img.shields.io/crates/l/hackmd.svg)](#license)

Terminal markdown editor for [HackMD](https://hackmd.io) — plus a full CLI and async Rust SDK.

![Split live-preview editor](https://raw.githubusercontent.com/zemse/hackmd/main/assets/editor-split-view.png)

## Install

```sh
cargo install hackmd
```

```sh
hackmd                # your hackmd.io notes when logged in, else browse the cwd
hackmd login          # paste an access token (from hackmd.io/settings#api)
hackmd new my note    # create a cloud note and open it in the editor
```

## Highlights

- **Split live-preview editor** — raw markdown on one side, rendered preview on the other, scroll-synced. `e` to edit, `Ctrl-S` to save.
- **Your HackMD notes in the terminal** — browse, read, edit, publish, and download notes (and team notes) without leaving the shell. `H` toggles local ↔ cloud.
- **Two-way local ↔ cloud sync** — `U` publishes a local file and keeps it in sync, three-way merging edits from both sides with a per-hunk conflict resolver.
- **Rich vim-style reader** — fast scrolling, search, table of contents, inline images (kitty/iTerm2/sixel), mermaid diagrams rendered as Unicode art, expandable tables, clickable links, and a git diff lens.
- **Marp presentations** — a `marp: true` markdown file opens as a slide deck: arrows/`Space` switch slides, `Home`/`End` jump to first/last, `p` toggles back to the scrolling reader. Splits on `---`, hides directives/notes, honours `header`/`footer`/`paginate`/`_class: lead`.
- **Scriptable CLI** — `@hackmd/hackmd-cli` parity with `table/json/csv/tsv/yaml` output for notes, teams, and folders.
- **Async Rust SDK** — the full HackMD v1 API with ETag caching and retries (`cargo add hackmd --no-default-features`).

## Docs

CLI commands, SDK usage, and config are documented on [docs.rs](https://docs.rs/hackmd). Press `?` in the TUI for the full key map.

## Credits

The TUI is [md-tui](https://github.com/zemse/md-tui), merged in and extended with HackMD cloud mode. The CLI and SDK port the official [`hackmd-cli`](https://github.com/hackmdio/hackmd-cli) and [`api-client`](https://github.com/hackmdio/api-client).

## License

MIT
