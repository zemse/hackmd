//! Per-line JSON expansion for `.json` / `.jsonl` / `.ndjson` files.
//!
//! When viewing a JSON-line file in the reader, any source line that would
//! overflow the body width gets a clickable button in the left gutter. Click
//! it and the line is replaced (in the rendered view only) with its pretty-
//! printed multi-line form. Click again to collapse. We keep the on-disk
//! content untouched.

/// Pretty-print a single JSON value into 2-space-indented lines. Returns
/// `None` if `s` doesn't parse as one complete JSON value (any trailing
/// non-whitespace also fails the parse).
pub fn prettify(s: &str) -> Option<Vec<String>> {
    let mut p = Parser {
        input: s.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let val = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.input.len() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    write_value(&val, 0, &mut out, &mut cur);
    out.push(cur);
    Some(out)
}

#[derive(Debug)]
enum Value {
    Null,
    Bool(bool),
    /// Raw token text, preserved verbatim so the printer doesn't reformat the
    /// user's number.
    Num(String),
    /// Raw quoted string including surrounding `"` and original escapes.
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => self.parse_string_raw().map(Value::Str),
            b't' | b'f' => self.parse_bool(),
            b'n' => self.parse_null(),
            b'-' | b'0'..=b'9' => self.parse_number().map(Value::Num),
            _ => None,
        }
    }

    fn parse_string_raw(&mut self) -> Option<String> {
        let start = self.pos;
        if self.peek() != Some(b'"') {
            return None;
        }
        self.pos += 1;
        while let Some(b) = self.peek() {
            if b == b'\\' {
                self.pos += 1;
                if self.pos >= self.input.len() {
                    return None;
                }
                self.pos += 1;
            } else if b == b'"' {
                self.pos += 1;
                return std::str::from_utf8(&self.input[start..self.pos])
                    .ok()
                    .map(|s| s.to_string());
            } else if b < 0x20 {
                return None;
            } else {
                self.pos += 1;
            }
        }
        None
    }

    fn parse_bool(&mut self) -> Option<Value> {
        if self.input[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Some(Value::Bool(true))
        } else if self.input[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Some(Value::Bool(false))
        } else {
            None
        }
    }

    fn parse_null(&mut self) -> Option<Value> {
        if self.input[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Some(Value::Null)
        } else {
            None
        }
    }

    fn parse_number(&mut self) -> Option<String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(b) = self.peek() {
            if matches!(b, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start || (self.pos == start + 1 && self.input[start] == b'-') {
            return None;
        }
        std::str::from_utf8(&self.input[start..self.pos])
            .ok()
            .map(|s| s.to_string())
    }

    fn parse_object(&mut self) -> Option<Value> {
        if self.peek() != Some(b'{') {
            return None;
        }
        self.pos += 1;
        self.skip_ws();
        let mut items: Vec<(String, Value)> = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Some(Value::Obj(items));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return None;
            }
            let key = self.parse_string_raw()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return None;
            }
            self.pos += 1;
            let v = self.parse_value()?;
            items.push((key, v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Some(Value::Obj(items));
                }
                _ => return None,
            }
        }
    }

    fn parse_array(&mut self) -> Option<Value> {
        if self.peek() != Some(b'[') {
            return None;
        }
        self.pos += 1;
        self.skip_ws();
        let mut items: Vec<Value> = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Some(Value::Arr(items));
        }
        loop {
            let v = self.parse_value()?;
            items.push(v);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Some(Value::Arr(items));
                }
                _ => return None,
            }
        }
    }
}

/// Emit `v` indented at `indent` columns. Containers open their bracket on
/// the current line, push that line to `out`, indent the body by +2, then
/// place the closing bracket on a fresh line whose indent is back to
/// `indent`. The final fragment (last line of the value) is left in `cur`
/// for the caller to extend (with `,` etc.).
fn write_value(v: &Value, indent: usize, out: &mut Vec<String>, cur: &mut String) {
    match v {
        Value::Null => cur.push_str("null"),
        Value::Bool(true) => cur.push_str("true"),
        Value::Bool(false) => cur.push_str("false"),
        Value::Num(s) => cur.push_str(s),
        Value::Str(s) => cur.push_str(s),
        Value::Arr(items) => {
            if items.is_empty() {
                cur.push_str("[]");
            } else {
                cur.push('[');
                out.push(std::mem::take(cur));
                let inner = " ".repeat(indent + 2);
                for (i, item) in items.iter().enumerate() {
                    let mut line = inner.clone();
                    write_value(item, indent + 2, out, &mut line);
                    if i + 1 < items.len() {
                        line.push(',');
                    }
                    out.push(line);
                }
                cur.push_str(&" ".repeat(indent));
                cur.push(']');
            }
        }
        Value::Obj(items) => {
            if items.is_empty() {
                cur.push_str("{}");
            } else {
                cur.push('{');
                out.push(std::mem::take(cur));
                let inner = " ".repeat(indent + 2);
                for (i, (k, val)) in items.iter().enumerate() {
                    let mut line = format!("{}{}: ", inner, k);
                    write_value(val, indent + 2, out, &mut line);
                    if i + 1 < items.len() {
                        line.push(',');
                    }
                    out.push(line);
                }
                cur.push_str(&" ".repeat(indent));
                cur.push('}');
            }
        }
    }
}

/// A clickable expand/collapse button rendered in the left gutter of a JSON
/// reader line. `line` / `col_start` / `col_end` are display-space hit-box
/// coordinates; `source_line` is the 0-based index into the file's source
/// lines that the button controls.
#[derive(Clone, Debug)]
pub struct JsonlButton {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub source_line: usize,
}

#[derive(Default, Clone, Debug)]
pub struct JsonlOverlay {
    pub buttons: Vec<JsonlButton>,
}

impl JsonlOverlay {
    /// Returns the index of the button covering `(line, col)`, if any.
    pub fn at(&self, line: usize, col: usize) -> Option<usize> {
        self.buttons
            .iter()
            .position(|b| b.line == line && col >= b.col_start && col < b.col_end)
    }
}

/// Inject expand/collapse glyphs into the first column of the rendered Pre
/// block produced by a JSON-line file's render. The Pre layout reserves a
/// two-cell `pad_left` at the front of each row; we paint over its first cell
/// with `▶` (collapsed) / `▼` (expanded) for the head row of each long source
/// line, and `│` for continuation rows of an expanded line so the user can
/// see the group.
///
/// `raw_lines` is the original file split on `\n`. `line_map[code_idx]` is
/// the source-line index that produced code line `code_idx`. `pre_start` is
/// the display row at which the Pre block starts. `body_width` is the cell
/// budget for the full Pre row (used to decide what counts as overflow).
///
/// Returns the populated overlay with hit boxes in display coordinates.
pub fn inject_buttons(
    lines: &mut [ratatui::text::Line<'static>],
    raw_lines: &[&str],
    line_map: &[usize],
    expanded: &std::collections::HashSet<usize>,
    pre_start: usize,
    body_width: usize,
    accent: ratatui::style::Color,
) -> JsonlOverlay {
    use ratatui::style::{Modifier, Style};
    use unicode_width::UnicodeWidthStr;

    let mut overlay = JsonlOverlay::default();
    if line_map.is_empty() {
        return overlay;
    }
    // Threshold: a source line overflows when its display width can't fit
    // inside the Pre content area (body_width minus 2-cell pad_left).
    let limit = body_width.saturating_sub(2);

    let head_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let guide_style = Style::default().fg(accent);

    let mut code_idx = 0usize;
    while code_idx < line_map.len() {
        let src_idx = line_map[code_idx];
        // Find the contiguous run of code lines that share this source line.
        let mut run_end = code_idx + 1;
        while run_end < line_map.len() && line_map[run_end] == src_idx {
            run_end += 1;
        }
        let is_expanded = expanded.contains(&src_idx);
        let raw_line = raw_lines.get(src_idx).copied().unwrap_or("");
        let overflows = raw_line.width() > limit;
        if overflows || is_expanded {
            // Head row: replace `pad_left` (the first two-char span) with a
            // glyph + space.
            let head_row = pre_start + code_idx;
            if let Some(line) = lines.get_mut(head_row) {
                let glyph = if is_expanded { "▼" } else { "▶" };
                replace_pad_left(line, glyph, head_style);
                overlay.buttons.push(JsonlButton {
                    line: head_row,
                    col_start: 0,
                    col_end: 2,
                    source_line: src_idx,
                });
            }
            // Continuation rows: visual guide so the user sees the group.
            for offset in (code_idx + 1)..run_end {
                let row = pre_start + offset;
                if let Some(line) = lines.get_mut(row) {
                    replace_pad_left(line, "│", guide_style);
                    // Continuation rows also accept clicks so a misclick at
                    // the bottom of a tall expanded block still collapses it.
                    overlay.buttons.push(JsonlButton {
                        line: row,
                        col_start: 0,
                        col_end: 2,
                        source_line: src_idx,
                    });
                }
            }
        }
        code_idx = run_end;
    }
    overlay
}

/// Replace the first cell of `line`'s leading 2-space pad with `glyph`, keeping
/// the existing background (cell 1 stays as a space). The Pre layout always
/// emits the pad as the first content span after any quote-prefix runs; for
/// top-level code blocks (the JSONL case) there is no quote prefix, so the
/// pad is the very first span.
fn replace_pad_left(
    line: &mut ratatui::text::Line<'static>,
    glyph: &str,
    glyph_style: ratatui::style::Style,
) {
    use ratatui::text::Span;

    let Some(first) = line.spans.first_mut() else {
        return;
    };
    if first.content.as_ref() != "  " {
        return;
    }
    let bg_style = first.style;
    let pad_after = Span::styled(" ".to_string(), bg_style);
    *first = Span::styled(glyph.to_string(), glyph_style);
    line.spans.insert(1, pad_after);
}

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use std::collections::HashSet;

    fn pre_row(content: &str) -> Line<'static> {
        let bg = Style::default();
        Line::from(vec![
            Span::styled("  ".to_string(), bg),
            Span::raw(content.to_string()),
        ])
    }

    #[test]
    fn long_lines_get_collapse_button() {
        let mut lines = vec![pre_row("short"), pre_row(&"x".repeat(200))];
        let raw = ["short", &"x".repeat(200)];
        let raw_refs: Vec<&str> = raw.iter().map(|s| *s).collect();
        let map = vec![0, 1];
        let expanded = HashSet::new();
        let overlay = inject_buttons(&mut lines, &raw_refs, &map, &expanded, 0, 80, Color::Reset);
        assert_eq!(overlay.buttons.len(), 1);
        assert_eq!(overlay.buttons[0].source_line, 1);
        assert_eq!(lines[0].spans[0].content.as_ref(), "  ");
        assert_eq!(lines[1].spans[0].content.as_ref(), "▶");
    }

    #[test]
    fn expanded_lines_show_guide_on_continuation() {
        let mut lines = vec![pre_row("{"), pre_row("  k: 1"), pre_row("}")];
        let raw = [r#"{"k":1}"#];
        let raw_refs: Vec<&str> = raw.iter().map(|s| *s).collect();
        let map = vec![0, 0, 0];
        let mut expanded = HashSet::new();
        expanded.insert(0);
        let overlay = inject_buttons(&mut lines, &raw_refs, &map, &expanded, 0, 80, Color::Reset);
        assert_eq!(overlay.buttons.len(), 3);
        assert_eq!(lines[0].spans[0].content.as_ref(), "▼");
        assert_eq!(lines[1].spans[0].content.as_ref(), "│");
        assert_eq!(lines[2].spans[0].content.as_ref(), "│");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettify_flat_object() {
        let out = prettify(r#"{"a":1,"b":"two"}"#).unwrap();
        assert_eq!(out, vec!["{", "  \"a\": 1,", "  \"b\": \"two\"", "}"]);
    }

    #[test]
    fn prettify_nested() {
        let out = prettify(r#"{"a":{"b":[1,2]}}"#).unwrap();
        assert_eq!(
            out,
            vec![
                "{",
                "  \"a\": {",
                "    \"b\": [",
                "      1,",
                "      2",
                "    ]",
                "  }",
                "}",
            ]
        );
    }

    #[test]
    fn prettify_empty_containers() {
        assert_eq!(prettify("{}"), Some(vec!["{}".to_string()]));
        assert_eq!(prettify("[]"), Some(vec!["[]".to_string()]));
    }

    #[test]
    fn prettify_scalars() {
        assert_eq!(prettify("42"), Some(vec!["42".to_string()]));
        assert_eq!(prettify("true"), Some(vec!["true".to_string()]));
        assert_eq!(prettify("null"), Some(vec!["null".to_string()]));
        assert_eq!(prettify("\"hello\""), Some(vec!["\"hello\"".to_string()]));
    }

    #[test]
    fn prettify_rejects_garbage() {
        assert_eq!(prettify("not json"), None);
        assert_eq!(prettify("{unquoted: 1}"), None);
        assert_eq!(prettify("{\"a\": 1,}"), None);
        assert_eq!(prettify("{\"a\": 1} extra"), None);
        assert_eq!(prettify(""), None);
    }

    #[test]
    fn prettify_preserves_string_escapes() {
        let out = prettify(r#"{"k":"a\"b\\c"}"#).unwrap();
        assert_eq!(out, vec!["{", "  \"k\": \"a\\\"b\\\\c\"", "}"]);
    }
}
