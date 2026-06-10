# Merge md-tui into hackmd: one binary for local + cloud markdown

## Context

The user owns two crates: `hackmd` (v0.0.2 — async SDK + CLI + minimal TUI for hackmd.io) and `md-tui-rs` (v0.3.0, binary `md` — a ~10k-line synchronous ratatui markdown reader/browser/split-editor for local files). Goal: one application that reads/writes local markdown exactly like `md` does today, **plus** works with cloud notes on hackmd.io (browse, read, edit, publish, create/delete, local↔cloud transfer).

**User decisions (fixed):**
- Combined app lives in the **hackmd repo**; md-tui's source is ported in, replacing the current minimal `src/tui/`.
- Ship **two binaries**: `hackmd` (existing CLI, `hackmd tui` opens the TUI in cloud view) and `md` (`required-features=["tui"]`, exact md-tui CLI surface: `md`, `md file.md`, `md dir/`, stdin, `-w/-s/-l`).
  - **Update (2026-06-10):** ship the `hackmd` binary **only** for now — the `md` bin was built (M0–M6) and then dropped post-M7 (`src/bin/md.rs` removed; recoverable from git history). May return later.
- Cloud is a **separate mode behind a keybind** (`H` in browser views, `gh` chord anywhere) — not mixed into the local file browser.
- Cloud scope v1: browse (own + team notes), read, edit (Ctrl-S PATCH), publish controls, push/download transfer, create + delete. **No comments.**

**Why comments are impossible:** the HackMD v1 REST API has no comment endpoints at all (verified in `RESEARCH.md:28-51` and the upstream JS SDK/CLI sources in `_ref/` — `commentPermission` appears only in the POST /notes create body, set-once, not even PATCHable). Comments live exclusively in the web UI over socket.io. Same for "publish": there is no publish endpoint — a note is published by setting `readPermission` to `guest`, and `publishLink` is a read-only field that always exists. v1 implements publish that way.

**Verified compatibility facts:**
- Both repos use ratatui 0.30 / crossterm 0.29 / clap 4. Only conflict: dirs 5 (hackmd) vs dirs 6 (md-tui); hackmd's single call site `dirs::home_dir()` (`src/cli/config.rs:54`) is unchanged in dirs 6 → bump.
- md-tui pins `ratatui-image = 11.0.0-alpha.2`; stable 11.0.2/11.0.4 exist → use `version = "11"` (fallback: pin the alpha, which is still publishable).
- `hackmd` `src/main.rs:11` is `#[tokio::main]` multi-thread → `block_in_place` is safe for the `hackmd tui` path.
- md-tui's modules are self-contained (no cross-repo type clashes); its `#[cfg(test)]` modules move for free.
- hackmd's crates.io `include = ["/src/**/*.rs", ...]` already covers all new files.

## Milestones (each compiles/tests green)

### [x] M0 — Cargo.toml alignment
`hackmd-rs/Cargo.toml`: version → `0.1.0`; `dirs = "6"`; add md-tui's deps **all optional** (anyhow, pulldown-cmark 0.13 no-default, syntect 5 `default-fancy`, open 5, unicode-width 0.2, walkdir 2, ignore 0.4, ratatui-image 11 no-default +serde, image 0.25 no-default +png/jpeg/gif/webp, base64 0.22, toml 1); extend `tui` feature to enable all of them; add:
```toml
[[bin]]
name = "md"
path = "src/bin/md.rs"
required-features = ["tui"]
```
(`cargo install hackmd` → only `hackmd`; `--features tui` → both bins.)
Verify: `cargo check`, `--no-default-features`, `--features tui`; `cargo tree --features tui` resolves a single ratatui 0.30.

### [x] M1 — Verbatim port (md bin works, zero cloud)
- Delete old `src/tui/{app,ui}.rs` and the body of `src/tui/mod.rs` (the `$EDITOR` round-trip is superseded by the in-app split editor; keep its spawn+mpsc pattern as the seed for M2's `cloud.rs`).
- Copy md-tui `src/{app,ui,events,markdown,config,jsonl,links,palette,read_state,syntax,theme}.rs` → `src/tui/`; rename ported `config.rs` → `md_config.rs`. Mechanical path rewrite `crate::X` → `crate::tui::X`. Keep `anyhow::Result` internally.
- New `src/tui/mod.rs`: module tree + `LaunchOpts { source, width, line_numbers, style }` + `pub fn run_blocking(opts, cloud: CloudContext) -> anyhow::Result<()>` (md-tui `main.rs:50-92` body: md_config load, theme resolve, App::new, terminal setup, panic hook, `events::run`, read_state flush, restore).
- New `src/bin/md.rs`: md-tui's `Cli` struct verbatim + `Source` selection (md-tui `main.rs:55-67`), then plain `fn main` with explicit `tokio::runtime::Runtime` (NOT `#[tokio::main]` — the sync loop owns the main thread; workers run cloud futures).
- Rewire `src/cli/commands/tui.rs`: `tokio::task::block_in_place(|| run_blocking(opts, CloudContext::with_client(client, Handle::current())))`; `hackmd tui` starts in the cloud view, falls back to `Source::Directory(cwd)` when no token.
- Keep md-tui's user config files working unchanged (`~/.config/md/config.toml`, read-state file) — zero-cost continuity; auth stays in `~/.hackmd/config.json`.
- `tests/tui_smoke.rs`: replace (old Action/App API dies) with a placeholder until M6.
Verify: `cargo run --features tui --bin md -- README.md`, dir browse, stdin; `cargo test --features tui` (ported in-file tests pass).

Git history: plain copy (cross-repo history preservation not worth subtree gymnastics); commit message records `port md-tui v0.3.0 (zemse/md-tui @ <sha>)`.

### [x] M2 — Async bridge skeleton (`src/tui/cloud.rs`)
Reuse the shipped 0.0.2 TUI pattern (spawn + `tokio::sync::mpsc::unbounded` + `try_recv` drained each 250 ms tick):
- `CloudContext { handle: Handle, client: Option<Client>, tx, rx }`; `init(handle)` resolves token via `cli::config::effective` and **tolerates** missing token (`client: None`, no startup error); `with_client(client, handle)`.
- `CloudMsg` enum (responses only; errors cross as `String`): `Lists`, `Note { id, intent, result }`, `Saved`, `Created`, `Deleted`, `PermissionSet`. `FetchIntent::{OpenReader{scroll}, DownloadTo(PathBuf), Revalidate{etag}}`; `CreateIntent::{Blank, PushedFrom(PathBuf)}`.
- Spawn helpers: `spawn_fetch_lists` (notes + teams + per-team team_notes in one task), `spawn_fetch_note` (uses ETag-aware `client.note`), `spawn_save{,_team}` (`update_note_content`), `spawn_create`, `spawn_delete{,_team}`, `spawn_set_read_permission` (`update_note` with `read_permission`).
- `App` gains `cloud: CloudState { ctx, lists, note_cache: HashMap<id, CachedNote{note, etag}>, saving: HashSet<id>, pending: u32, ... }`; `App::new` takes the `CloudContext` arg (both bins pass it).
- Hook in `events::run` (`events.rs:23`): `app.drain_cloud_msgs()` beside `poll_external_change()`. Statusline shows "⟳ syncing…" while `pending > 0`; errors go to the existing `app.status`.

### [x] M3 — Cloud state model + cloud browser + H toggle
In `src/tui/app.rs`:
- `EntryKind` (+`CloudList`, `CloudNote{id,title}`); `ReaderOrigin` (+`CloudNote{id, title, team_path, publish_link, read_permission, etag}`); `View` (+`Cloud(CloudBrowser)` — flat list grouped by "My notes"/team headers, `[pub]` badge; folders deferred).
- `App::load`: `CloudList` builds from `cloud.lists` (spawns fetch if absent); `CloudNote` from `note_cache` synchronously, else `pending_nav = Some((id, scroll))` + spawn fetch and **stay put** — history is pushed only when `CloudMsg::Note` completes the navigation (failed fetch leaves history clean; stale responses dropped by id mismatch). `Reader::from_cloud(&SingleNote)` next to `from_file` (app.rs:1580).
- `H` toggles local↔cloud **in browser views only** (preserves vim viewport-`H` in Reader, events.rs:271); add `gh` chord via the existing `pending_g` machinery (events.rs:130-138) to work anywhere. No token → status "No HackMD token — run `hackmd login` or set HMD_API_ACCESS_TOKEN".
- Cloud browser keys: `j/k`, `Enter` open, `Esc/b` back, `R` refetch lists.

### [x] M4 — Cloud editing
- `save_edit` (app.rs:1185) dispatches on origin: `CloudNote` → guard `saving` set, spawn content PATCH, **keep `dirty` until `Saved{Ok}`** (pessimistic — failed PATCH never clears the dirty marker).
- `toggle_checkbox` (app.rs:629): optimistic local mutate + same PATCH; on error, status + buffer intact.
- Freshness: no per-tick poll for cloud; on cache-hit open, spawn ETag revalidate (`client.note(id, Some(etag))` → 304 short-circuit); if changed and not editing, swap content + "Note updated remotely" (mirrors local reload flow app.rs:590-601).

### [x] M5 — Publish, transfers, create/delete, prompts
- New prompt overlay on `App` (`Prompt { title, input, kind }`; kinds: NewNoteTitle, PushTitle(PathBuf), DownloadFilename{id}, ConfirmDelete{id,title,team_path}) — input handling clones the doc-search prompt pattern.
- Keybinds (all verified unbound in their contexts):

| key | context | action |
|---|---|---|
| `H` / `gh` | browsers / anywhere | toggle local ↔ cloud |
| `n` | cloud browser | new note (title prompt → create → open) |
| `D` | cloud browser/Reader | delete (confirm → delete_note/team variant) |
| `P` | cloud browser/Reader | publish toggle (read_permission guest↔owner; status shows publish_link) |
| `y` | cloud Reader | copy publish_link (reuse existing OSC-52 copy) |
| `o` | cloud Reader (no focused link) | open publish_link in system browser |
| `S` | cloud browser/Reader | download to local file (filename prompt, default slugified title, under `app.root`) |
| `U` | local Reader / browser on `.md` | push file up as new cloud note (title prompt, default file stem) |

- Update `?` help overlay with a HackMD section (notes "not logged in" when applicable).

### [x] M6 — Tests
- Ported in-file tests run via `cargo test --features tui`; add `CloudContext::disconnected()` test constructor (no runtime needed).
- Unit tests for `apply_cloud_msg` (pure sync, no terminal): lists populate, pending nav completes + pushes history, `Saved{Err}` keeps dirty, stale-id responses dropped, pending counter zeroes.
- Wiremock integration `tests/tui_cloud.rs` (pattern from existing `tests/sdk_integration.rs`): real runtime + MockServer, assert PATCH bodies and channel delivery.
- Rewrite `tests/tui_smoke.rs`: `assert_cmd` `md --version/--help` snapshot (locks the CLI surface).

### [x] M7 — Docs, packaging, cleanup
- README rewrite: what it is (SDK + CLI + reader/editor, two bins), install matrix, `md` usage (port md-tui README sections), cloud mode key table + token setup, SDK quick start, feature table, credit "md-tui v0.3.0 merged in".
- CHANGELOG 0.1.0 entry. Update stale `src/lib.rs` tui doc blurb.
- Sanity only: `cargo package --list --features tui`, `cargo publish --dry-run`. **No publish** (route through `/release` later).
- md-tui repo: afterwards, README banner "merged into zemse/hackmd" + archive (out of scope for this change).

## Verification (end-to-end)
- [x] `cargo test` / `cargo test --features tui` / `cargo check --no-default-features` all green.
- [ ] `cargo run --features tui --bin md -- README.md` — reader renders; `md dir/`, stdin pipe, split editor Ctrl-S on a local file. *(headless PTY smoke check passed — binary boots, enters alt screen, draws; full interactive pass needs a human terminal)*
- [ ] With a real token: `H` → cloud list loads; open a note; edit + Ctrl-S; verify on hackmd.io. `P` publish → open publish_link. `S` download, `U` push, `n` create, `D` delete.
- [ ] No token: `H` shows login instruction, app stays usable locally.
- [ ] `cargo run --features tui -- tui` opens cloud view via the hackmd bin.

## Top risks
1. Async bridge races (fetch completes after user navigated) — mitigated by id-matched `pending_nav`, history pushed on completion, deterministic sync unit tests.
2. `block_in_place` panics on current-thread runtimes — safe (main is multi-thread `#[tokio::main]`); `md` bin uses an explicit Runtime anyway.
3. ratatui-image 11 stable vs ratatui 0.30 — checked in M0; fallback pin `=11.0.0-alpha.2`.
4. Keybind collisions — resolved by context scoping against the verified events.rs map.

Per user's global workflow rules: commit after each milestone (format first), and commit the whole plan execution at the end.
