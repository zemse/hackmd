//! Open `$EDITOR` on a temp Markdown file and read it back on close.
//!
//! Ports `_ref/hackmd-cli/src/open-editor.ts` + `utils.ts:40-46`.
//! Improvement over upstream: the tempfile is auto-cleaned via the [`tempfile`]
//! crate's RAII Drop, where upstream leaks the file under `$TMPDIR`.

use std::process::Command;

use crate::error::{Error, Result};

/// Spawn the user's editor on an empty `.md` tempfile and return the contents
/// once the editor exits.
///
/// Editor resolution: `$VISUAL` → `$EDITOR` → platform default
/// (`notepad` on Windows, `vim` everywhere else).
pub fn open_in_editor() -> Result<String> {
    let tmp = tempfile::Builder::new()
        .suffix(".md")
        .tempfile()
        .map_err(Error::Io)?;
    let path = tmp.path().to_path_buf();

    let editor = resolve_editor();
    // Allow editor + flags via shell-style splitting (mirrors upstream).
    let mut parts = editor.split_whitespace();
    let bin = parts
        .next()
        .ok_or_else(|| Error::Config("editor binary not found".into()))?;
    let extra_args: Vec<&str> = parts.collect();

    let status = Command::new(bin)
        .args(&extra_args)
        .arg(&path)
        .status()
        .map_err(Error::Io)?;
    if !status.success() {
        return Err(Error::Config(format!(
            "editor `{bin}` exited with status {status}"
        )));
    }

    let content = std::fs::read_to_string(&path).map_err(Error::Io)?;
    // tmp dropped here → file cleaned up.
    Ok(content)
}

fn resolve_editor() -> String {
    if let Ok(v) = std::env::var("VISUAL")
        && !v.is_empty()
    {
        return v;
    }
    if let Ok(e) = std::env::var("EDITOR")
        && !e.is_empty()
    {
        return e;
    }
    default_editor().to_string()
}

#[cfg(windows)]
fn default_editor() -> &'static str {
    "notepad"
}

#[cfg(not(windows))]
fn default_editor() -> &'static str {
    "vim"
}

#[cfg(test)]
mod tests {
    use super::*;

    // SAFETY: tests in this module manipulate process env and so must NOT run
    // concurrently with anything else that reads VISUAL/EDITOR. They're kept
    // narrow + serial by Rust's per-module test ordering for a single test
    // function. We avoid coupling to other tests by not touching env outside
    // these tests.

    #[test]
    fn resolve_falls_back_to_platform_default() {
        // Snapshot + clear, restore on drop.
        struct G {
            v: Option<String>,
            e: Option<String>,
        }
        impl Drop for G {
            fn drop(&mut self) {
                unsafe {
                    match &self.v {
                        Some(v) => std::env::set_var("VISUAL", v),
                        None => std::env::remove_var("VISUAL"),
                    }
                    match &self.e {
                        Some(e) => std::env::set_var("EDITOR", e),
                        None => std::env::remove_var("EDITOR"),
                    }
                }
            }
        }
        let _g = G {
            v: std::env::var("VISUAL").ok(),
            e: std::env::var("EDITOR").ok(),
        };
        unsafe {
            std::env::remove_var("VISUAL");
            std::env::remove_var("EDITOR");
        }
        assert_eq!(resolve_editor(), default_editor());
    }
}
