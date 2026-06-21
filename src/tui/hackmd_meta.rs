//! Local ↔ HackMD link bookkeeping embedded in a markdown file.
//!
//! When a local file is published to HackMD we stamp a small HTML comment at
//! the bottom of the file recording the note's id and links. HTML comments
//! are invisible in rendered markdown (HackMD's own preview, GitHub, this
//! TUI's reader), so the block is unobtrusive, but it lets a later "publish"
//! recognise the file as already-linked and *update* the existing note instead
//! of creating a duplicate. (Blocks left at the top by older versions are
//! still recognised and migrate to the bottom on the next write.)
//!
//! Format (key: value lines bounded by an opening tag and `-->`):
//!
//! ```text
//! <!-- hackmd-sync
//! id: AbCdEf123
//! url: https://hackmd.io/AbCdEf123
//! publish: https://hackmd.io/s/xyz
//! team: my-team
//! synced: 2026-06-21T10:11:12Z
//! -->
//! ```
//!
//! `team` is omitted for personal notes. Parsing is line-oriented and
//! tolerant: unknown keys are ignored and the block is matched by its opening
//! `<!-- hackmd-sync` marker, so hand-edited whitespace doesn't break
//! round-trips. The marker is deliberately specific (not a bare `<!-- hackmd`)
//! so a comment a user writes by hand is far less likely to collide, and a
//! block is only ever *removed* when it parses to valid link metadata (see
//! [`strip`]) — a stray marker with no `id:` is left untouched, never eaten.

/// The opening marker for the managed block. A line whose trimmed content
/// equals this starts the block; the next `-->` line closes it.
const OPEN_MARKER: &str = "<!-- hackmd-sync";
/// Earlier releases stamped a bare `<!-- hackmd` marker. Still recognised on
/// read so already-linked files aren't orphaned (and re-create duplicates);
/// they migrate to [`OPEN_MARKER`] on the next write.
const LEGACY_OPEN_MARKER: &str = "<!-- hackmd";
const CLOSE_MARKER: &str = "-->";

/// Parsed link metadata for a published note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HackmdMeta {
    pub id: String,
    /// `Some(path)` for a team note, `None` for a personal note.
    pub team_path: Option<String>,
    pub url: String,
    pub publish_link: String,
}

impl HackmdMeta {
    pub fn editor_url(id: &str) -> String {
        format!("https://hackmd.io/{id}")
    }
}

/// Locate the managed block in `content`, returning the inclusive
/// `(start_byte, end_byte)` range of the lines it spans (start at the `<!--`
/// line, end just past the `-->` line's newline). `None` if absent. The block
/// may sit anywhere (it lives at the bottom now, but older files put it at the
/// top); the first `<!-- hackmd`…`-->` pair wins.
fn block_span(content: &str) -> Option<(usize, usize)> {
    let is_open = |trimmed: &str| trimmed == OPEN_MARKER || trimmed == LEGACY_OPEN_MARKER;
    let mut offset = 0usize;
    let mut start: Option<usize> = None;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        match start {
            None => {
                if is_open(trimmed) {
                    start = Some(offset);
                }
            }
            Some(s) => {
                if trimmed == CLOSE_MARKER {
                    return Some((s, offset + line.len()));
                }
            }
        }
        offset += line.len();
    }
    None
}

/// Parse the managed block at the top of `content`, if any.
pub fn parse(content: &str) -> Option<HackmdMeta> {
    let (start, end) = block_span(content)?;
    let mut id = String::new();
    let mut team_path = None;
    let mut url = String::new();
    let mut publish_link = String::new();
    for line in content[start..end].lines() {
        let line = line.trim();
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let val = val.trim().to_string();
        match key.trim() {
            "id" => id = val,
            "team" => team_path = (!val.is_empty()).then_some(val),
            "url" => url = val,
            "publish" => publish_link = val,
            _ => {}
        }
    }
    if id.is_empty() {
        return None;
    }
    Some(HackmdMeta {
        id,
        team_path,
        url,
        publish_link,
    })
}

/// True if `content` carries a managed-block marker, whether or not it parses.
/// Lets a caller tell a corrupted link block (e.g. the `id:` line was deleted)
/// apart from a genuinely unlinked file, so it can warn instead of creating a
/// duplicate note.
pub fn has_block(content: &str) -> bool {
    block_span(content).is_some()
}

/// Render the managed block (without a trailing blank line). `synced` is an
/// already-formatted timestamp string (e.g. RFC 3339).
pub fn block(meta: &HackmdMeta, synced: &str) -> String {
    let mut s = String::new();
    s.push_str(OPEN_MARKER);
    s.push('\n');
    s.push_str(&format!("id: {}\n", meta.id));
    s.push_str(&format!("url: {}\n", meta.url));
    if !meta.publish_link.is_empty() {
        s.push_str(&format!("publish: {}\n", meta.publish_link));
    }
    if let Some(team) = &meta.team_path {
        s.push_str(&format!("team: {team}\n"));
    }
    s.push_str(&format!("synced: {synced}\n"));
    s.push_str(CLOSE_MARKER);
    s
}

/// The user's document with the managed block (and its surrounding blank-line
/// separators) removed, normalised to end in a single newline. If no block is
/// present the input is returned unchanged.
pub fn strip(content: &str) -> String {
    // Only remove a block that actually parses to valid link metadata. A
    // marker the user typed by hand (e.g. while documenting this very feature)
    // with no `id:` line is left in place rather than silently eaten.
    if parse(content).is_none() {
        return content.to_string();
    }
    let Some((start, end)) = block_span(content) else {
        return content.to_string();
    };
    // Text before the block, with the blank-line separator we insert before it
    // trimmed off.
    let head = content[..start].trim_end_matches(['\n', '\r', ' ', '\t']);
    // Any real text after the block (rare — the block sits at the bottom), with
    // leading blank lines trimmed.
    let tail = content[end..].trim_start_matches(['\n', '\r']);
    let mut out = String::with_capacity(content.len());
    if !head.is_empty() {
        out.push_str(head);
        out.push('\n');
    }
    if !tail.is_empty() {
        out.push_str(tail);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Insert or replace the managed block at the *bottom* of `content`, returning
/// the updated document. The user's text is preserved above it, separated by
/// one blank line.
pub fn upsert(content: &str, meta: &HackmdMeta, synced: &str) -> String {
    let body = strip(content);
    let block = block(meta, synced);
    if body.trim().is_empty() {
        format!("{block}\n")
    } else {
        // `body` ends with a single newline; one more gives the blank-line
        // separator before the block.
        format!("{body}\n{block}\n")
    }
}

/// Ensure the attribution footer ([`crate::TUI_FOOTER`]) is present in `body`,
/// appending it (after a `---` rule) when missing. Idempotent — once added,
/// the footer is plain content the user may edit or delete, and it won't be
/// re-added on later syncs. Used only at first publish, mirroring `hackmd new`.
pub fn ensure_footer(body: &str) -> String {
    if body.contains(crate::TUI_FOOTER) {
        return body.to_string();
    }
    let core = body.trim_end_matches(['\n', '\r', ' ', '\t']);
    if core.is_empty() {
        return format!("{}\n", crate::TUI_FOOTER);
    }
    format!("{core}\n\n---\n\n{}\n", crate::TUI_FOOTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> HackmdMeta {
        HackmdMeta {
            id: "AbCdEf123".into(),
            team_path: None,
            url: "https://hackmd.io/AbCdEf123".into(),
            publish_link: "https://hackmd.io/s/xyz".into(),
        }
    }

    #[test]
    fn upsert_puts_block_at_bottom_and_parse_roundtrips() {
        let doc = "# Title\n\nbody text\n";
        let stamped = upsert(doc, &meta(), "2026-06-21T00:00:00Z");
        // The body comes first; the block is appended at the bottom.
        assert!(stamped.starts_with("# Title\n\nbody text\n"));
        assert!(stamped.trim_end().ends_with("-->"));
        let parsed = parse(&stamped).expect("block parses");
        assert_eq!(parsed, meta());
    }

    #[test]
    fn upsert_is_idempotent_on_id_and_does_not_stack_blocks() {
        let doc = "hello\n";
        let once = upsert(doc, &meta(), "t1");
        let twice = upsert(&once, &meta(), "t2");
        // Exactly one opening marker after a re-stamp.
        assert_eq!(twice.matches(OPEN_MARKER).count(), 1);
        // Body preserved at the top.
        assert!(twice.starts_with("hello\n"));
    }

    #[test]
    fn strip_removes_bottom_block_and_separator() {
        let stamped = upsert("real content\n", &meta(), "t");
        assert_eq!(strip(&stamped), "real content\n");
    }

    #[test]
    fn strip_then_upsert_round_trips_body() {
        // The base/local comparison the sync relies on: strip(upsert(x)) == x
        // for a body that already ends in a single newline.
        let body = "# Doc\n\nsome text\n";
        let stamped = upsert(body, &meta(), "t");
        assert_eq!(strip(&stamped), body);
    }

    #[test]
    fn parse_finds_block_anywhere() {
        assert!(parse("# Just a doc\n").is_none());
        // A block at the bottom (the new placement) is recognised.
        assert!(parse("text\n\n<!-- hackmd\nid: x\n-->\n").is_some());
        // As is one left at the top by an older version.
        assert!(parse("<!-- hackmd\nid: y\n-->\n\ntext\n").is_some());
    }

    #[test]
    fn ensure_footer_appends_once() {
        let body = "# Doc\n\ntext\n";
        let with = ensure_footer(body);
        assert!(with.contains(crate::TUI_FOOTER));
        assert!(with.contains("\n---\n"));
        // Idempotent: a second call doesn't add a second footer.
        let again = ensure_footer(&with);
        assert_eq!(again, with);
        assert_eq!(again.matches(crate::TUI_FOOTER).count(), 1);
    }

    #[test]
    fn strip_leaves_a_hand_written_marker_untouched() {
        // A user documenting this feature writes the marker but no real `id:`
        // block. It must survive `strip` verbatim rather than being eaten.
        let doc = "# Notes\n\n<!-- hackmd-sync is the sync marker -->\n\nmore\n";
        assert_eq!(strip(doc), doc);
        // Even a multi-line block that never closes with a valid `id:` stays.
        let doc2 = "intro\n\n<!-- hackmd-sync\nnot a real block\n-->\n\ntail\n";
        assert_eq!(strip(doc2), doc2);
        assert!(parse(doc2).is_none());
    }

    #[test]
    fn legacy_bare_marker_still_parses_and_migrates_on_write() {
        // Files stamped by an older release used a bare `<!-- hackmd` marker.
        let legacy = "body\n\n<!-- hackmd\nid: Old123\nurl: https://hackmd.io/Old123\n-->\n";
        let parsed = parse(legacy).expect("legacy block parses");
        assert_eq!(parsed.id, "Old123");
        // Re-stamping migrates it to the new marker, leaving exactly one block.
        let restamped = upsert(legacy, &parsed, "t");
        assert_eq!(restamped.matches(OPEN_MARKER).count(), 1);
        assert!(!restamped.contains("<!-- hackmd\n"));
    }

    #[test]
    fn has_block_detects_corrupted_block() {
        // Marker present but the `id:` line was deleted → corrupted, not
        // unlinked. `has_block` sees it even though `parse` rejects it.
        let corrupted = "text\n\n<!-- hackmd-sync\nurl: https://hackmd.io/x\n-->\n";
        assert!(has_block(corrupted));
        assert!(parse(corrupted).is_none());
        // A plain unlinked file has no block.
        assert!(!has_block("# Just a doc\n"));
    }

    #[test]
    fn team_note_roundtrips_team_path() {
        let m = HackmdMeta {
            team_path: Some("my-team".into()),
            ..meta()
        };
        let stamped = upsert("x\n", &m, "t");
        assert_eq!(parse(&stamped).unwrap().team_path, Some("my-team".into()));
    }
}
