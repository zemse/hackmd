//! Bidirectional local ↔ HackMD sync: three-way merge plus the on-disk
//! "base" cache that records the last-synced content so we can tell local and
//! upstream edits apart.
//!
//! The link itself lives in the file's managed `<!-- hackmd … -->` block (see
//! [`crate::tui::hackmd_meta`]); this module owns the *content* side of the
//! sync. The base for note `<id>` is cached at `<root>/.hackmd/<id>.base`, the
//! common ancestor fed to the three-way merge.

use std::path::{Path, PathBuf};

/// Directory under the search root holding per-note base snapshots.
const CACHE_DIR: &str = ".hackmd";

/// Path of the base-content cache file for `id` under `root`.
pub fn base_path(root: &Path, id: &str) -> PathBuf {
    // `id`s are HackMD note ids (URL-safe), so they're already filename-safe,
    // but guard against separators defensively.
    let safe: String = id
        .chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect();
    root.join(CACHE_DIR).join(format!("{safe}.base"))
}

/// Read the cached base content for `id`, if present.
pub fn read_base(root: &Path, id: &str) -> Option<String> {
    std::fs::read_to_string(base_path(root, id)).ok()
}

/// Write `content` as the new base for `id`, creating the cache dir as needed.
/// Best-effort: returns the IO error so the caller can surface it, but a
/// failure doesn't corrupt anything (the merge already happened in memory).
pub fn write_base(root: &Path, id: &str, content: &str) -> std::io::Result<()> {
    let path = base_path(root, id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

/// Normalise line endings to `\n`. HackMD may return `\r\n` while the local
/// file is `\n`; [`merge3`] compares exact strings, so without this every line
/// would read as changed and a first sync would explode into a whole-file
/// conflict. Cheap no-op when there's no `\r`.
pub fn normalize_newlines(s: &str) -> String {
    if s.contains('\r') {
        s.replace("\r\n", "\n")
    } else {
        s.to_string()
    }
}

/// One piece of a (possibly conflicted) merged document, in document order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    /// Text both sides agree on — emitted verbatim.
    Stable(String),
    /// A region local and upstream both changed. `local`/`remote` are the two
    /// candidate texts (already newline-terminated as in the source).
    Conflict { local: String, remote: String },
}

/// Result of a three-way merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Merged cleanly; the string is the final content.
    Clean(String),
    /// At least one region conflicts; `segments` reconstructs the whole
    /// document with the conflicting regions called out for the resolver.
    Conflict { segments: Vec<Segment> },
}

/// Three-way merge of `local` and `remote` against their common ancestor
/// `base`. Clean merges return the combined text; overlapping edits return
/// structured conflict segments for the resolver UI.
pub fn merge3(base: &str, local: &str, remote: &str) -> MergeOutcome {
    // Fast paths: nothing to do, or only one side moved.
    if local == remote {
        return MergeOutcome::Clean(local.to_string());
    }
    if base == local {
        return MergeOutcome::Clean(remote.to_string());
    }
    if base == remote {
        return MergeOutcome::Clean(local.to_string());
    }
    match diffy::merge(base, local, remote) {
        Ok(merged) => MergeOutcome::Clean(merged),
        Err(conflicted) => MergeOutcome::Conflict {
            segments: parse_conflicts(&conflicted),
        },
    }
}

/// Parse diffy's conflict-marked merge output into ordered segments. Handles
/// both plain (`<<<<<<< / ======= / >>>>>>>`) and diff3 (`<<<<<<< / |||||||
/// base / ======= / >>>>>>>`) marker styles by discarding the base section.
fn parse_conflicts(text: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut stable = String::new();
    let mut lines = text.split_inclusive('\n').peekable();

    let is_marker = |line: &str, ch: char| line.starts_with(&ch.to_string().repeat(7));

    while let Some(line) = lines.next() {
        if is_marker(line, '<') {
            // Flush accumulated stable text before the conflict.
            if !stable.is_empty() {
                segments.push(Segment::Stable(std::mem::take(&mut stable)));
            }
            let mut local = String::new();
            let mut remote = String::new();
            // "ours" lines until the base (|||||||) or separator (=======).
            let mut in_base = false;
            for l in lines.by_ref() {
                if is_marker(l, '|') {
                    in_base = true;
                } else if is_marker(l, '=') {
                    break;
                } else if !in_base {
                    local.push_str(l);
                }
            }
            // "theirs" lines until the closing marker (>>>>>>>).
            for l in lines.by_ref() {
                if is_marker(l, '>') {
                    break;
                }
                remote.push_str(l);
            }
            segments.push(Segment::Conflict { local, remote });
        } else {
            stable.push_str(line);
        }
    }
    if !stable.is_empty() {
        segments.push(Segment::Stable(stable));
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_edits_merge_clean() {
        let base = "a\nb\nc\n";
        let local = "A\nb\nc\n"; // changed first line
        let remote = "a\nb\nC\n"; // changed last line
        match merge3(base, local, remote) {
            MergeOutcome::Clean(s) => assert_eq!(s, "A\nb\nC\n"),
            other => panic!("expected clean, got {other:?}"),
        }
    }

    #[test]
    fn same_edit_on_both_sides_is_clean() {
        let base = "a\nb\n";
        let both = "a\nX\n";
        assert_eq!(merge3(base, both, both), MergeOutcome::Clean(both.into()));
    }

    #[test]
    fn one_sided_edit_takes_that_side() {
        let base = "a\nb\n";
        assert_eq!(
            merge3(base, "a\nb\n", "a\nZ\n"),
            MergeOutcome::Clean("a\nZ\n".into())
        );
        assert_eq!(
            merge3(base, "a\nY\n", "a\nb\n"),
            MergeOutcome::Clean("a\nY\n".into())
        );
    }

    #[test]
    fn overlapping_edits_conflict_with_both_sides() {
        let base = "title\nshared\n";
        let local = "title\nlocal change\n";
        let remote = "title\nremote change\n";
        let MergeOutcome::Conflict { segments } = merge3(base, local, remote) else {
            panic!("expected conflict");
        };
        // There must be a conflict segment carrying both candidate texts.
        let conflict = segments
            .iter()
            .find_map(|s| match s {
                Segment::Conflict { local, remote } => Some((local.clone(), remote.clone())),
                _ => None,
            })
            .expect("a conflict segment");
        assert!(conflict.0.contains("local change"));
        assert!(conflict.1.contains("remote change"));
    }

    #[test]
    fn crlf_remote_against_lf_local_merges_clean() {
        // The bug: HackMD returns `\r\n`, the local file is `\n`. Without
        // normalisation every line differs and the same content conflicts.
        let local = "line one\nline two\n";
        let remote_crlf = "line one\r\nline two\r\n";
        // Raw, the two are unequal and a no-base merge would conflict.
        assert_ne!(local, remote_crlf);
        let local_n = normalize_newlines(local);
        let remote_n = normalize_newlines(remote_crlf);
        assert_eq!(local_n, remote_n);
        assert_eq!(
            merge3("", &local_n, &remote_n),
            MergeOutcome::Clean(local.into())
        );
    }

    #[test]
    fn base_cache_roundtrips() {
        let dir = std::env::temp_dir().join(format!("hackmd-sync-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_base(&dir, "note1").is_none());
        write_base(&dir, "note1", "hello\n").unwrap();
        assert_eq!(read_base(&dir, "note1").as_deref(), Some("hello\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
