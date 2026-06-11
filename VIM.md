# Vim / Helix support roadmap

Commands and motions worth supporting in the editor, collected from vim and
helix. Checked items are implemented and covered by tests. The editor opens
in insert mode (the GUI-editor default); Esc moves to the command line on
the statusline instead of a full normal mode — normal-mode items below are
future work toward that.

## Command line (`:` after Esc)

- [x] `:w` — write buffer to disk / cloud, stay in editor
- [x] `:wq` — write and quit the editor (back to preview)
- [x] `:x` — alias of `:wq`
- [x] `:q` — quit the editor, discarding unsaved changes
- [x] `:q!` — alias of `:q`
- [x] `:preview` — full-screen preview of the unsaved buffer (Esc returns)
- [x] Tab — accept the grey inline completion (e.g. `:pre` → `:preview`)
- [x] Esc on the command line — discard changes and exit (non-vim escape)
- [x] Backspace on empty command — back to insert mode
- [ ] `:N` — jump to line N
- [ ] `:s/old/new/` and `:%s/old/new/g` — substitute
- [ ] `:e!` — reload buffer from disk, discarding edits (stay editing)
- [ ] command history (↑/↓ on the command line)

## Entering / leaving edit

- [x] `e` from the preview — open editor (insert mode)
- [x] `i` from the preview — same as `e` (vim muscle memory)
- [ ] `o` / `O` — open editor with a new line below / above
- [ ] `A` — open editor with cursor at end of current line

## Insert-mode editing (modern-GUI flavor)

- [x] mouse click places the source cursor; wheel scrolls
- [x] Alt-←/→ (Ctrl- on Linux/Win) — word jump
- [x] Alt-Backspace / Alt-Delete — delete word back / forward
- [x] Ctrl-Z / Ctrl-Y (or Ctrl-R) — undo / redo
- [x] Ctrl-S — save in place (Ctrl-W fallback for XOFF terminals)
- [x] Home / End — line start / end
- [ ] Cmd/Ctrl-←/→ — line start / end (GUI style)
- [ ] auto-indent continuation for lists (`- ` / `1. ` on Enter)
- [ ] bracket/emphasis pair completion (`*`, `_`, `[`, `(`, `` ` ``)

## Normal-mode motions (future — vim/helix navigation layer)

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
