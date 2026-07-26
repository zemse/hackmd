# Changelog

## Unreleased

### Added

- **Marp presentations** — a local markdown file that enables
  [Marp](https://marp.app) now opens as a slide deck instead of a scrolling
  document. A deck is recognized by a `marp: true` line, a leading
  `<!-- marp: true -->` comment, or (like the canonical marp.app example, which
  omits `marp: true`) any Marp-distinctive front-matter directive — `paginate`,
  `backgroundColor`/`backgroundImage`, or a `_`-prefixed spot directive — while
  ordinary Jekyll/Hugo front matter stays a plain document. Slides split on `---` page breaks; front-matter and
  `<!-- ... -->` directives/presenter-notes are hidden from the render. Arrow
  keys, `Space`, and PageUp/PageDown switch slides (they don't scroll);
  `Home`/`End` jump to the first/last slide; the status bar shows `Slide n/m`.
  Global `header`/`footer`/`paginate` directives carry across slides (locals like
  `_class: lead` apply to one slide and center it), and the page number renders
  bottom-right when `paginate` is on. `p` (or `Esc`) toggles between the deck and
  the ordinary scrolling reader. A `---` inside a fenced code block no longer
  splits the deck, and front-matter spot directives (`_class: lead`) scope to the
  first slide.
- **Live slide preview while editing a deck** — the split editor's preview pane
  now shows the single slide the cursor is in (rendered as a slide, with
  centering, header/footer and a `n / total` label) instead of the whole
  document, and follows the cursor across slides as you type or move.
- **Marp background images** — a `![bg left:40%](url)` / `![bg right](url)`
  background now reserves that fraction of the slide for the image and flows the
  slide text into the remaining column (the image is pulled out of the text flow
  instead of rendering as an inline `[image: …]` placeholder). Plain `![bg](url)`
  fills the slide. The text re-wraps to the narrower content width.
- **Remote and SVG images in the reader** — images now load off the render
  thread: remote `http(s)://` assets are downloaded (and cached), and `.svg`
  sources are rasterized (via `resvg`) before being handed to the terminal
  graphics protocol (kitty / iTerm2 / sixel). Previously only local raster files
  rendered; a Marp deck's remote SVG logos now appear too. Terminals without a
  graphics protocol still show the `[image: …]` placeholder.
- **Mermaid diagrams in the view pane** — a ` ```mermaid ` fenced block now
  renders as a Unicode box-drawing diagram in the reader instead of raw source
  (sequence, flowchart, class, ER, and xychart types via `merman-core` +
  `merman-ascii`). The editor still shows the raw source so it stays editable,
  and any unsupported diagram type or parse error falls back to the normal
  highlighted code block.
- **Git commit from the TUI** — uncommitted files (modified, added, deleted,
  untracked, renamed) now carry an `[uncommitted]` badge in the browser, on
  both files and the folders containing them. Press `gc` on an open file, a
  selected file, or a selected folder to open a commit screen: it lists every
  uncommitted file in the repo with per-file `+adds -dels` counts, pre-checks
  the ones in the current selection's scope, and scrolls when the list
  overflows. `Space` toggles a file, `a` toggles all, `Tab` moves between the
  file list and the message box, `Enter` commits the checked files, `Esc`
  cancels. The message box is multi-line: `Shift+Enter` (or `Alt+Enter` where
  the terminal can't report Shift+Enter) inserts a newline and the box grows to
  fit. Commits touch only the checked paths, leaving other changes untouched.
- **Create folders and move entries in the browser** — ending a `n` (new) name
  with `/` creates a folder instead of a file and browses straight into it, so
  `notes/drafts/` makes both levels at once. `m` moves the selected file or
  folder: the destination is read relative to the browsed directory, Tab
  ghost-completes existing folders, a trailing `/` (or a name that already is a
  folder) means "into there, keep the name", and anything else is the full new
  path, so a move can rename in the same keystroke. Existing destinations are
  never clobbered and a folder can't be moved inside itself.

### Changed

- **Browser keys**: `r` renames (was `c`, `F2` still works), `m` moves, `c`
  commits (the `gc` chord is unchanged), and `R` marks read (was `r`). `m` still
  toggles mouse capture everywhere outside the browser, where `M` now does it.
- Empty directories are listed in the browser. They hold nothing to read, but
  hiding them made a folder just created (with `n`, or with `mkdir` outside the
  app) look like it had failed.

### Fixed

- Renaming or moving a HackMD-linked file no longer orphans its sync base
  snapshot. The cache is keyed by the file's path, so the next sync used to find
  no common ancestor and turn a clean pull into a whole-file conflict.
- Renaming or moving a file no longer resets it to `[unread]`, and history
  entries pointing at it are rewritten so `h`/Back doesn't dead-end on the old
  path.
- The browser now notices a change one level down (a folder gaining its first
  markdown file), which previously left the folder hidden until something else
  in the listed directory happened to change.

## 0.1.0 (2026-06-25)

The md-tui merge: one crate now ships the HackMD SDK/CLI **and** a full
terminal markdown reader/editor behind `hackmd tui`.

### Added

- **Full TUI** (`--features tui`, run as `hackmd tui`) —
  [md-tui](https://github.com/zemse/md-tui) v0.3.0 merged in: file/directory
  browser (gitignore-aware, read/unread badges), vim-style reader with
  mouse + clickable links, inline images, tables with click-to-expand,
  JSON-line pretty-printing, in-document and fuzzy file search, git lens,
  and a split live-preview editor with undo/redo. Existing
  `~/.config/md/config.toml` files keep working. The standalone `md`
  binary is not shipped for now (may return later).
- **HackMD cloud mode in the TUI** — `H` (browsers) / `gh` (anywhere)
  toggles between local files and hackmd.io: browse own + team notes
  (`[pub]` badges), open with ETag-cached fetches and background
  revalidation, edit with `Ctrl-S` PATCH (pessimistic dirty marker),
  checkbox toggles sync optimistically, `n` create, `D` delete (with
  confirm), `P` publish/unpublish via `readPermission`, `y`/`o`
  copy/open the publish link, `S` download to a local file, `U` push a
  local file up as a new note. All API calls run on background tasks
  drained each UI tick — the interface never blocks on the network.
- **Continuous bidirectional local ↔ HackMD sync** — a `diffy` 3-way
  merge with a git-style conflict resolver and a version-stable
  base-content cache, so edits made in both places reconcile instead of
  clobbering each other.
- **Editor refinements** — drag-select with copy-as-markdown / cut /
  delete, shift-click and shift+arrow range selection, `:s///`
  substitute, command-line history, `:N` line jump, `:e!` reload,
  bracket/emphasis pair completion, `Ctrl+Up`/`Down` line move, list
  continuation, paste-as-link, scrolloff + page padding, crash-recovery
  autosave to `/tmp`, GUI clipboard cut/copy/paste, and
  confirm-before-discard on unsaved edits.
- **Double-click dictionary lookup** (macOS) with the looked-up word
  highlighted while the popover is open.
- **Autolink bare URLs** in rendered markdown.
- **Browser** — text-file detection, show-all toggle, a last-modified
  column with `s` to sort by recency, and a UTF-8 error modal.
- `hackmd tui` now opens this TUI instead of the previous minimal
  two-pane viewer — straight into the cloud browser when logged in,
  the local cwd browser otherwise.

### Changed

- `dirs` 5 → 6.
- The crate version jumps 0.0.2 → 0.1.0 to mark the merge.

### Fixed

- Pin `time` below 0.3.48 so a non-locked `cargo install` builds.
- Quote CSV fields containing carriage returns (RFC 4180).
- Sync hardening (revert, comment-eating, dupes); in-document
  search-match highlight inside wider spans; wide-char selection edges;
  long-line wrap clipping; fall back to plain text instead of panicking
  on a missing theme.

### Removed

- The old minimal `hackmd tui` implementation (list + `$EDITOR`
  round-trip), superseded by the in-app split editor.

## 0.0.2 — 2026-05-25

- SDK: full HackMD v1 API surface (user, notes, teams, team-notes,
  folders, folder-order), ETag-aware GETs, retry with exponential
  backoff, 429 rate-limit parsing.
- CLI: parity with `@hackmd/hackmd-cli` — login/logout/whoami, history,
  export, teams, notes + team-notes CRUD, `--output table|json|csv|yaml`.
- Minimal opt-in TUI (`hackmd tui`).

## 0.0.1 — 2026-05-25

- Initial release.
