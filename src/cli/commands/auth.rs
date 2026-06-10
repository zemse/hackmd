//! `login`, `logout`, `whoami`.
//!
//! Ports `_ref/hackmd-cli/src/commands/{login,logout,whoami}.ts`.

use std::path::Path;

use crate::Client;
use crate::cli::config::{self, effective};
use crate::cli::output::{OutputOpts, print_table};
use crate::error::{Error, Result};

/// Default columns for `whoami` (matches upstream column set).
const WHOAMI_COLUMNS: &[&str] = &["id", "email", "name", "userPath"];

/// `hackmd login` — prompt for a token, validate via `me()`, persist it.
pub async fn login(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
) -> Result<()> {
    let eff = effective(config_dir, cli_endpoint, cli_token)?;

    // Already logged in? Validate, short-circuit.
    if let Some(token) = eff.token.clone() {
        let client = Client::with_endpoint(token, eff.endpoint.clone())?;
        match client.me().await {
            Ok(user) => {
                println!("Already logged in as {} ({})", user.name, user.user_path);
                return Ok(());
            }
            Err(_) => {
                eprintln!("Stored credentials are invalid — please re-enter your token.");
            }
        }
    }

    // Point the user at the exact page where tokens are created. Only for
    // the official endpoint — a self-hosted instance has its own URL.
    if eff.endpoint == crate::client::DEFAULT_ENDPOINT {
        const TOKEN_SETTINGS_URL: &str = "https://hackmd.io/settings#api";
        println!("Create an access token at: {TOKEN_SETTINGS_URL}");
        // Best-effort: opens locally, silently does nothing over SSH.
        let _ = open::that_detached(TOKEN_SETTINGS_URL);
    }

    let token = prompt_token_masked("Enter your HackMD access token: ")?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(Error::Config("empty token".into()));
    }

    let client = Client::with_endpoint(token.clone(), eff.endpoint.clone())?;
    match client.me().await {
        Ok(_) => {
            config::set_access_token(&eff.config_dir, &token)?;
            println!("Login successfully");
            Ok(())
        }
        Err(e) => {
            eprintln!("Login failed");
            Err(e)
        }
    }
}

/// Read a secret from the terminal, echoing one `*` per character so
/// typing AND pasting give visible feedback. Backspace works; Enter
/// submits; Ctrl-C/Ctrl-D abort. Falls back to a plain (echo-less) line
/// read when stdin isn't a TTY (e.g. `echo $TOKEN | hackmd login`).
fn prompt_token_masked(prompt: &str) -> Result<String> {
    use std::io::{IsTerminal, Write};

    let mut stdout = std::io::stdout();
    print!("{prompt}");
    stdout.flush().map_err(Error::Io)?;

    if !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(Error::Io)?;
        println!();
        return Ok(line);
    }

    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    use crossterm::terminal;

    terminal::enable_raw_mode().map_err(|e| Error::Config(format!("raw mode: {e}")))?;
    let mut buf = String::new();
    let res = loop {
        match read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') | KeyCode::Char('d')
                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    break Err(Error::Config("interrupted".into()));
                }
                KeyCode::Backspace => {
                    if buf.pop().is_some() {
                        let _ = write!(stdout, "\x08 \x08");
                        let _ = stdout.flush();
                    }
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    buf.push(c);
                    let _ = write!(stdout, "*");
                    let _ = stdout.flush();
                }
                _ => {}
            },
            // Terminals with bracketed paste active deliver the whole paste
            // as one event — echo one star per character all the same.
            Ok(Event::Paste(s)) => {
                for c in s.chars().filter(|c| !c.is_control()) {
                    buf.push(c);
                    let _ = write!(stdout, "*");
                }
                let _ = stdout.flush();
            }
            Err(e) => break Err(Error::Config(format!("read key: {e}"))),
            _ => {}
        }
    };
    // Always restore the terminal before returning, error or not.
    let _ = terminal::disable_raw_mode();
    println!();
    res.map(|()| buf)
}

/// `hackmd logout` — clear the stored token (file stays in place).
pub fn logout(config_dir: Option<&Path>) -> Result<()> {
    let dir = config::resolve_config_dir(config_dir)?;
    config::set_access_token(&dir, "")?;
    println!("You've logged out successfully");
    Ok(())
}

/// `hackmd whoami` — render the authenticated user as a table.
pub async fn whoami(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    opts: &OutputOpts,
) -> Result<()> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let user = client.me().await?;
    print_table(&[user], WHOAMI_COLUMNS, opts)
}
