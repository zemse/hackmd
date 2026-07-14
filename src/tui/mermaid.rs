//! Render a ```mermaid fenced code block into terminal-friendly diagram text.
//!
//! Mermaid has no pure-Rust rendering engine, so we lean on `merman-core` to
//! parse the source into a typed model and `merman-ascii` to lay that model out
//! as Unicode box-drawing lines. Only the diagram types `merman-ascii` supports
//! (sequence, flowchart, class, ER, xychart) render; anything else — or any
//! parse error — returns `None` so the caller falls back to showing the raw
//! fenced source as a highlighted code block.

use merman_ascii::{AsciiRenderOptions, render_model};
use merman_core::{Engine, ParseOptions};

/// Render `source` (the body of a ```mermaid fence) to plain diagram lines with
/// no trailing whitespace. Returns `None` when the diagram can't be parsed or
/// its type isn't supported by the ASCII renderer.
pub fn render(source: &str) -> Option<Vec<String>> {
    // `lenient` would hand back an `error` diagram model for bad input, which
    // we'd then try to render; `strict` fails fast so we can fall back cleanly.
    let parsed = Engine::new()
        .parse_diagram_for_render_model_sync(source, ParseOptions::strict())
        .ok()??;
    let out = render_model(&parsed.model, &AsciiRenderOptions::unicode()).ok()?;
    let lines: Vec<String> = out.lines().map(|l| l.trim_end().to_string()).collect();
    if lines.iter().all(|l| l.is_empty()) {
        return None;
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_sequence_diagram() {
        let src = "sequenceDiagram\n    A->>B: hello\n    B-->>A: hi";
        let lines = render(src).expect("sequence renders");
        let joined = lines.join("\n");
        assert!(joined.contains('A'));
        assert!(joined.contains('B'));
        assert!(joined.contains("hello"));
    }

    #[test]
    fn unknown_diagram_falls_back() {
        assert!(render("not a mermaid diagram at all").is_none());
    }
}
