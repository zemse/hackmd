# hackmd

[![crates.io](https://img.shields.io/crates/v/hackmd.svg)](https://crates.io/crates/hackmd)
[![docs.rs](https://img.shields.io/docsrs/hackmd)](https://docs.rs/hackmd)
[![license](https://img.shields.io/crates/l/hackmd.svg)](#license)

A terminal markdown editor for local files and your [HackMD](https://hackmd.io) notes, written in Rust.

![Split live-preview editor with a mermaid diagram rendered as Unicode art](https://raw.githubusercontent.com/zemse/hackmd/main/assets/editor-split-view.png)

**Features**

- **Mermaid diagrams** render as Unicode box-drawing art instead of raw source
- **Split live preview**, markdown on the left, rendered on the right, scroll-synced
- **Your HackMD notes** open, edit and browse in the terminal; `H` toggles local and cloud
- **Two-way sync** on every `Ctrl-S`, 3-way merged, with a per-hunk resolver only when edits truly collide
- **Publish** a local file to a public URL in one keystroke, link waiting on your clipboard
- **Heading anchors** autocomplete inside `[](other-note.md#…)` links, reading the file you point at
- **Click a heading** to copy a link to that section, click that link to land on it
- **Commit without leaving** the editor with `gc`, per-file checkboxes, unrelated changes untouched
- **Git lens** on `Ctrl-G` diffs the open file against `HEAD` inline
- **Selections copy as markdown**, so a copied link pastes back as `[label](url)`
- **Scriptable CLI** with `@hackmd/hackmd-cli` parity and `table/json/csv/tsv/yaml` output
- **Async Rust SDK** underneath, the full HackMD v1 API with ETag caching and retries

## Install

```sh
cargo install hackmd
```

```sh
hackmd                # your hackmd.io notes when logged in, else browse the cwd
hackmd .              # browse a local directory
hackmd login          # paste an access token (from hackmd.io/settings#api)
hackmd new my note    # create a cloud note and open it in the editor
```

## Docs

CLI commands, SDK usage, and config live on [docs.rs](https://docs.rs/hackmd). Press `?` in the TUI for the full key map.

## Credits

The TUI is [md-tui](https://github.com/zemse/md-tui), merged in and extended with HackMD cloud mode. The CLI and SDK port the official [`hackmd-cli`](https://github.com/hackmdio/hackmd-cli) and [`api-client`](https://github.com/hackmdio/api-client).

## License

MIT
