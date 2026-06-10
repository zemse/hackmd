//! `md` — terminal markdown reader/editor with mouse + clickable links.
//!
//! Same CLI surface as the standalone md-tui binary this crate absorbed:
//! `md`, `md file.md`, `md dir/`, piped stdin, `-w/-l/-s` flags.

use anyhow::Result;
use clap::Parser;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use hackmd::tui::{LaunchOpts, app};

#[derive(Parser, Debug)]
#[command(
    name = "md",
    version,
    about = "Markdown reader TUI with mouse + clickable links"
)]
struct Cli {
    /// File or directory to view. If omitted and stdin is piped, reads stdin.
    /// If omitted with a TTY, browses the current directory.
    path: Option<PathBuf>,

    /// Word-wrap width (0 = use terminal width)
    #[arg(short = 'w', long, default_value_t = 0)]
    width: u16,

    /// Show line numbers
    #[arg(short = 'l', long)]
    line_numbers: bool,

    /// Theme: dark | light | auto
    #[arg(short = 's', long, default_value = "auto")]
    style: String,

    /// Pager mode (kept for glow CLI parity)
    #[arg(short = 'p', long)]
    pager: bool,

    /// TUI mode (kept for glow CLI parity)
    #[arg(short = 't', long)]
    tui: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let source = match cli.path.as_deref() {
        Some(p) if p.is_dir() => app::Source::Directory(p.to_path_buf()),
        Some(p) => app::Source::File(p.to_path_buf()),
        None => {
            if io::stdin().is_terminal() {
                app::Source::Directory(std::env::current_dir()?)
            } else {
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                app::Source::Stdin(buf)
            }
        }
    };
    let _ = (cli.pager, cli.tui);

    hackmd::tui::run_blocking(LaunchOpts {
        source,
        width: cli.width,
        line_numbers: cli.line_numbers,
        style: cli.style,
    })
}
