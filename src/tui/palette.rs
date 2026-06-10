//! Terminal color-depth detection + RGB downgrade.
//!
//! Apple Terminal.app and other older emulators only speak xterm-256, not
//! 24-bit truecolor. Sending `Color::Rgb` to those terminals either drops the
//! color outright or produces garbage. We probe `COLORTERM` once and, if
//! truecolor isn't advertised, map every RGB triple to its nearest entry in
//! the xterm-256 palette so syntax highlighting and theme accents survive.

use ratatui::style::Color;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorDepth {
    TrueColor,
    Indexed256,
}

static DEPTH: OnceLock<ColorDepth> = OnceLock::new();

pub fn depth() -> ColorDepth {
    *DEPTH.get_or_init(detect)
}

fn detect() -> ColorDepth {
    // Strongest signal: explicit `COLORTERM` advertisement. Honored even on
    // Apple Terminal so a user can force-upgrade if Apple ever adds support.
    if let Ok(v) = std::env::var("COLORTERM") {
        let v = v.to_ascii_lowercase();
        if v == "truecolor" || v == "24bit" {
            return ColorDepth::TrueColor;
        }
    }

    // Apple Terminal.app advertises 256 colors only and does not support
    // truecolor as of macOS 15. Pin it down even if `COLORTERM` got stripped
    // (e.g. by ssh) so we don't fall through to a hopeful TERM-based guess.
    if let Ok(p) = std::env::var("TERM_PROGRAM") {
        if p == "Apple_Terminal" {
            return ColorDepth::Indexed256;
        }
        // Known truecolor-capable terminal emulators that may not set
        // COLORTERM after env stripping.
        if matches!(
            p.as_str(),
            "iTerm.app" | "vscode" | "WezTerm" | "ghostty" | "Hyper"
        ) {
            return ColorDepth::TrueColor;
        }
    }

    // `xterm-direct`, `*-direct`, and `*-truecolor` terminfo entries advertise
    // direct color. Kitty also identifies itself this way.
    if let Ok(t) = std::env::var("TERM") {
        if t.ends_with("-direct") || t.contains("truecolor") || t == "xterm-kitty" {
            return ColorDepth::TrueColor;
        }
    }

    ColorDepth::Indexed256
}

/// Returns `Color::Rgb` when truecolor is supported, otherwise the nearest
/// `Color::Indexed` entry in the xterm-256 palette.
pub fn rgb(r: u8, g: u8, b: u8) -> Color {
    match depth() {
        ColorDepth::TrueColor => Color::Rgb(r, g, b),
        ColorDepth::Indexed256 => Color::Indexed(rgb_to_xterm256(r, g, b)),
    }
}

/// Map (r,g,b) → nearest xterm-256 index. Considers both the 6×6×6 color cube
/// (16..=231) and the 24-step grayscale ramp (232..=255), picking whichever is
/// closer in squared-RGB distance.
fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let cube_idx = |c: u8| -> usize {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|&(_, &v)| (v as i32 - c as i32).abs())
            .map(|(i, _)| i)
            .unwrap()
    };
    let ri = cube_idx(r);
    let gi = cube_idx(g);
    let bi = cube_idx(b);
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let cube_rgb = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_dist = sq_dist((r, g, b), cube_rgb);

    // Grayscale ramp: index = 232 + n, gray value = 8 + 10n for n in 0..24.
    let avg = (r as u16 + g as u16 + b as u16) / 3;
    let gray_n = if avg < 8 {
        0
    } else if avg > 238 {
        23
    } else {
        ((avg - 8) / 10) as u8
    };
    let gray_v = 8 + gray_n * 10;
    let gray_idx = 232 + gray_n;
    let gray_dist = sq_dist((r, g, b), (gray_v, gray_v, gray_v));

    if gray_dist < cube_dist {
        gray_idx
    } else {
        cube as u8
    }
}

fn sq_dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    let dr = a.0 as i32 - b.0 as i32;
    let dg = a.1 as i32 - b.1 as i32;
    let db = a.2 as i32 - b.2 as i32;
    dr * dr + dg * dg + db * db
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_white_maps_to_cube_white() {
        assert_eq!(rgb_to_xterm256(255, 255, 255), 231);
    }

    #[test]
    fn pure_black_maps_to_cube_black() {
        assert_eq!(rgb_to_xterm256(0, 0, 0), 16);
    }

    #[test]
    fn mid_gray_uses_grayscale_ramp() {
        let idx = rgb_to_xterm256(128, 128, 128);
        assert!(
            (232..=255).contains(&idx),
            "expected grayscale, got {}",
            idx
        );
    }

    #[test]
    fn saturated_red_picks_cube_red() {
        // Should land on or near 196 (16 + 36*5 + 0 + 0).
        assert_eq!(rgb_to_xterm256(255, 0, 0), 196);
    }
}
