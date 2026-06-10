use ratatui::style::{Color, Modifier, Style};

use crate::tui::palette;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Theme {
    pub name: String,
    pub fg: Color,
    pub bg: Option<Color>,
    pub muted: Color,
    pub heading: [Color; 6],
    pub heading_modifier: Modifier,
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
        let lavender = palette::rgb(0xb4, 0xbe, 0xfe);
        let yellow = palette::rgb(0xf9, 0xe2, 0xaf);
        let green = palette::rgb(0xa6, 0xe3, 0xa1);
        let peach = palette::rgb(0xfa, 0xb3, 0x87);
        let subtext1 = palette::rgb(0xba, 0xc2, 0xde);
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
            heading: [mauve, sky, lavender, yellow, green, subtext1],
            heading_modifier: Modifier::BOLD,
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
        let teal = palette::rgb(0x17, 0x9d, 0x9c);
        let yellow = palette::rgb(0xdf, 0x8e, 0x1d);
        let green = palette::rgb(0x40, 0xa0, 0x2b);
        // Red (not Peach) for code/link-focus: peach #fe640b only reaches
        // ~3:1 contrast on a near-white code bg, failing WCAG AA. Red gives ~5:1.
        let red = palette::rgb(0xd2, 0x0f, 0x39);
        let subtext1 = palette::rgb(0x5c, 0x5f, 0x77);
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
            heading: [mauve, blue, teal, yellow, green, subtext1],
            heading_modifier: Modifier::BOLD,
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
