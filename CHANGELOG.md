# Changelog

## 0.1.0 — 2026-06-10

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
- `hackmd tui` now opens this TUI instead of the previous minimal
  two-pane viewer — straight into the cloud browser when logged in,
  the local cwd browser otherwise.

### Changed

- `dirs` 5 → 6.
- The crate version jumps 0.0.2 → 0.1.0 to mark the merge.

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
