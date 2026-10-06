//! `hackmd reveal` — show a file selected in the system file manager.
//!
//! A local `PATH` is revealed as is. With `--note-id` the note is written to a
//! temp file named after its title first, since a cloud note has no file on
//! disk. Either way the file can then be dragged into a chat app.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::CachedResponse;
use crate::error::{Error, Result};

pub async fn run(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    path: Option<PathBuf>,
    note_id: Option<&str>,
) -> Result<()> {
    let file = match (path, note_id) {
        (Some(p), _) => {
            std::fs::metadata(&p).map_err(|e| Error::Config(format!("{}: {e}", p.display())))?;
            p.canonicalize().unwrap_or(p)
        }
        (None, Some(id)) => export_to_temp(config_dir, cli_endpoint, cli_token, id).await?,
        (None, None) => return Err(Error::MissingArgument("PATH or --note-id")),
    };
    reveal(&file)?;
    println!("{}", file.display());
    Ok(())
}

async fn export_to_temp(
    config_dir: Option<&Path>,
    cli_endpoint: Option<&str>,
    cli_token: Option<&str>,
    note_id: &str,
) -> Result<PathBuf> {
    let (client, _eff) = super::build_client(config_dir, cli_endpoint, cli_token)?;
    let body = match client.note(note_id, None).await? {
        CachedResponse::Modified { body, .. } => body,
        CachedResponse::NotModified => {
            return Err(Error::Config(
                "unexpected 304 from server (no ETag was sent)".into(),
            ));
        }
    };
    // One folder per note id so two notes with the same title don't collide.
    let dir = std::env::temp_dir().join("hackmd-reveal").join(note_id);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{}.md", file_stem(&body.title)));
    std::fs::write(&file, body.content)?;
    Ok(file)
}

/// A title made safe to use as a file name.
fn file_stem(title: &str) -> String {
    let stem: String = title
        .trim()
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let stem = stem.trim_matches('.');
    if stem.is_empty() {
        "untitled".into()
    } else {
        stem.into()
    }
}

/// Show `file` selected in the platform's file manager.
fn reveal(file: &Path) -> Result<()> {
    if cfg!(target_os = "macos") {
        run_ok(Command::new("open").arg("-R").arg(file))
    } else if cfg!(target_os = "windows") {
        // `explorer` exits 1 even when it succeeds, so the status is ignored.
        // `canonicalize` yields a `\\?\` path that `/select,` doesn't accept.
        let plain = file.to_string_lossy();
        let plain = plain.strip_prefix(r"\\?\").unwrap_or(&plain);
        Command::new("explorer")
            .arg(format!("/select,{plain}"))
            .status()?;
        Ok(())
    } else {
        // The FileManager1 D-Bus call selects the file; fall back to opening
        // the containing folder where no file manager implements it.
        let selected = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{}", file_uri(file)))
            .arg("string:")
            .output()
            .is_ok_and(|o| o.status.success());
        if selected {
            return Ok(());
        }
        let dir = file.parent().unwrap_or(Path::new("."));
        run_ok(Command::new("xdg-open").arg(dir))
    }
}

fn run_ok(cmd: &mut Command) -> Result<()> {
    let status = cmd.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "{:?} failed: {status}",
            cmd.get_program()
        )))
    }
}

/// `file://` URI with everything outside the unreserved set percent-encoded.
fn file_uri(file: &Path) -> String {
    let mut uri = String::from("file://");
    for b in file.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stem_replaces_path_separators() {
        assert_eq!(file_stem("a/b: c"), "a-b- c");
    }

    #[test]
    fn file_uri_percent_encodes_spaces_and_unicode() {
        assert_eq!(
            file_uri(Path::new("/tmp/a b/é.md")),
            "file:///tmp/a%20b/%C3%A9.md"
        );
    }

    #[test]
    fn file_stem_falls_back_when_empty() {
        assert_eq!(file_stem("  "), "untitled");
        assert_eq!(file_stem(".."), "untitled");
    }
}
