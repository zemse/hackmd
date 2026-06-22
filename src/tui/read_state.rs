//! Machine-local "have I read this file" tracking.
//!
//! A file is *read* when we have recorded the mtime it had at the moment it was
//! read, and the file's current mtime still matches. A file is *unread* when we
//! have no record for it, or its current mtime differs from the recorded one —
//! i.e. it was created, or changed since it was last read.
//!
//! State lives in md-tui's own state dir (not in any browsed repo), so it never
//! syncs across machines and never lands in git. It is keyed by absolute path.
//! Storage format is one `mtime\tpath` line per entry — no extra dependency.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct ReadState {
    /// abs path -> mtime (whole seconds since the Unix epoch) at last read.
    map: HashMap<PathBuf, u64>,
}

impl ReadState {
    /// Load the on-disk state. On the very first run (no state file yet) we seed
    /// every text file currently under `seed_root` as read, so the user isn't
    /// met by a wall of `[unread]` badges on pre-existing files — only files
    /// created or changed *after* adoption then show as unread.
    pub fn load(seed_root: &Path) -> Self {
        let mut s = ReadState {
            map: HashMap::new(),
        };
        match std::fs::read_to_string(state_path()) {
            Ok(text) => {
                for line in text.lines() {
                    if line.is_empty() {
                        continue;
                    }
                    if let Some((mt, path)) = line.split_once('\t')
                        && let Ok(mt) = mt.parse::<u64>()
                    {
                        s.map.insert(PathBuf::from(path), mt);
                    }
                }
            }
            Err(_) => {
                // No state file (first run) or unreadable: seed-as-read snapshot.
                s.seed(seed_root);
                s.flush();
            }
        }
        s
    }

    /// True if the markdown/text file at `path` has never been read, or has
    /// changed since it was last read.
    pub fn is_unread(&self, path: &Path) -> bool {
        match self.map.get(path) {
            // Recorded: unread only if the file changed since (mtime differs).
            // If the mtime can't be read, treat as read (don't nag).
            Some(&recorded) => current_mtime(path).is_some_and(|m| m != recorded),
            None => true,
        }
    }

    /// True if any text file anywhere under `dir` is unread. Short-circuits on
    /// the first hit. Used to badge parent directories in the browser.
    pub fn dir_has_unread(&self, dir: &Path) -> bool {
        for path in text_files_under(dir) {
            if self.is_unread(&path) {
                return true;
            }
        }
        false
    }

    /// Record `path`'s current mtime as read. Idempotent: a no-op when the
    /// recorded value already matches, so it's safe to call every render frame
    /// while the reader sits at the bottom of a document.
    pub fn mark_read(&mut self, path: &Path) {
        if let Some(mt) = current_mtime(path)
            && self.map.get(path) != Some(&mt)
        {
            self.map.insert(path.to_path_buf(), mt);
        }
    }

    /// Mark a single file read, or — for a directory — mark every text file
    /// under it read recursively. Backs the browser `r` keybind.
    pub fn mark_read_recursive(&mut self, path: &Path) {
        if path.is_dir() {
            for p in text_files_under(path) {
                self.mark_read(&p);
            }
        } else {
            self.mark_read(path);
        }
    }

    /// Snapshot every text file under `root` as read (first-run seeding).
    fn seed(&mut self, root: &Path) {
        if !root.exists() {
            return;
        }
        for p in text_files_under(root) {
            if let Some(mt) = current_mtime(&p) {
                self.map.insert(p, mt);
            }
        }
    }

    /// Persist the whole map to disk. Cheap (a few lines) — called on explicit
    /// mark-read and once on app exit, not per frame.
    pub fn flush(&self) {
        let path = state_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut out = String::new();
        for (p, mt) in &self.map {
            // Our line format can't round-trip non-UTF-8 paths; skip them.
            if let Some(s) = p.to_str() {
                out.push_str(&mt.to_string());
                out.push('\t');
                out.push_str(s);
                out.push('\n');
            }
        }
        let _ = std::fs::write(&path, out);
    }
}

/// Absolute path of the state file: `<state-or-data-dir>/md/read.state`.
/// `dirs::state_dir()` follows XDG (`XDG_STATE_HOME`); on macOS/Windows it
/// returns `None`, so we fall back to the platform data dir (e.g.
/// `~/Library/Application Support`).
fn state_path() -> PathBuf {
    match dirs::state_dir().or_else(dirs::data_dir) {
        Some(d) => d.join("md").join("read.state"),
        None => PathBuf::from(".md-read.state"),
    }
}

/// File mtime in whole seconds since the Unix epoch, if readable.
fn current_mtime(path: &Path) -> Option<u64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    m.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// All text files under `dir` (recursive, gitignore/hidden-aware), matching the
/// same filter the browser uses for what it lists.
fn text_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(dir)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .require_git(false)
        .build();
    for result in walker {
        let Ok(entry) = result else { continue };
        let path = entry.path();
        if path == dir {
            continue;
        }
        let is_file = entry.file_type().is_some_and(|t| t.is_file());
        if is_file && crate::tui::app::is_text_file(path) {
            out.push(path.to_path_buf());
        }
    }
    out
}
