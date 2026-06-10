# hackmd

[![crates.io](https://img.shields.io/crates/v/hackmd.svg)](https://crates.io/crates/hackmd)
[![docs.rs](https://img.shields.io/docsrs/hackmd)](https://docs.rs/hackmd)
[![license](https://img.shields.io/crates/l/hackmd.svg)](#license)

Rust SDK, CLI, and terminal markdown reader/editor for [HackMD](https://hackmd.io).

- **SDK** — async `Client` with the full HackMD v1 surface: user, notes, teams, team-notes, folders, folder-order, ETag-aware GETs, retries with exponential backoff, and `429` rate-limit parsing. Ports the official [`@hackmd/api`](https://github.com/hackmdio/api-client) SDK.
- **CLI** — `hackmd` binary with parity to [`@hackmd/hackmd-cli`](https://github.com/hackmdio/hackmd-cli): `login`/`logout`/`whoami`, `history`, `export`, `teams`, `notes` and `team-notes` CRUD, shared `--output table|json|csv|yaml` formatting.
- **TUI** *(opt-in)* — `hackmd tui`, a full terminal markdown reader/browser/editor (mouse support, clickable links, images, split live-preview editing), with your HackMD notes one keypress away: browse, read, edit, publish, push and download — all without leaving the terminal. [md-tui](https://github.com/zemse/md-tui) v0.3.0 merged in.

## Install

```sh
cargo install hackmd                  # SDK + CLI
cargo install hackmd --features tui   # also enables `hackmd tui`
```

The `cli` feature is on by default; library-only consumers should opt out (see [Library](#library)).

## `hackmd tui` — terminal markdown reader/editor

```sh
hackmd tui            # cloud view when logged in, else browses the cwd
```

Logged in, it opens straight onto your hackmd.io notes; the local file browser is one `H` (or `Esc`) away — and without a token it simply starts there.

Reader: vim-style scrolling (`j/k`, `d/u`, `gg/G`, counts), in-document search (`/`, `n/N`), fuzzy file search (`T`), Tab-cycle across links and checkboxes, click-to-follow links, click-to-toggle checkboxes, inline images (kitty/iTerm2/sixel terminals), tables with click-to-expand, JSON-line pretty-print, git lens (`Ctrl-G`, diff vs HEAD), read/unread badges in the browser.

Editor (`e`): HackMD-style split view — raw markdown on one side, live preview on the other, scroll-synced both ways. `Ctrl-S` saves, `Ctrl-Z`/`Ctrl-Y` undo/redo, `Esc Esc` discards. Checkbox toggles persist straight to disk from read mode.

Config lives at `~/.config/md/config.toml` (`theme`, `width`, `line_numbers`) — unchanged from md-tui, an existing file keeps working. `?` shows the full key map.

> The standalone `md` binary from md-tui isn't shipped right now — the whole reader/editor lives behind `hackmd tui`. It may return as a second binary later.

### HackMD cloud mode

Log in once (`hackmd login`, or set `HMD_API_ACCESS_TOKEN`), then press `H` in any browser view — or `gh` from anywhere — to flip between your local files and your hackmd.io notes. Without a token everything local keeps working; cloud mode just tells you how to log in.

| key | context | action |
|---|---|---|
| `H` / `gh` | browsers / anywhere | toggle local ↔ hackmd.io |
| `Enter` | cloud browser | open note |
| `R` | cloud browser | refresh note lists |
| `e` + `Ctrl-S` | cloud note | edit, save back to the cloud (PATCH) |
| `n` | cloud browser | new note (title prompt) |
| `D` | cloud browser / note | delete (asks for confirmation) |
| `P` | cloud browser / note | publish / unpublish |
| `y` | cloud note | copy the publish link |
| `o` | cloud note | open the publish link in your browser |
| `S` | cloud browser / note | download note to a local file |
| `U` | local file / browser | push a local file up as a new note |

The cloud list shows your notes plus each team's, with a `[pub]` badge on published ones. Fetched notes are cached and revalidated by ETag, so reopening is instant and remote edits show up on their own. Saves are pessimistic — the dirty marker only clears once the server confirms. All network traffic runs on background tasks; the UI never blocks.

Publishing flips the note's `readPermission` between `guest` and `owner` — that's how the HackMD v1 API models it (there is no separate publish endpoint, and no comments API).

## `hackmd` — CLI

### Authentication

Run `hackmd login` — it opens <https://hackmd.io/settings#api> (where access tokens are created) in your browser and prompts for the token:

```sh
$ hackmd login
Create an access token at: https://hackmd.io/settings#api
(opened in your browser)
Enter your HackMD access token: ********
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

hackmd notes list                                     # list your notes
hackmd notes get --note-id <id>                       # fetch a single note
hackmd notes create [--title <t>] [--content <c>] \
                    [--read-permission <r>] \
                    [--write-permission <w>] \
                    [--comment-permission <c>] \
                    [-e | --editor]                   # create a note (stdin or $EDITOR also accepted)
hackmd notes update --note-id <id> --content <c>      # replace a note's content
hackmd notes delete --note-id <id>                    # delete a note

hackmd team-notes --team-path <p> list
hackmd team-notes --team-path <p> create  ...         # same flags as `notes create`
hackmd team-notes --team-path <p> update --note-id <id> --content <c>
hackmd team-notes --team-path <p> delete --note-id <id>

hackmd tui                                            # the TUI, opened in cloud view (requires --features tui)
```

Every list-style command accepts shared output flags:

| flag | meaning |
|---|---|
| `--output {table\|json\|csv\|yaml}` | output format (default `table`) |
| `--columns id,title,...` | project a subset of columns |
| `--sort <column>` | sort rows by a column (string compare) |
| `--filter key=value` | keep rows where `row[key] == value` |
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
| `tui` | no  | the `hackmd tui` terminal UI (implies `cli`) — `ratatui`, `syntect`, image decoding, … |

`cargo build --no-default-features` skips the binary (gated by `required-features = ["cli"]`) and compiles the SDK alone.

## Credits

The TUI is [md-tui](https://github.com/zemse/md-tui) v0.3.0, merged into this crate and extended with HackMD cloud mode. The SDK and CLI port the official HackMD [`api-client`](https://github.com/hackmdio/api-client) and [`hackmd-cli`](https://github.com/hackmdio/hackmd-cli).

## License

MIT
