use ratatui::style::{Color, Modifier, Style};

use crate::tui::palette;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Theme {
    pub name: String,
    pub fg: Color,
    pub bg: Option<Color>,
    pub muted: Color,
    /// h1..h6, ordered brightest to dimmest so heading depth reads off
    /// brightness alone. Kept clear of `link`/`code_fg` so heading text can
    /// never be mistaken for a link or inline code.
    pub heading: [Color; 6],
    pub heading_modifier: Modifier,
    /// Chrome accents (borders, scrollbars, table headers, status pills).
    /// Separate from `heading` so retuning the heading ramp doesn't repaint
    /// the whole UI.
    pub accent: Color,
    pub accent_cool: Color,
    pub accent_warm: Color,
    pub emphasis: Modifier,
    pub strong: Modifier,
    pub code_fg: Color,
    pub code_bg: Option<Color>,
    pub link: Color,
    pub link_focused: Color,
    pub link_modifier: Modifier,
    pub quote: Color,
    pub list_marker: Color,
    pub rule: Color,
    pub strikethrough: Modifier,
    pub status_fg: Color,
    pub status_bg: Color,
    pub syntect_theme: &'static str,
}

#[allow(dead_code)]
impl Theme {
    pub fn dark() -> Self {
        // Palette: Catppuccin Mocha — pastel hues designed for dark-bg readability.
        // Avoids low-luminance blues (the default ANSI LightBlue ~ #5f87ff) that
        // wash out against #1e1e1e-class terminal backgrounds.
        let mauve = palette::rgb(0xcb, 0xa6, 0xf7);
        let sky = palette::rgb(0x89, 0xdc, 0xeb);
        let yellow = palette::rgb(0xf9, 0xe2, 0xaf);
        let green = palette::rgb(0xa6, 0xe3, 0xa1);
        let pink = palette::rgb(0xf5, 0xc2, 0xe7);
        let maroon = palette::rgb(0xeb, 0xa0, 0xac);
        let peach = palette::rgb(0xfa, 0xb3, 0x87);
        let subtext0 = palette::rgb(0xa6, 0xad, 0xc8);
        let overlay1 = palette::rgb(0x7f, 0x84, 0x9c);
        let surface2 = palette::rgb(0x58, 0x5b, 0x70);
        // Surface1 (lifted +1 from surface0) so code blocks remain visibly
        // distinct from common terminal backgrounds (#1e1e1e..#2d2d2d).
        let surface1 = palette::rgb(0x45, 0x47, 0x5a);
        let crust = palette::rgb(0x11, 0x11, 0x1b);
        Theme {
            name: "dark".into(),
            fg: Color::Reset,
            bg: None,
            muted: overlay1,
            // Relative luminance steps down monotonically .78 → .66 → .64 →
            // .47 → .46 → .42, so a bigger heading is always a brighter one.
            // Sky (link) and peach (code) are deliberately absent.
            heading: [yellow, green, pink, mauve, maroon, subtext0],
            heading_modifier: Modifier::BOLD,
            accent: mauve,
            accent_cool: sky,
            accent_warm: yellow,
            emphasis: Modifier::ITALIC,
            strong: Modifier::BOLD,
            code_fg: peach,
            code_bg: Some(surface1),
            link: sky,
            link_focused: peach,
            link_modifier: Modifier::UNDERLINED,
            quote: subtext0,
            list_marker: mauve,
            rule: surface2,
            strikethrough: Modifier::CROSSED_OUT,
            status_fg: crust,
            status_bg: mauve,
            syntect_theme: "base16-ocean.dark",
        }
    }

    pub fn light() -> Self {
        // Palette: Catppuccin Latte — darker pastels with strong contrast on
        // light terminal backgrounds (#eff1f5-class).
        let mauve = palette::rgb(0x88, 0x39, 0xef);
        let blue = palette::rgb(0x1e, 0x66, 0xf5);
        let yellow = palette::rgb(0xdf, 0x8e, 0x1d);
        // Heading ramp. Stock Latte accents are all pastel-bright, which on a
        // light bg inverts the ordering we want (yellow sits at 2.3:1 while
        // mauve sits at 4.8:1). So the six hues are darkened to a deliberate
        // contrast ladder instead: prominence = contrast against #eff1f5.
        let h_violet = palette::rgb(0x67, 0x11, 0xd8);
        let h_green = palette::rgb(0x1e, 0x6a, 0x17);
        let h_magenta = palette::rgb(0xbb, 0x1e, 0x91);
        let h_teal = palette::rgb(0x0d, 0x7e, 0x81);
        let h_amber = palette::rgb(0xb1, 0x6c, 0x11);
        let h_grey = palette::rgb(0x82, 0x85, 0x9d);
        // Red (not Peach) for code/link-focus: peach #fe640b only reaches
        // ~3:1 contrast on a near-white code bg, failing WCAG AA. Red gives ~5:1.
        let red = palette::rgb(0xd2, 0x0f, 0x39);
        let subtext0 = palette::rgb(0x6c, 0x6f, 0x85);
        let overlay1 = palette::rgb(0x8c, 0x8f, 0xa1);
        let surface1 = palette::rgb(0xbc, 0xc0, 0xcc);
        // Mantle (lighter than surface0) — gentler code background that still
        // separates from a white terminal bg.
        let mantle = palette::rgb(0xe6, 0xe9, 0xef);
        let base = palette::rgb(0xef, 0xf1, 0xf5);
        Theme {
            name: "light".into(),
            fg: Color::Reset,
            bg: None,
            muted: overlay1,
            // Contrast steps down monotonically 7.0 → 5.9 → 5.0 → 4.3 → 3.7 →
            // 3.2, so a bigger heading is always a stronger one. Blue (link)
            // and red (code) are deliberately absent.
            heading: [h_violet, h_green, h_magenta, h_teal, h_amber, h_grey],
            heading_modifier: Modifier::BOLD,
            accent: mauve,
            accent_cool: blue,
            accent_warm: yellow,
            emphasis: Modifier::ITALIC,
            strong: Modifier::BOLD,
            code_fg: red,
            code_bg: Some(mantle),
            link: blue,
            link_focused: red,
            link_modifier: Modifier::UNDERLINED,
            quote: subtext0,
            list_marker: mauve,
            rule: surface1,
            strikethrough: Modifier::CROSSED_OUT,
            status_fg: base,
            status_bg: mauve,
            syntect_theme: "InspiredGitHub",
        }
    }

    pub fn style_text(&self) -> Style {
        Style::default().fg(self.fg)
    }
}

pub fn resolve(name: &str, cfg: &crate::tui::md_config::Config) -> Theme {
    let n = if name == "auto" {
        cfg.theme.as_deref().unwrap_or("auto")
    } else {
        name
    };
    match n {
        "light" => Theme::light(),
        "dark" => Theme::dark(),
        _ => detect_terminal_theme(),
    }
}

fn detect_terminal_theme() -> Theme {
    if let Ok(v) = std::env::var("COLORFGBG") {
        // Convention: "<fg>;<bg>" — bg 7-15 light, 0-6 dark
        if let Some(bg) = v.split(';').last().and_then(|s| s.parse::<u8>().ok()) {
            if bg >= 7 {
                return Theme::light();
            }
        }
    }
    Theme::dark()
}
