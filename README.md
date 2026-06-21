# hackmd

[![crates.io](https://img.shields.io/crates/v/hackmd.svg)](https://crates.io/crates/hackmd)
[![docs.rs](https://img.shields.io/docsrs/hackmd)](https://docs.rs/hackmd)
[![license](https://img.shields.io/crates/l/hackmd.svg)](#license)

Rust SDK, CLI, and terminal markdown reader/editor for [HackMD](https://hackmd.io).

- **SDK** — async `Client` with the full HackMD v1 surface: user, notes, teams, team-notes, folders, folder-order, ETag-aware GETs, retries with exponential backoff, and `429` rate-limit parsing. Ports the official [`@hackmd/api`](https://github.com/hackmdio/api-client) SDK.
- **CLI** — `hackmd` binary with parity to [`@hackmd/hackmd-cli`](https://github.com/hackmdio/hackmd-cli): `login`/`logout`/`whoami`, `history`, `export`, `teams`, `notes` and `team-notes` CRUD, shared `--output table|json|csv|yaml` formatting.
- **TUI** — `hackmd`, a full terminal markdown reader/browser/editor (mouse support, clickable links, images, split live-preview editing), with your HackMD notes one keypress away: browse, read, edit, publish, push and download — all without leaving the terminal. [md-tui](https://github.com/zemse/md-tui) v0.3.0 merged in.

## Install

```sh
cargo install hackmd                            # SDK + CLI + TUI
cargo install hackmd --no-default-features      # library-only (SDK)
```

The `cli` and `tui` features are on by default; library-only consumers should opt out (see [Library](#library)).

## TUI — terminal markdown reader/editor

```sh
hackmd                # cloud notes view when logged in, else browses the cwd
hackmd .              # browse the current directory
hackmd notes.md       # open a file (or any dir/file path) in the reader
hackmd new meeting notes   # create a cloud note titled "meeting notes" and edit it
hackmd tui            # same as bare `hackmd`
```

Logged in, bare `hackmd` opens straight onto your hackmd.io notes; the local file browser is one `H` (or `Esc`) away — and without a token it simply starts there. With a path argument it stays local: a directory opens the file browser, a markdown file opens the reader.

Reader: vim/less-style scrolling (`j/k`, `d/u`, `f/b`, `gg/G`, counts), heading jumps (`]]` / `[[`), table of contents (`t`), in-document search (`/`, `n/N`), fuzzy file search (`T`), history (`h/l`, `Ctrl-O`), Tab-cycle across links and checkboxes, click-to-follow links, click-to-toggle checkboxes, inline images (kitty/iTerm2/sixel terminals), tables with click-to-expand, JSON-line pretty-print, git lens (`Ctrl-G`, diff vs HEAD), read/unread badges in the browser.

Editor (`e`): HackMD-style split view — raw markdown on one side, live preview on the other, scroll-synced both ways. `Ctrl-S` saves, `Ctrl-Z`/`Ctrl-Y` undo/redo, `Esc Esc` discards. Checkbox toggles persist straight to disk from read mode.

Config lives at `~/.config/md/config.toml` (`theme`, `width`, `line_numbers`) — unchanged from md-tui, an existing file keeps working. `?` shows the full key map.

> The standalone `md` binary from md-tui isn't shipped right now — the whole reader/editor lives behind `hackmd tui`. It may return as a second binary later.

### HackMD cloud mode

Log in once (`hackmd login`, or set `HMD_API_ACCESS_TOKEN`), then press `H` in any browser view — or `gh` from anywhere — to flip between your local files and your hackmd.io notes. Without a token everything local keeps working; cloud mode just tells you how to log in.

| key | context | action |
|---|---|---|
| `H` / `gh` | browsers / anywhere | toggle local ↔ hackmd.io |
| `Enter` | cloud browser | open note |
| `Tab` / `S-Tab` | cloud browser | switch workspace tab (you / teams) |
| `R` | cloud browser | refresh note lists |
| `e` + `Ctrl-S` | cloud note | edit, save back to the cloud (PATCH) |
| `n` | cloud browser | new note (title prompt) |
| `D` | cloud browser / note | delete (asks for confirmation) |
| `P` | cloud browser / note | publish / unpublish |
| `y` | cloud note | copy the publish link |
| `o` | cloud note | open the publish link in your browser |
| `S` | cloud browser / note | download note to a local file |
| `U` | local file / browser | publish a local file — links it, then keeps it in sync |
| `A` / `O` | reader | open the editor at end-of-doc / a new top line |

The cloud browser shows one workspace tab per owner — your notes first, then each team (the tab bar only appears when there's more than one). Every note carries a visibility badge: `[published]` (on the owner's public profile, indexable), `[anyone with link]`, `[signed in]`, `[only team]`, or `[only me]`. Fetched notes are cached and revalidated by ETag, so reopening is instant and remote edits show up on their own. Saves are pessimistic — the dirty marker only clears once the server confirms. All network traffic runs on background tasks; the UI never blocks.

Publishing flips the note's `readPermission` between `guest` and `owner` — that's how the HackMD v1 API models it (there is no separate publish endpoint).

**Local ↔ HackMD sync.** Pressing `U` on a local file the first time creates the note and stamps a managed `<!-- hackmd … -->` link block at the **bottom** of the file (note id, URL, publish link, last-synced time — don't hand-edit it). The new note also gets a one-time attribution footer in its body (the same one `hackmd new` adds); after that it's plain content you can edit or remove. From then on the file and its note stay in sync automatically: on open, on save, and on a background poll. Changes from both sides are three-way merged against a cached base under `<root>/.hackmd/`; when local and upstream both edit the same lines, a full-screen resolver lets you pick a side per hunk (`l`/`u`/`b`/`n`, then `Enter`). The note title is taken from the file's first `# H1` (you're prompted only when there isn't one). The link block is stripped before pushing, so the hackmd.io copy stays clean.

**Comments.** The HackMD v1 REST API exposes no comment or reaction endpoints, so comments on a note are **not fetchable** through this tool (or any v1 API client). See [hackmdio/hackmd-io-issues#428](https://github.com/hackmdio/hackmd-io-issues/issues/428).

## `hackmd` — CLI

### Authentication

Run `hackmd login` — it opens <https://hackmd.io/settings#api> (where access tokens are created) in your browser and prompts for the token:

```sh
$ hackmd login
Create an access token at: https://hackmd.io/settings#api
Enter your HackMD access token: ****************************************
Login successfully
```

The token is written to `~/.hackmd/config.json`. To log out:

```sh
$ hackmd logout
You've logged out successfully
```

### Commands

```
hackmd login                                          # interactive token prompt + validate
hackmd logout                                         # clear the stored token
hackmd whoami                                         # show the authenticated user
hackmd history                                        # list browse history
hackmd export --note-id <id>                          # dump raw markdown to stdout
hackmd teams                                          # list teams
hackmd version                                        # print version (also -v / --version)

hackmd notes [--note-id <id>]                         # bare = list your notes; with an id = fetch one
hackmd notes list                                     # list your notes
hackmd notes get --note-id <id>                       # fetch a single note
hackmd notes create [--title <t>] [--content <c>] \
                    [--tags t1,t2] \
                    [--read-permission <r>] \
                    [--write-permission <w>] \
                    [--comment-permission <c>] \
                    [--parent-folder-id <id>] \
                    [-e | --editor]                   # create a note (stdin or $EDITOR also accepted)
hackmd notes update --note-id <id> [--content <c>] \
                    [--tags t1,t2] [--permalink <p>] \
                    [--read-permission <r>] \
                    [--write-permission <w>] \
                    [--parent-folder-id <id>]         # update content and/or metadata
hackmd notes delete --note-id <id>                    # delete a note

hackmd team-notes --team-path <p>                     # bare = list the team's notes
hackmd team-notes --team-path <p> list
hackmd team-notes --team-path <p> create  ...         # same flags as `notes create`
hackmd team-notes --team-path <p> update --note-id <id> ...  # same flags as `notes update`
hackmd team-notes --team-path <p> delete --note-id <id>

hackmd folders [--folder-id <id>]                     # bare = list folders; with an id = fetch one
hackmd folders create [--name <n>] [--description <d>] \
                      [--icon <i>] [--color <c>] \
                      [--parent-folder-id <id>]
hackmd folders update --folder-id <id> ...            # same metadata flags as create
hackmd folders delete --folder-id <id>
hackmd folders order [--order '{"root":["id",...]}']  # bare = print order JSON; with --order = replace it

hackmd team-folders --team-path <p> ...               # same surface as `folders`, team-scoped

hackmd new [TITLE...]                                 # create a note and open it in the TUI editor
                                                      # (alias: hackmd create; quotes around the title optional)
hackmd [PATH]                                         # the TUI: cloud view bare, local browser/reader with a path
```

### Drop-in compatibility with `@hackmd/hackmd-cli`

Command lines written for the original [hackmd-cli](https://github.com/hackmdio/hackmd-cli) work unchanged — alias `hackmd-cli=hackmd` and existing scripts keep running:

- every camelCase flag is accepted (`--noteId`, `--teamPath`, `--readPermission`, `--writePermission`, `--commentPermission`, `--parentFolderId`, `--folderId`), with the kebab-case spellings as this crate's preferred forms;
- bare `notes` / `team-notes` / `folders` / `team-folders` list, and `--noteId` / `--folderId` without a subcommand fetches a single record, exactly like upstream;
- the oclif table flags work: `--csv`, `-x`/`--extended`, `--no-header`, `--no-truncate`, `--filter key=value` (partial match), `--sort -column` (descending), and `--columns` matches header-style names case-insensitively (`--columns=ID,Title`);
- **the config file is shared**: both CLIs read and write `~/.hackmd/config.json` with the same `accessToken` / `hackmdAPIEndpointURL` keys (and the same `HMD_*` env vars), so logging in with either CLI logs in the other, and unknown keys in the file are preserved on write.

Not implemented: `autocomplete` (oclif-specific shell completion).

Every list-style command accepts shared output flags:

| flag | meaning |
|---|---|
| `--output {table\|json\|csv\|tsv\|yaml}` | output format — defaults to `table` on a terminal and `tsv` when stdout is piped, so scripts and LLM agents get parseable rows without any flag |
| `--csv` | shorthand for `--output csv` |
| `--columns id,title,...` | project a subset of columns (names match case-insensitively, spaces ignored) |
| `-x`, `--extended` | show every column of the records, not just the defaults |
| `--sort <column>` | sort rows by a column (string compare; `-column` for descending) |
| `--filter key=value` | keep rows where `row[key]` contains `value` |
| `--no-header` | omit the header row |
| `--no-truncate` | don't shrink wide cells to fit the terminal |

Permission values: `owner`, `signed_in`, `guest`.
Comment permission values: `disabled`, `forbidden`, `owners`, `signed_in_users`, `everyone`.

### Content precedence on `notes create` / `team-notes create`

1. `--editor` opens `$EDITOR` on a temp `.md` file and uses the saved contents.
2. If stdin is piped (`cat note.md | hackmd notes create`), the piped bytes are used.
3. Otherwise the `--content` flag value is used.
4. If none of those is set, the note is created with empty content (matches upstream).

### Environment variables

| variable | maps to | notes |
|---|---|---|
| `HMD_API_ACCESS_TOKEN` | the access token | takes precedence over the file config |
| `HMD_API_ENDPOINT_URL` | the API base URL | defaults to `https://api.hackmd.io/v1` |
| `HMD_CLI_CONFIG_DIR` | config directory | defaults to `~/.hackmd` |

Each of these has a matching global CLI flag (`--token`, `--endpoint`, `--config-dir`) that takes priority over the env value when set. The TUI resolves its token through the same chain.

### Examples

```sh
# Show notes as JSON
hackmd notes list --output json

# Filter then sort
hackmd notes list --filter teamPath=demo --sort title

# Create a note from stdin
cat draft.md | hackmd notes create --title "Draft"

# Create a team note via $EDITOR
hackmd team-notes --team-path my-team create --editor --title "RFC"
```

## Library

```toml
[dependencies]
hackmd = { version = "0.1", default-features = false }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

`default-features = false` drops the CLI/TUI deps (`clap`, `comfy-table`, `ratatui`, …) and leaves you with a lean async SDK.

```rust,no_run
# async fn run() -> hackmd::Result<()> {
let client = hackmd::Client::new(std::env::var("HMD_API_ACCESS_TOKEN").unwrap())?;
let me = client.me().await?;
println!("hello {}", me.name);

let notes = client.notes().await?;
for n in &notes {
    println!("{}\t{}", n.id, n.title);
}
# Ok(()) }
```

Every endpoint hangs off [`Client`](https://docs.rs/hackmd/latest/hackmd/client/struct.Client.html); tunable retry/timeout behavior lives on [`ClientConfig`](https://docs.rs/hackmd/latest/hackmd/client/struct.ClientConfig.html) and [`RetryConfig`](https://docs.rs/hackmd/latest/hackmd/client/struct.RetryConfig.html). `GET` for a single note returns a [`CachedResponse`](https://docs.rs/hackmd/latest/hackmd/client/enum.CachedResponse.html) so you can plug in an `ETag` cache.

## Features

| feature | default | what it pulls in |
|---|---|---|
| `cli` | yes | the `hackmd` binary and its dependencies |
| `tui` | yes | the terminal UI (implies `cli`) — `ratatui`, `syntect`, image decoding, … |

`cargo build --no-default-features` skips the binary (gated by `required-features = ["cli"]`) and compiles the SDK alone.

## Credits

The TUI is [md-tui](https://github.com/zemse/md-tui) v0.3.0, merged into this crate and extended with HackMD cloud mode. The SDK and CLI port the official HackMD [`api-client`](https://github.com/hackmdio/api-client) and [`hackmd-cli`](https://github.com/hackmdio/hackmd-cli).

## License

MIT
