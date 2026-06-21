# Tasks

Pending work only. The SDK/CLI port and the md-tui merge (M0–M7) are shipped;
their plans were dropped. API/rate-limit reference lives in `RESEARCH.md`.

Recently shipped and removed from this list: editor cut/paste & pair
completion, list continuation, type-ahead find, create/rename in the browser,
local↔HackMD publish + bidirectional sync with a conflict resolver, H1 title
inference, `:s///` substitute, command history, and `A`/`O` editor entry.

## User suggested tasks

- [ ] publish a note / change its visibility from inside the editor or preview. `P` already toggles visibility from the cloud browser and reader, but not while a cloud note is open in the editor (the key is captured as input there).

## Editor — entering / leaving edit

- [ ] `o` — open the editor with a new line *below* the current one. `A` (append at end) and `O` (open at top) shipped; lowercase `o` still needs a non-conflicting trigger since it opens links in the reader.

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
- [ ] Sync round-trip: `U` a local file, edit it on hackmd.io, confirm the change merges back locally on the next poll; make conflicting edits on both sides and resolve them in the conflict pane
- [ ] No token: `H` shows the login instruction, app stays usable locally
- [ ] `cargo run --features tui -- tui` opens the cloud view via the `hackmd` bin

## Sync hardening — remaining edge cases (from a code review)

The high-impact sync bugs are fixed (cloud-edit no longer reverts a linked
file; a hand-written `<!-- hackmd-sync` marker is no longer eaten; a corrupted
link block warns instead of duplicating; a team note recovers its `team:` from
cache; CRLF remote no longer spuriously conflicts; skipped pushes report
"deferred" rather than "synced"). These narrower ones are left:

- [ ] Two local files linked to the same note id share one base (`<root>/.hackmd/<id>.base` is keyed by id, not path) — a second linked copy can clobber the first's base. Would need per-(id,path) base keying.
- [ ] First-publish and conflict-resolve write a snapshot captured a moment earlier; an external on-disk edit in that sub-second window is lost. Narrow race; would need a re-read + recheck before the write.
- [ ] Missing base cache (`.hackmd/<id>.base` deleted) turns any local≠remote difference into a whole-file conflict. Equal sides already merge clean and rebuild the base; only the genuinely-diverged case is noisy. A "no base" path could offer take-local / take-remote instead of a giant conflict.
- [ ] `parse_conflicts` keys conflict markers off `starts_with(repeat(7))`, so a user line beginning with 7+ `<`/`=`/`>`/`|` chars could be misread as a marker. Exotic; would need fenced-code / exact-marker awareness.

## HackMD comments (API gap)

The v1 REST API has no comment/reaction endpoints — see `RESEARCH.md` and
upstream issue [hackmdio/hackmd-io-issues#428](https://github.com/hackmdio/hackmd-io-issues/issues/428).
The README now documents that comments aren't fetchable.

- [ ] Add weight to #428: 👍 + a comment as a third-party Rust SDK/CLI consumer (outward action — needs a go-ahead before posting)

## Future (post-v0.1, was out of scope in the original plan)

- [ ] Image upload — `POST /notes/{id}/images` (SDK + CLI)
