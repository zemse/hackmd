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

/// Length of the conflict markers `merge3` asks diffy to emit. The marked-up
/// text is internal (parsed into [`Segment`]s, never shown raw), so we pick a
/// run far longer than git's default 7: a content line beginning with seven
/// `<`/`=`/`>`/`|` (e.g. a `=======` setext underline) would otherwise be
/// misread as a conflict marker.
const CONFLICT_MARKER_LEN: usize = 12;

fn safe_id(id: &str) -> String {
    // `id`s are HackMD note ids (URL-safe), so they're already filename-safe,
    // but guard against separators defensively.
    id.chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect()
}

/// A short, stable hash of `file`'s canonical path, so two local files linked
/// to the *same* note id don't share (and clobber) one base snapshot.
fn path_tag(file: &Path) -> String {
    // Canonicalize so the same file reached via different relative paths maps
    // to one base; fall back to the raw path before the file exists.
    let canon = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    // FNV-1a over the canonical path bytes. We deliberately avoid
    // `DefaultHasher`, whose output is not stable across Rust releases: a
    // toolchain upgrade would change every tag and orphan existing
    // `<id>.<tag>.base` caches, silently dropping the three-way merge's common
    // ancestor. FNV-1a is fixed by its own definition, so a given path always
    // maps to the same tag regardless of compiler version.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in canon.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Path of the base-content cache file for note `id` linked at `file`.
pub fn base_path(root: &Path, id: &str, file: &Path) -> PathBuf {
    root.join(CACHE_DIR)
        .join(format!("{}.{}.base", safe_id(id), path_tag(file)))
}

/// Pre-path-keying cache path (`<id>.base`). Read as a fallback so files linked
/// by an earlier release aren't orphaned. The new path-keyed format is written
/// going forward; the legacy file is left in place and not deleted.
fn legacy_base_path(root: &Path, id: &str) -> PathBuf {
    root.join(CACHE_DIR).join(format!("{}.base", safe_id(id)))
}

/// Read the cached base content for note `id` at `file`, if present. Falls back
/// to the legacy single-slot cache so existing links keep working.
pub fn read_base(root: &Path, id: &str, file: &Path) -> Option<String> {
    std::fs::read_to_string(base_path(root, id, file))
        .ok()
        .or_else(|| std::fs::read_to_string(legacy_base_path(root, id)).ok())
}

/// Write `content` as the new base for note `id` at `file`, creating the cache
/// dir as needed. Best-effort: returns the IO error so the caller can surface
/// it, but a failure doesn't corrupt anything (the merge already happened in
/// memory).
pub fn write_base(root: &Path, id: &str, file: &Path, content: &str) -> std::io::Result<()> {
    let path = base_path(root, id, file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

/// Re-file a note's base snapshot after its linked file is renamed or moved.
///
/// The cache name hashes the file's *canonical* path, so without this a rename
/// orphans the snapshot and the next sync has no common ancestor to merge
/// against, turning an otherwise clean pull into a whole-file conflict.
///
/// `old_base` must be captured with [`base_path`] **before** the rename (the
/// tag can only be computed while the file is still there to canonicalize);
/// `file` is the new location, already moved. A missing cache is a no-op.
pub fn rehome_base(root: &Path, id: &str, old_base: &Path, file: &Path) -> std::io::Result<()> {
    let new = base_path(root, id, file);
    if new == old_base || !old_base.exists() {
        return Ok(());
    }
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(old_base, new)
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

/// Prepare a string for three-way merge comparison: [`normalize_newlines`] plus
/// a single trailing newline on non-empty content.
///
/// HackMD stores note content with **no** trailing newline, while
/// [`crate::tui::hackmd_meta::strip`] always forces exactly one. Left
/// unreconciled, the two sides of an otherwise-identical first sync differ by
/// that lone newline, so [`merge3`]'s `local == remote` fast path misses and
/// (with no cached base) the whole file explodes into a spurious conflict.
/// Collapsing trailing newlines to exactly one on both sides — matching
/// `strip`'s own contract — makes identical text compare equal. Empty input
/// (an absent base) stays empty so the "nothing silently dropped" semantics of
/// an empty ancestor are preserved.
pub fn normalize_for_merge(s: &str) -> String {
    let lf = normalize_newlines(s);
    if lf.is_empty() {
        return lf;
    }
    let core = lf.trim_end_matches('\n');
    format!("{core}\n")
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
    let mut opts = diffy::MergeOptions::new();
    opts.set_conflict_marker_length(CONFLICT_MARKER_LEN);
    match opts.merge(base, local, remote) {
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

    // A real diffy marker is exactly `CONFLICT_MARKER_LEN` of the char, alone
    // on its line (the `|||||||`/`=======` separators) or followed by a space
    // and a label (`<<<<<<< ours`). Requiring that boundary stops a content
    // line that merely *starts* with a long run of the char from being eaten.
    let is_marker = |line: &str, ch: char| {
        let marker = ch.to_string().repeat(CONFLICT_MARKER_LEN);
        match line.strip_prefix(&marker) {
            Some(rest) => rest.is_empty() || rest.starts_with([' ', '\n', '\r']),
            None => false,
        }
    };

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
    fn trailing_newline_only_diff_merges_clean() {
        // The bug: HackMD stores content with no trailing newline, while
        // `hackmd_meta::strip` forces exactly one. With no cached base a first
        // sync of otherwise-identical text `merge3("", "x\n", "x")` conflicts
        // over that lone newline. `normalize_for_merge` reconciles both sides.
        let remote_no_nl = "# Title\n\nbody line"; // as HackMD returns it
        let local_stripped = "# Title\n\nbody line\n"; // as `strip` produces it
        assert_ne!(remote_no_nl, local_stripped);
        // Raw, with an empty base, this is the spurious whole-file conflict.
        assert!(matches!(
            merge3("", local_stripped, remote_no_nl),
            MergeOutcome::Conflict { .. }
        ));
        // Normalised, both collapse to one trailing newline and merge clean.
        let l = normalize_for_merge(local_stripped);
        let r = normalize_for_merge(remote_no_nl);
        assert_eq!(l, r);
        assert_eq!(merge3("", &l, &r), MergeOutcome::Clean(l.clone()));
    }

    #[test]
    fn normalize_for_merge_collapses_trailing_and_keeps_empty() {
        assert_eq!(normalize_for_merge("x"), "x\n");
        assert_eq!(normalize_for_merge("x\n"), "x\n");
        assert_eq!(normalize_for_merge("x\n\n\n"), "x\n");
        assert_eq!(normalize_for_merge("x\r\n"), "x\n");
        // An absent base stays empty so empty-ancestor semantics are preserved.
        assert_eq!(normalize_for_merge(""), "");
    }

    #[test]
    fn base_cache_roundtrips() {
        let dir = std::env::temp_dir().join(format!("hackmd-sync-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.md");
        assert!(read_base(&dir, "note1", &a).is_none());
        write_base(&dir, "note1", &a, "hello\n").unwrap();
        assert_eq!(read_base(&dir, "note1", &a).as_deref(), Some("hello\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_id_two_files_keep_independent_bases() {
        let dir = std::env::temp_dir().join(format!("hackmd-base2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.md");
        let b = dir.join("b.md");
        // Two local files linked to the same note id must not clobber each
        // other's base snapshot.
        write_base(&dir, "shared", &a, "base of a\n").unwrap();
        write_base(&dir, "shared", &b, "base of b\n").unwrap();
        assert_eq!(
            read_base(&dir, "shared", &a).as_deref(),
            Some("base of a\n")
        );
        assert_eq!(
            read_base(&dir, "shared", &b).as_deref(),
            Some("base of b\n")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_single_slot_base_is_read_as_fallback() {
        let dir = std::env::temp_dir().join(format!("hackmd-baselegacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(CACHE_DIR)).unwrap();
        // A pre-path-keying `<id>.base` left by an older release.
        std::fs::write(legacy_base_path(&dir, "old"), "legacy base\n").unwrap();
        let f = dir.join("old.md");
        assert_eq!(read_base(&dir, "old", &f).as_deref(), Some("legacy base\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn marker_char_line_inside_a_conflict_is_not_split() {
        // Overlapping edits force a conflict whose local side contains a line
        // of seven `=` (a setext underline). The old `starts_with(7×ch)` parse
        // mistook that for a `=======` separator and truncated the hunk; the
        // longer, boundary-checked marker keeps the content whole.
        let base = "intro\nshared line\noutro\n";
        let local = "intro\nlocal change\n=======\ntail\noutro\n";
        let remote = "intro\nremote change\noutro\n";
        let MergeOutcome::Conflict { segments } = merge3(base, local, remote) else {
            panic!("expected a conflict");
        };
        let (local_side, _remote_side) = segments
            .iter()
            .find_map(|s| match s {
                Segment::Conflict { local, remote } => Some((local.clone(), remote.clone())),
                _ => None,
            })
            .expect("a conflict segment");
        assert!(
            local_side.contains("=======") && local_side.contains("tail"),
            "the `=======` content line must stay inside the hunk, got {local_side:?}"
        );
    }
}
