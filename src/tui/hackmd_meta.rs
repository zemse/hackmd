//! Local ↔ HackMD link bookkeeping embedded in a markdown file.
//!
//! When a local file is published to HackMD we stamp a small HTML comment at
//! the very top of the file recording the note's id and links. HTML comments
//! are invisible in rendered markdown (HackMD's own preview, GitHub, this
//! TUI's reader), so the block is unobtrusive, but it lets a later "publish"
//! recognise the file as already-linked and *update* the existing note instead
//! of creating a duplicate.
//!
//! Format (key: value lines bounded by an opening tag and `-->`):
//!
//! ```text
//! <!-- hackmd
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
//! `<!-- hackmd` marker, so hand-edited whitespace doesn't break round-trips.

/// The opening marker for the managed block. A line whose trimmed content
/// equals this starts the block; the next `-->` line closes it.
const OPEN_MARKER: &str = "<!-- hackmd";
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
/// line, end just past the `-->` line's newline). `None` if absent.
fn block_span(content: &str) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    let mut start: Option<usize> = None;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        match start {
            None => {
                if trimmed == OPEN_MARKER {
                    start = Some(offset);
                } else if !trimmed.is_empty() {
                    // The block, if present, must be the first non-blank
                    // content. A non-blank line that isn't the marker means
                    // there's no (leading) block to manage.
                    return None;
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

/// Content with the managed block removed (and any single blank line that
/// followed it), leaving the user's actual document. If no block is present
/// the input is returned unchanged.
pub fn strip(content: &str) -> String {
    let Some((start, mut end)) = block_span(content) else {
        return content.to_string();
    };
    // Swallow one blank separator line after the block so repeated
    // strip/insert cycles don't accumulate blank lines.
    let rest = &content[end..];
    if let Some(nl) = rest.find('\n') {
        if rest[..nl].trim().is_empty() {
            end += nl + 1;
        }
    } else if rest.trim().is_empty() {
        end = content.len();
    }
    let mut out = String::with_capacity(content.len());
    out.push_str(&content[..start]);
    out.push_str(&content[end..]);
    out
}

/// Insert or replace the managed block at the top of `content`, returning the
/// updated document. The user's text (everything after any existing block) is
/// preserved verbatim, separated from the block by one blank line.
pub fn upsert(content: &str, meta: &HackmdMeta, synced: &str) -> String {
    let body = strip(content);
    let block = block(meta, synced);
    if body.trim().is_empty() {
        // Brand-new / empty file: block plus a trailing newline.
        format!("{block}\n")
    } else {
        format!("{block}\n\n{body}")
    }
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
    fn upsert_then_parse_roundtrips() {
        let doc = "# Title\n\nbody text\n";
        let stamped = upsert(doc, &meta(), "2026-06-21T00:00:00Z");
        // The original body survives untouched after the block.
        assert!(stamped.ends_with("# Title\n\nbody text\n"));
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
        // Body preserved.
        assert!(twice.ends_with("hello\n"));
    }

    #[test]
    fn strip_removes_block_and_separator() {
        let stamped = upsert("real content\n", &meta(), "t");
        assert_eq!(strip(&stamped), "real content\n");
    }

    #[test]
    fn parse_none_when_no_block() {
        assert!(parse("# Just a doc\n").is_none());
        // A block not at the very top isn't ours to manage.
        assert!(parse("text\n<!-- hackmd\nid: x\n-->\n").is_none());
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
