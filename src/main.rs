//! `hackmd` CLI binary entrypoint.
//!
//! The binary itself is gated via `required-features = ["cli"]` in
//! `Cargo.toml`, so building with `--no-default-features` simply skips it.
//! The `#[cfg(feature = "cli")]` shim below keeps a stub `main` in place for
//! IDE/check-without-features compatibility.

#![cfg_attr(not(feature = "cli"), allow(unused))]

#[cfg(feature = "cli")]
#[tokio::main]
async fn main() {
    use clap::Parser;
    use hackmd::cli::{Cli, dispatch};

    let cli = Cli::parse();
    match dispatch(cli).await {
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "cli"))]
fn main() {}
