//! Crash-recovery of unsaved editor buffers.
//!
//! While a file is open in the editor with unsaved changes, the buffer is
//! mirrored to a recovery file under the system temp dir, keyed by the edited
//! file's absolute path. If the app dies before the edit is saved (Ctrl-C,
//! crash, `kill`, terminal closed), the next open of that file offers to
//! restore the buffer. The recovery file is deleted once the edit is saved or
//! the user explicitly discards it.
//!
//! The format is plain text: line 1 is the absolute source path (so a hash
//! collision can be rejected), line 2 is the cursor byte offset, and
//! everything after the second newline is the buffer verbatim.

use std::path::{Path, PathBuf};

/// A buffer recovered from a recovery file.
pub struct Recovered {
    pub content: String,
    pub cursor: usize,
}

/// Directory holding recovery files (created on demand).
fn dir() -> PathBuf {
    std::env::temp_dir().join("hackmd-rs-recovery")
}

/// FNV-1a 64-bit hash. Small, dependency-free, and stable within a build —
/// good enough to name a recovery file after a path.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Hash of buffer contents — used by the autosave throttle to skip rewriting
/// an unchanged buffer.
pub fn content_hash(content: &str) -> u64 {
    fnv1a(content.as_bytes())
}

/// Absolute form of `path` for stable keying. Unlike `canonicalize` this
/// doesn't require the file to exist and never touches the filesystem.
fn abs(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Recovery file path for a given source file.
fn file_for(path: &Path) -> PathBuf {
    let a = abs(path);
    dir().join(format!(
        "{:016x}.recovery",
        fnv1a(a.to_string_lossy().as_bytes())
    ))
}

/// Mirror `content` (with `cursor`) to the recovery file for `path`. Silent on
/// failure — recovery is best-effort and must never interrupt editing.
pub fn save(path: &Path, content: &str, cursor: usize) {
    if std::fs::create_dir_all(dir()).is_err() {
        return;
    }
    let a = abs(path);
    let mut data = String::with_capacity(content.len() + 64);
    data.push_str(&a.to_string_lossy());
    data.push('\n');
    data.push_str(&cursor.to_string());
    data.push('\n');
    data.push_str(content);
    let _ = std::fs::write(file_for(path), data);
}

/// Delete the recovery file for `path` (no-op if absent).
pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(file_for(path));
}

/// Read the recovery file for `path`, validating the stored path to guard
/// against hash collisions. `None` if absent or mismatched.
pub fn load(path: &Path) -> Option<Recovered> {
    let data = std::fs::read_to_string(file_for(path)).ok()?;
    // path \n cursor \n content (content may itself contain newlines).
    let mut parts = data.splitn(3, '\n');
    let stored_path = parts.next()?;
    let cursor = parts.next()?;
    let content = parts.next().unwrap_or("");
    if stored_path != abs(path).to_string_lossy() {
        return None;
    }
    Some(Recovered {
        content: content.to_string(),
        cursor: cursor.parse().unwrap_or(0),
    })
}

/// A recovery worth offering: present, and different from what's on disk now.
/// A stale recovery that matches the file is tidied away and not offered.
pub fn pending(path: &Path, disk_content: &str) -> Option<Recovered> {
    let rec = load(path)?;
    if rec.content == disk_content {
        clear(path);
        None
    } else {
        Some(rec)
    }
}
