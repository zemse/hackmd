# Tasks

Pending work only. The SDK/CLI port and the md-tui merge (M0–M7) are shipped;
their plans were dropped. API/rate-limit reference lives in `RESEARCH.md`.

## User suggested tasks

- [x] When working with a bulleted list, new line should create bullet list, similarly patterns like number, alphabets for numbered ordering stuff should also happen when user press enter it should add with the next numbering. also "- [ ]" tick box should also repeat similar to the bullet list. Enter now auto-continues `-`/`*`/`+` bullets, `1.`/`1)` numbered (incrementing), `a.`/`A)` alpha (advancing), and `- [ ]`/`- [x]` checkboxes (new item unchecked); Enter on an empty marker terminates the list (`list_continuation` in `app.rs`).
- [x] in the file/dir browser screen, type-ahead find. Implemented lf-style: `f` arms find mode, then typed chars build an anchored, smartcase prefix query and the selection live-jumps; `;`/`,` cycle next/prev match, `Enter` opens, `Esc` cancels. (Chose a trigger key over bare letters — the vim-TUI canon, e.g. lf/ranger/vifm/yazi — to avoid colliding with the single-letter command bindings.)
- [x] create a new file (`n`) and rename an entry (`c` / F2) in the local browser; new file opens straight into the editor.
- [x] publish a local file to HackMD (`U`) — stamps the file with a managed `<!-- hackmd … -->` link block (id, url, publish link, synced time) and re-pushes update the linked note instead of creating a duplicate (`src/tui/hackmd_meta.rs`).
- [x] continuous bidirectional sync between a linked local file and its HackMD note. Three-way merge (`diffy`) against a cached base (`<root>/.hackmd/<id>.base`); fully automatic on open, on save, and on a background poll (`SYNC_INTERVAL`, 15s). The note title is inferred from the first H1 (prompt only when absent). Overlapping edits open a dedicated conflict resolver pane (side-by-side local/upstream, per-hunk `l`/`u`/`b`/`n`, Enter applies). Base advances only on the server's `Saved` confirmation so a re-sync can't revert a just-resolved change (`src/tui/sync.rs`).
- [x] editor drag-selection now supports `x` cut and `p` / Ctrl-V paste-over (replace selection with clipboard) alongside the existing `y` copy / Del delete.
- [ ] ability to publish a note or change visibility of note from the editor or preview (cloud notes; `P` already toggles visibility from the cloud browser/reader)

## Bug fixes

- [x] editor pane dropped text after a long unbreakable token. `wrap_to_width` (`src/tui/markdown.rs`) reset `col` to 0 on a whitespace wrap but left the scan head `ci` past the break point, so `col` under-counted for the rest of the line and no further wrap fired. A line with a long token (e.g. a markdown link URL like `[PR 1165](https://github.com/leanEthereum/leanSpec/pull/1165)`) followed by more words collapsed onto one overwide row, which the non-wrapping editor `Paragraph` clipped at the pane edge — text visible in the preview went missing in the edit pane. Fix rewinds `ci` to the break boundary so the new line's width recomputes from `break_at`; added regression tests asserting no wrapped chunk exceeds the width and that chunk byte ranges tile the input contiguously (commit `673716e`).

## Editor — vim/helix command line (`:` after Esc)

- [ ] `:s/old/new/` and `:%s/old/new/g` — substitute
- [ ] command history (↑/↓ on the command line)

## Editor — entering / leaving edit

- [ ] `o` / `O` — open editor with a new line below / above
- [ ] `A` — open editor with cursor at end of current line

## Editor — insert-mode editing (modern-GUI flavor)

- [ ] Cmd/Ctrl-←/→ — line start / end (GUI style)
- [x] auto-indent continuation for lists (`- ` / `1. ` on Enter) — see the user-suggested item above
- [ ] bracket/emphasis pair completion (`*`, `_`, `[`, `(`, `` ` ``)

## Editor — normal-mode motion layer (future)

The editor opens in insert mode; Esc goes to the command line, not a full
normal mode. This whole layer is future work toward vim/helix navigation.

- [ ] `h j k l` — char/line movement without leaving normal mode
- [ ] `w b e` — word motions
- [ ] `0 $ ^` — line start / end / first non-blank
- [ ] `gg G` — buffer top / bottom
- [ ] `{ }` — paragraph motions
- [ ] counts on motions (`5j`, `3w`)
- [ ] `dd yy p` — delete / yank / paste line
- [ ] `x` — delete char, `r` — replace char
- [ ] `u` / `Ctrl-R` — undo / redo (normal-mode bindings)
- [ ] `v` visual selection + `y d` over it (helix-style select-first also fine)
- [ ] `/` search within the buffer while editing
- [ ] `.` — repeat last change

## Cloud TUI — manual end-to-end verification

Needs a human terminal; headless PTY smoke check already passed.

- [ ] With a real token: `H` → cloud list loads; open a note; edit + Ctrl-S, verify on hackmd.io; `P` publish → open publish_link; `S` download; `U` push; `n` create; `D` delete
- [ ] No token: `H` shows the login instruction, app stays usable locally
- [ ] `cargo run --features tui -- tui` opens the cloud view via the `hackmd` bin

## HackMD comments (API gap)

The v1 REST API has no comment/reaction endpoints — see `RESEARCH.md` and
upstream issue [hackmdio/hackmd-io-issues#428](https://github.com/hackmdio/hackmd-io-issues/issues/428).

- [ ] Add weight to #428: 👍 + a comment as a third-party Rust SDK/CLI consumer
- [ ] Note in README that comments aren't fetchable via the API (link #428)

## Future (post-v0.1, was out of scope in the original plan)

- [ ] Image upload — `POST /notes/{id}/images` (SDK + CLI)
