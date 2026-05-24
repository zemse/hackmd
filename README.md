# hackmd

Rust client library and CLI for [HackMD](https://hackmd.io).

Status: early / WIP. Ports the official [`@hackmd/api`](https://github.com/hackmdio/api-client) SDK and [`@hackmd/hackmd-cli`](https://github.com/hackmdio/hackmd-cli) to Rust, with an optional TUI.

## Install (CLI)

```sh
cargo install hackmd
```

Then:

```sh
hackmd --help
hackmd tui      # opens the TUI (when built with the `tui` feature)
```

## Use as a library

The default features include the CLI dependencies. Library users should opt out:

```toml
[dependencies]
hackmd = { version = "0.0", default-features = false }
```

## Features

| feature | default | what it pulls in |
|---|---|---|
| `cli` | yes | CLI binary + its deps |
| `tui` | no  | TUI subcommand (implies `cli`) |

## License

MIT
