# hackmd-rs — Implementation Plan

Port of [`@hackmd/api`](https://github.com/hackmdio/api-client) (SDK) and [`@hackmd/hackmd-cli`](https://github.com/hackmdio/hackmd-cli) (CLI) to Rust as a single crate `hackmd`, plus a TUI mode. Three execution agents (A / B / C) work in parallel through their assigned tracks. Milestones (M1…M6) are explicit sync points where one track unblocks another.

Reference sources are checked out (gitignored) at:
- `_ref/api-client/nodejs/src/{index.ts, type.ts, error.ts}` — SDK
- `_ref/hackmd-cli/src/{commands/, config.ts, command.ts, flags.ts, open-editor.ts, utils.ts}` — CLI

---

## 0. Shared decisions (read first, all agents)

### 0.1 Module layout

```
src/
  lib.rs                 # re-exports the public SDK surface
  client.rs              # Client struct, HTTP plumbing, retry, ETag, rate-limit
  error.rs               # Error enum + From impls
  types/
    mod.rs               # re-exports
    user.rs              # User, SimpleUserProfile, Team
    note.rs              # Note, SingleNote, CreateNoteOptions, UpdateNoteOptions, enums
    folder.rs            # ApiFolder, ApiFolderOrder, Create/Update*FolderBody
  api/
    mod.rs
    notes.rs             # user note endpoints
    team_notes.rs
    folders.rs
    team_folders.rs
    teams.rs
    user.rs              # getMe, getHistory

  # CLI (gated by `cli` feature, but feature body is empty — deps are unconditional;
  # `cli` only gates the binary and the cli module via #[cfg(feature = "cli")])
  main.rs                # bin entrypoint
  cli/
    mod.rs               # clap Cli struct + dispatch
    config.rs            # ~/.hackmd/config.json + env loading
    output.rs            # table/json/csv/yaml rendering
    editor.rs            # $EDITOR temp-file flow
    commands/
      mod.rs
      auth.rs            # login, logout, whoami
      notes.rs           # notes list/get/create/update/delete
      team_notes.rs
      teams.rs
      history.rs
      export.rs
      tui.rs             # tui subcommand entry (gated by `tui` feature)
  tui/                   # gated by `tui` feature
    mod.rs
    app.rs
    views/...
```

### 0.2 Dependency choices (lock these once, no drift)

| purpose | crate | feature flags |
|---|---|---|
| async runtime | `tokio` | `["macros", "rt-multi-thread", "fs", "io-std"]` |
| HTTP | `reqwest` | `["json", "rustls-tls", "stream"]`, `default-features = false` |
| JSON | `serde`, `serde_json` | `serde` with `["derive"]` |
| YAML output | `serde_yaml` | — |
| errors | `thiserror` | — |
| dates | `chrono` | `["serde"]` |
| CLI parsing | `clap` | `["derive", "env"]` |
| config dir | `dirs` | — |
| editor temp file | `tempfile` | — |
| table output | `comfy-table` | — |
| TUI | `ratatui`, `crossterm` | — |
| HTTP mocking (test) | `wiremock` | — |
| CLI testing | `assert_cmd`, `predicates` | — |
| env loading (test) | `dotenvy` | — (dev-deps only) |

Edition: **2024**. MSRV: latest stable.

### 0.3 Naming conventions

- TS `camelCase` JSON fields → Rust `snake_case` structs with `#[serde(rename_all = "camelCase")]` on the struct.
- TS string-literal enums → Rust enums with `#[serde(rename_all = "snake_case")]` (or explicit `#[serde(rename = "...")]` where the JS value isn't snake_case-derivable — e.g. `NotePermissionRole::SignedIn` ↔ `"signed_in"`).
- TS `Date` fields that come over JSON as strings → `String` (the JS lib doesn't actually parse them either; see Surprising #11 in SDK scan).
- TS `ApiFolder.createdAt: number` (ms epoch) → `i64`.
- Nullable fields → `Option<T>` with `#[serde(default)]` so missing fields deserialize cleanly.
- Method names: drop the `get` prefix where Rust idiom prefers it (`me()`, `note(id)`, `notes()`, `create_note(...)`, `update_note(id, ...)`, etc.). Match the npm SDK only where it adds clarity.

### 0.4 Error model

Single `Error` enum in `error.rs` (using `thiserror`):

```rust
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("missing required argument: {0}")]
    MissingArgument(&'static str),
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("server error (HTTP {status})")]
    InternalServer { status: u16 },
    #[error("rate limited (HTTP 429)")]
    RateLimit {
        user_limit: u32,
        user_remaining: u32,
        reset_after: Option<i64>,
    },
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
```

Mapping mirrors SDK: 5xx → `InternalServer`, 429 → `RateLimit` (parse `x-ratelimit-user*` headers), other non-2xx → `Http`.

### 0.5 Client behavior contract (mirror SDK exactly)

- Base URL default: `https://api.hackmd.io/v1`
- Auth: `Authorization: Bearer <token>`
- Timeout default: **30s**
- Retry: enabled by default, `max_retries=3`, `base_delay=100ms`, backoff = `2^attempt * base_delay`
  - Retryable verbs: **GET, HEAD, OPTIONS, PUT, DELETE only** (NOT POST/PATCH — see SDK gotcha #5)
  - Retryable status: 5xx, 429, network error
  - Stop early if `x-ratelimit-userremaining <= 0`
- ETag: `get_note` sends `If-None-Match` when caller passes an etag; accepts 304; returns `Option<SingleNote>` or wrapper that signals 304
- All API calls are async (require tokio runtime)

### 0.6 Test strategy (mirrors upstream)

- **Unit tests** alongside each module via `#[cfg(test)] mod tests` — use `wiremock` for HTTP
- **Integration tests** in `tests/` — also wiremock-driven
- **E2E tests** in `tests/e2e/` — gated by `HACKMD_ACCESS_TOKEN` env var; skip cleanly when absent (no `#[ignore]` — use runtime skip so CI can opt in via env)
- **CLI tests** in `tests/cli_*.rs` — `assert_cmd` + `predicates`, with `tempfile` for config isolation
- Don't ship a `.env` file. Document required env in README.

### 0.7 Cross-cutting rules

- Library has zero `println!`/`eprintln!`. All user-facing output happens in `cli/` only.
- No `unwrap()` / `expect()` in library code paths. Tests are fine.
- Every public type derives `Debug`. Most also derive `Clone`. `Serialize`/`Deserialize` where they cross the wire.
- Hidden invariants and workarounds get a one-line `//` comment. Nothing else.

---

## 1. Milestones (cross-track sync points)

Each milestone is "this deliverable is on disk, committed, tests green." Other tracks can depend on it.

| ID | Owner | Deliverable | Unblocks |
|---|---|---|---|
| **M1** | A | `Client::new` + auth + retry + `me()` working end-to-end with wiremock test | C wires real SDK calls; B starts folder endpoints |
| **M2** | A | All note operations done (user + team CRUD, content + metadata, ETag) | C implements `notes` + `team-notes` commands; B starts folder-order |
| **M3** | B | All folder + folder-order endpoints (user + team) | C (optional folder commands — upstream CLI doesn't expose them but we may) |
| **M4** | C | CLI core: config, output, auth commands, `notes` + `team-notes` commands shipping | TUI work, e2e CLI tests |
| **M5** | B | E2E test harness + SDK integration tests green | release confidence |
| **M6** | C | TUI feature behind `--features tui` shipping | v0.1.0 release |

Agents should not block on milestones they don't actually need. Most work is parallelizable.

---

## 2. Track A — SDK core (foundations + notes + teams)

Goal: get the lib to "you can publish notes from Rust today."

### A.1 Bootstrap deps
- Add to `Cargo.toml`: `tokio`, `reqwest`, `serde`, `serde_json`, `thiserror`, `chrono` (per 0.2)
- Add dev-deps: `wiremock`, `tokio` with `["macros", "rt-multi-thread"]`
- Verify `cargo build` and `cargo test` still pass

### A.2 Type module — `src/types/`
- Port enums from `_ref/api-client/nodejs/src/type.ts:1-22`:
  - `TeamVisibilityType`, `NotePublishType`, `CommentPermissionType`, `NotePermissionRole`
  - Use `#[serde(rename = "...")]` per the literal string values (snake_case where direct, explicit otherwise)
- Port `User`, `Team`, `SimpleUserProfile` (`type.ts:24-66`)
- Port `Note`, `SingleNote`, `FolderPath` (`type.ts:69-100`)
- Port `CreateNoteOptions`, `UpdateNoteOptions` (`type.ts:102-122`)
- Add `#[derive(Debug, Clone, Serialize, Deserialize)]` + `#[serde(rename_all = "camelCase")]` on each struct
- Use `Option<T>` + `#[serde(skip_serializing_if = "Option::is_none")]` on payload types so PATCH bodies don't send `null`s
- **Tests:** round-trip serde tests for each struct against a JSON fixture taken from `_ref/api-client/nodejs/tests/` fixtures

### A.3 Error module — `src/error.rs`
- Implement the `Error` enum per 0.4
- Helper: `fn from_response(status: u16, headers: &HeaderMap, body: &str) -> Error` that does the 5xx/429/other mapping

### A.4 Client foundation — `src/client.rs`  → **M1**
- `pub struct Client { http: reqwest::Client, base_url: String, token: String, retry: RetryConfig }`
- `Client::new(token: impl Into<String>) -> Self`
- `Client::with_endpoint(token, endpoint) -> Self`
- `Client::with_config(token, endpoint, ClientConfig) -> Self` where `ClientConfig { timeout, retry }`
- Internal: `request<T: DeserializeOwned>(method, path, body, etag) -> Result<Response<T>>` that handles:
  - Adds `Authorization` header
  - Retry loop with exponential backoff per 0.5 (only for safe verbs)
  - Parses 429 rate-limit headers into `Error::RateLimit`
  - Wraps non-2xx into appropriate `Error` variant
  - Honors `If-None-Match` and returns `Response::NotModified(etag)` for 304
- Implement `me()` → `GET /me`
- **Tests** (wiremock):
  - happy path: 200 returns user
  - 429 with rate-limit headers → `Error::RateLimit` with parsed fields
  - 500 retried 3x then fails
  - 500 on POST (use a stub future endpoint) NOT retried — verify request count == 1
  - Network error retried
  - `x-ratelimit-userremaining: 0` halts retry early
- **Commit at M1.**

### A.5 User & history endpoints — `src/api/user.rs`
- `me()` (already done at M1; move into module)
- `history()` → `GET /history` → `Vec<Note>`
- Tests: happy path each

### A.6 User notes — `src/api/notes.rs`  → contributes to **M2**
Port from `_ref/api-client/nodejs/src/index.ts:181-214`:
- `notes()` → `GET /notes`
- `note(note_id, etag: Option<&str>)` → `GET /notes/{id}` (ETag-aware; returns enum `NoteResult::Note(SingleNote, etag) | NoteResult::NotModified`)
- `create_note(opts: CreateNoteOptions)` → `POST /notes`
- `update_note_content(note_id, content: Option<String>)` → `PATCH /notes/{id}` with body `{content}`
- `update_note(note_id, opts: UpdateNoteOptions)` → `PATCH /notes/{id}`
- `delete_note(note_id)` → `DELETE /notes/{id}`
- Tests: wiremock for each; ETag roundtrip (200 returns etag, 304 returns NotModified)

### A.7 Teams + team notes — `src/api/teams.rs`, `src/api/team_notes.rs`  → completes **M2**
Port from `_ref/api-client/nodejs/src/index.ts:216-234`:
- `teams()` → `GET /teams`
- `team_notes(team_path)` → `GET /teams/{teamPath}/notes`
- `create_team_note(team_path, opts)` → `POST /teams/{teamPath}/notes`
- `update_team_note_content(team_path, note_id, content)` → `PATCH /teams/{teamPath}/notes/{noteId}`
- `update_team_note(team_path, note_id, opts)` → `PATCH /teams/{teamPath}/notes/{noteId}`
- `delete_team_note(team_path, note_id)` → `DELETE /teams/{teamPath}/notes/{noteId}`
- **Note**: upstream is inconsistent here — team update methods return raw response. In Rust we make them consistent (return same `SingleNote` as user methods).
- Tests: same shape as A.6

### A.8 Re-exports — `src/lib.rs`
- `pub use client::Client;`
- `pub use error::{Error, Result};`
- `pub use types::*;`
- Module-level doc comment with quick-start example
- **Commit M2.**

---

## 3. Track B — Folders, polish, integration tests, docs

Goal: feature parity with `@hackmd/api` v2.5+ and a release-ready test surface.

### B.1 Wait for **M1**.
While waiting: draft folder types in a scratch branch / local file so they're ready to land the moment client exists. Read SDK scan section "Folder/team endpoints" and `_ref/api-client/nodejs/src/type.ts:124-178`.

### B.2 Folder types — extend `src/types/folder.rs`
- `ApiFolder` (note: `createdAt`/`updatedAt` are **ms epoch i64**, not strings)
- `ApiFolderOrder` = `HashMap<String, Vec<String>>` (key `"root"` for root-level)
- `CreateUserFolderBody`, `UpdateUserFolderBody` (with `Option<Option<String>>` or `serde_with::OptionExt`-style handling for `null`-vs-absent on update)
  - Update bodies need to distinguish "don't change" (absent) from "set to null" (explicit null). Simplest: use `serde_with::rust::double_option` or hand-roll. Document the chosen approach in module docs.
- Re-export from `types/mod.rs`

### B.3 User folders — `src/api/folders.rs`  → contributes to **M3**
Port from `_ref/api-client/nodejs/src/index.ts:236-263`:
- `folders()` → `GET /folders`
- `create_folder(body)` → `POST /folders`
- `folder(id)` → `GET /folders/{id}`
- `update_folder(id, body)` → `PATCH /folders/{id}`
- `delete_folder(id)` → `DELETE /folders/{id}`
- `folder_order()` → `GET /folders/folder-order`
- `update_folder_order(body)` → `PUT /folders/folder-order`
- Tests: wiremock + 404 graceful handling (some hosts don't support folders — `Result<_, Error>` is fine; CLI surfaces it nicely)

### B.4 Team folders — `src/api/team_folders.rs`  → completes **M3**
Same surface as B.3 but team-scoped. Port from `_ref/api-client/nodejs/src/index.ts:265-291`.

### B.5 Coverage-gap tests
From the tests-scan "Coverage gaps" list:
- Concurrent retries (spawn 5 tokio tasks against a rate-limited mock; verify behavior)
- Rate-limit header parsing edge cases: missing headers, non-numeric values, negative reset
- ETag 304 returns zero-body
- Exponential backoff formula verification (use `wiremock` delays + measure elapsed)
- Error responses with non-JSON body (HTML, plaintext) — graceful surfacing

### B.6 Integration tests — `tests/sdk_integration.rs`
- Cross-module flows: create note → update → fetch with etag → 304 → delete
- All wiremock-driven (no live API in CI)

### B.7 E2E harness — `tests/e2e/sdk_e2e.rs`  → contributes to **M5**
- Read `HACKMD_ACCESS_TOKEN` from env via `dotenvy` (dev-dep only)
- If absent, skip with a clear `eprintln!("e2e: HACKMD_ACCESS_TOKEN not set, skipping")`
- Coverage: `me`, `notes` list, `teams`, `history`, optional mutations gated by `HACKMD_E2E_MUTATIONS=1` env var (same convention as upstream)
- Folder tests gated by `HACKMD_E2E_FOLDERS=1` (default off, since not all hosts support)

### B.8 SDK rustdoc pass  → completes **M5**
- Module-level docs for `client`, `types::note`, `types::folder`
- Quick-start example in `lib.rs` doc comment that compiles (`cargo test --doc`)
- Document the `cli`/`tui` features in lib.rs so docs.rs shows the feature matrix

---

## 4. Track C — CLI + TUI

Goal: `cargo install hackmd` gives a working CLI that mirrors `@hackmd/hackmd-cli` command surface, plus `hackmd tui`.

### C.1 Clap scaffold — `src/cli/mod.rs`, `src/main.rs`
Can start before M1 — stub SDK calls with `todo!()` so the command tree compiles.
- Add deps: `clap`, `dirs`, `tempfile`, `comfy-table`, `serde_yaml`
- Top-level `Cli` enum with subcommands matching upstream:
  - `Login`, `Logout`, `Whoami`, `History`, `Export`
  - `Notes(NotesCmd)` with sub: `List | Get | Create | Update | Delete`
  - `TeamNotes(TeamNotesCmd)` with sub: `List | Create | Update | Delete`
  - `Teams`
  - `Tui` (gated behind `#[cfg(feature = "tui")]`)
- Global flags: `--config-dir`, `--endpoint`, `--token` (all also overridable by env)
- Wire `clap(env = "HMD_API_ACCESS_TOKEN")` and `clap(env = "HMD_API_ENDPOINT_URL")` for the relevant flags

### C.2 Config — `src/cli/config.rs`
Port from `_ref/hackmd-cli/src/config.ts`:
- Config path: `$HMD_CLI_CONFIG_DIR/config.json` if set, else `~/.hackmd/config.json` (use `dirs::home_dir`)
- Schema: `{ "hackmdAPIEndpointURL": string, "accessToken": string }`
- Auto-create with `{}` if missing
- Env precedence: env > file > default endpoint
- Tests with `tempfile::TempDir` — happy path, missing file (auto-create), invalid JSON, env override

### C.3 Output formatting — `src/cli/output.rs`
Port `ux.table` semantics from `_ref/hackmd-cli/src/commands/whoami.ts:25-37` etc.:
- `print_table<T>(rows: &[T], columns: &[(&str, fn(&T) -> String)], opts: OutputOpts)`
- `OutputOpts { format: Format, columns: Option<Vec<String>>, sort: Option<String>, filter: Option<String>, no_header: bool, no_truncate: bool }`
- `enum Format { Table, Json, Csv, Yaml }` (CSV/YAML via `serde_json::to_value` → reshape → write)
- Reuse a single set of `clap` "output flags" struct via `#[clap(flatten)]` on each list command

### C.4 Editor flow — `src/cli/editor.rs`
Port from `_ref/hackmd-cli/src/open-editor.ts` + `utils.ts:40-45`:
- `open_in_editor() -> Result<String>`
  - `tempfile::Builder::new().suffix(".md").tempfile()`
  - Resolve editor: `$VISUAL` → `$EDITOR` → platform default (`vim` / `notepad`)
  - `std::process::Command::new(editor).arg(path).status()?`
  - Read file contents, return as `String`
- Make sure `tempfile` is auto-cleaned (upstream doesn't clean up — we do)

### C.5 Auth commands — `src/cli/commands/auth.rs`  → needs **M1**
- `login`: interactive prompt for token (use `rpassword` for hidden input — add as dep), validate via `client.me()`, write to config
- `logout`: clear `accessToken` in config (don't delete file)
- `whoami`: print user via table

### C.6 Notes commands — `src/cli/commands/notes.rs`  → needs **M2**
Port from `_ref/hackmd-cli/src/commands/notes/`:
- `notes list` (no flags) → table
- `notes get --note-id <id>` → table (single row)
- `notes create [--title --content --read-permission --write-permission --comment-permission -e/--editor]`
  - Content precedence: `--editor` > stdin (if piped) > `--content` flag
  - Stdin: read with `if !io::stdin().is_terminal() { read all }` (Rust 1.70+ has `IsTerminal`)
- `notes update --note-id <id> --content <c>` (silent on success)
- `notes delete --note-id <id>` (silent on success)

### C.7 Team notes — `src/cli/commands/team_notes.rs`  → needs **M2**
Same as C.6 but team-scoped. Required `--team-path`.

### C.8 History / export / teams — `src/cli/commands/{history,export,teams}.rs`  → needs **M2**
- `history`: table from `client.history()`
- `export --note-id <id>`: print `client.note(id).await?.content` raw to stdout
- `teams`: table from `client.teams()`

### C.9 CLI integration tests — `tests/cli_integration.rs`  → **M4**
- `assert_cmd::Command::cargo_bin("hackmd")`
- Tests: `--version`, `--help`, each subcommand `--help`, unknown command, missing required flags
- For commands that hit the network: spin up `wiremock::MockServer`, set `--endpoint <mock>` and `--token test`, assert outputs

### C.10 README + examples
Update `README.md` with:
- Full CLI usage table (mirror `_ref/hackmd-cli/README.md` structure)
- Quick-start library snippet (works with `default-features = false`)
- Env var documentation
- Feature flag matrix
- **Commit M4.**

### C.11 TUI — `src/tui/`  → needs **M4**, drives **M6**
- Behind `tui` feature flag (which already exists in Cargo.toml; just wire the module)
- `hackmd tui` subcommand only available when compiled with `--features tui`
- Skeleton with `ratatui` + `crossterm`:
  - Left pane: note list (from `client.notes()`)
  - Right pane: selected note content (from `client.note(id)`)
  - Keybindings: `j/k` navigate, `Enter` open, `e` edit (drop into `$EDITOR`, write back via `update_note_content`), `q` quit
- This is the smallest useful TUI; expand in follow-ups
- Test: at minimum, a smoke test that the TUI initializes and tears down without panicking (no full UI test)
- **Commit M6.**

---

## 5. Suggested execution order per agent

```
Agent A:  A.1 → A.2 → A.3 → A.4 [M1] → A.5 → A.6 → A.7 → A.8 [M2]
Agent B:  (wait M1) → B.1 → B.2 → B.3 → B.4 [M3] → B.5 → B.6 → B.7 [M5] → B.8
Agent C:  C.1 → C.2 → C.3 → C.4 → (wait M1) → C.5 → (wait M2) → C.6 → C.7 → C.8 → C.9 [M4] → C.10 → C.11 [M6]
```

Agents commit frequently. Each commit message references the section ID (e.g. `A.4: client retry loop + me() + tests`). Reviewers can map back to this plan.

---

## 6. Out of scope (for v0.1.0)

- Image upload (`POST /notes/{id}/images`) — upstream SDK exposes it but rarely used; add in v0.2.
- OAuth flow — HackMD doesn't currently offer third-party OAuth, only personal access tokens. Not blocking.
- Sync/realtime CRDT layer — HackMD's edit channel is socket.io-based; out of scope for an HTTP client.
- Webhook receivers — no public webhook API yet.
- CodiMD / HedgeDoc compatibility — upstream v2 dropped CodiMD support; we follow.

---

## 7. Definition of done (v0.0.2 publish)

- All M1–M6 milestones hit
- `cargo build --no-default-features` clean
- `cargo build` clean
- `cargo build --features tui` clean
- `cargo test` green (excludes e2e)
- `cargo test --features tui` green
- `cargo clippy --all-targets --all-features -- -D warnings` clean
- `cargo fmt --check` clean
- `cargo doc --no-deps --all-features` produces docs.rs-ready output
- README updated; PLAN.md archived to `docs/PLAN-v0.1.md` (or deleted)
- E2E test suite passes against a live token (manual run, not CI)
- Bump version to `0.0.2`, run `cargo package --list` to verify clean contents, then `cargo publish`
