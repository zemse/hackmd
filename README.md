# hackmd

Rust client library and CLI for [HackMD](https://hackmd.io).

Ports the official [`@hackmd/api`](https://github.com/hackmdio/api-client) SDK and [`@hackmd/hackmd-cli`](https://github.com/hackmdio/hackmd-cli) to Rust, with an optional TUI mode planned in a follow-up release.

## Install

```sh
cargo install hackmd
```

This installs the `hackmd` binary. The `cli` feature is on by default; library-only consumers should opt out (see below).

## CLI

### Authentication

Create a HackMD access token at <https://hackmd.io/settings#api>, then log in:

```sh
$ hackmd login
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

hackmd tui                                            # TUI placeholder (coming in v0.0.3)
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

Each of these has a matching global CLI flag (`--token`, `--endpoint`, `--config-dir`) that takes priority over the env value when set.

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
hackmd = { version = "0.0", default-features = false }
```

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

The full SDK surface (user notes, team notes, folders + folder-order, ETag-aware GET, retries, rate-limit parsing) lives on [`Client`](https://docs.rs/hackmd/latest/hackmd/client/struct.Client.html). Requires a `tokio` runtime.

## Features

| feature | default | what it pulls in |
|---|---|---|
| `cli` | yes | CLI binary and its dependencies |
| `tui` | no  | TUI subcommand (implies `cli`) — coming in v0.0.3 |

The `cli` feature gates compilation of `src/cli/` and the `hackmd` binary. The binary's `required-features = ["cli"]` means `cargo build --no-default-features` simply skips the binary build.

## License

MIT
