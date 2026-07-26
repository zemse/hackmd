# hackmd

[![crates.io](https://img.shields.io/crates/v/hackmd.svg)](https://crates.io/crates/hackmd)
[![docs.rs](https://img.shields.io/docsrs/hackmd)](https://docs.rs/hackmd)
[![license](https://img.shields.io/crates/l/hackmd.svg)](#license)

A terminal markdown editor that renders what other terminal editors leave as source. Works on local files, and on your [HackMD](https://hackmd.io) notes.

![Split live-preview editor with a mermaid diagram rendered as Unicode art](https://raw.githubusercontent.com/zemse/hackmd/main/assets/editor-split-view.png)

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

## What it does

**Mermaid diagrams become Unicode art.** A ` ```mermaid ` block renders as a real box-drawing diagram in the preview pane (sequence, flowchart, class, ER, xychart). The editor pane keeps the raw source, so it stays editable while you watch the diagram redraw.

**Images render inline.** Not as `[image: …]` placeholders. Local files, remote URLs (downloaded and cached), and SVGs (rasterized) all draw through the terminal's graphics protocol on kitty, iTerm2, or sixel.

**Markdown files present as slide decks.** Set `marp: true` and the document opens as a [Marp](https://marp.app) presentation: arrows switch slides, headers and footers and page numbers carry across, and `![bg left:40%](…)` reserves half the slide for an image and reflows the text beside it. While editing, the preview pane shows just the slide your cursor is in.

**Your HackMD notes sync both ways.** `Ctrl-S` pushes and pulls in one keystroke. When both sides changed, a 3-way merge reconciles them, and only genuinely overlapping edits open a side-by-side resolver where you pick per hunk. `H` toggles between local files and the cloud.

**Commit without leaving the editor.** `gc` opens a commit screen listing every uncommitted file with its `+adds -dels`, pre-checking the ones you were looking at. Check what you want, write a multi-line message, `Enter`. Unrelated changes stay untouched.

**Selections copy as markdown, not as text.** Drag across a rendered link and you get `[label](url)` back, with delimiters balanced. On macOS, double-clicking a word opens the system dictionary.

Also here: a scriptable CLI with `@hackmd/hackmd-cli` parity and `table/json/csv/tsv/yaml` output, and the async Rust SDK it sits on (`cargo add hackmd --no-default-features`).

## Docs

CLI commands, SDK usage, and config live on [docs.rs](https://docs.rs/hackmd). Press `?` in the TUI for the full key map.

## Credits

The TUI is [md-tui](https://github.com/zemse/md-tui), merged in and extended with HackMD cloud mode. The CLI and SDK port the official [`hackmd-cli`](https://github.com/hackmdio/hackmd-cli) and [`api-client`](https://github.com/hackmdio/api-client).

## License

MIT
