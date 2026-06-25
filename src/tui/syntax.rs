use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::tui::palette;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEMES: OnceLock<ThemeSet> = OnceLock::new();

fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}
fn themes() -> &'static ThemeSet {
    THEMES.get_or_init(ThemeSet::load_defaults)
}

/// Highlight `code` and append styled spans to `out` line-by-line.
/// Each input line becomes a separate `Vec<Span<'static>>` pushed to `out`.
pub fn highlight(
    code: &str,
    lang_token: Option<&str>,
    theme_name: &str,
    out: &mut Vec<Vec<Span<'static>>>,
) {
    let ss = syntaxes();
    let ts = themes();
    let Some(theme) = ts
        .themes
        .get(theme_name)
        .or_else(|| ts.themes.values().next())
    else {
        // No themes at all (should never happen with `load_defaults`): fall
        // back to unstyled text rather than panicking.
        for line in code.lines() {
            out.push(vec![Span::raw(line.to_string())]);
        }
        return;
    };
    let syntax = lang_token
        .and_then(|t| ss.find_syntax_by_token(t))
        .or_else(|| ss.find_syntax_by_first_line(code))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut hl = HighlightLines::new(syntax, theme);
    for line in LinesWithEndings::from(code) {
        let regions = match hl.highlight_line(line, ss) {
            Ok(r) => r,
            Err(_) => {
                out.push(vec![Span::raw(line.trim_end_matches('\n').to_string())]);
                continue;
            }
        };
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (s, txt) in regions {
            let trimmed = txt.trim_end_matches('\n');
            if trimmed.is_empty() {
                continue;
            }
            spans.push(Span::styled(trimmed.to_string(), syn_to_ratatui(s)));
        }
        out.push(spans);
    }
}

fn syn_to_ratatui(s: SynStyle) -> Style {
    let mut style =
        Style::default().fg(palette::rgb(s.foreground.r, s.foreground.g, s.foreground.b));
    if s.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if s.font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}
