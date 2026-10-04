//! Colour (Decision 19, Q12). The design's palette as roles, not colours,
//! so the light theme and `NO_COLOR` are other tables:
//!
//! - truecolor when `COLORTERM` is `truecolor` or `24bit`;
//! - else the nearest of the 256-colour palette's cube and grey ramp;
//! - `NO_COLOR` set to anything non-empty: no colour at all, and bold, dim,
//!   reverse and underline carry focus, selection and change markers (the
//!   `~`/`-`/`+` markers are always characters, so nothing depends on
//!   colour alone);
//! - `--theme light` swaps the palette; dark is the default.
//!
//! The background is never painted: the terminal's own shows through.

use clap::ValueEnum;
use ratatui::style::{Color, Modifier, Style};

/// `--theme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

/// How colours reach the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
    NoColor,
}

/// The environment the theme is picked from (passed in, so the choice is a
/// pure function).
#[derive(Debug, Clone, Default)]
pub struct ThemeEnv {
    pub colorterm: Option<String>,
    pub no_color: Option<String>,
}

impl ThemeEnv {
    pub fn from_process() -> ThemeEnv {
        ThemeEnv {
            colorterm: std::env::var("COLORTERM").ok(),
            no_color: std::env::var("NO_COLOR").ok(),
        }
    }
}

/// What a piece of the screen is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The focused panel's border, title and active tab.
    Focus,
    /// An unfocused border.
    Border,
    Text,
    /// Inactive tabs, counters, secondary text.
    Muted,
    /// Placeholders and NULL.
    Dim,
    /// The cell cursor and the help's keys.
    Cursor,
    Modified,
    Deleted,
    Added,
    /// Column headers.
    Header,
    /// Table and object names, undo.
    Name,
    Warning,
}

/// A palette: one colour per role, plus the selection backgrounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub focus: (u8, u8, u8),
    pub border: (u8, u8, u8),
    pub text: (u8, u8, u8),
    pub muted: (u8, u8, u8),
    pub dim: (u8, u8, u8),
    pub cursor: (u8, u8, u8),
    pub modified: (u8, u8, u8),
    pub deleted: (u8, u8, u8),
    pub added: (u8, u8, u8),
    pub header: (u8, u8, u8),
    pub name: (u8, u8, u8),
    pub warning: (u8, u8, u8),
    pub selection: (u8, u8, u8),
    pub selection_unfocused: (u8, u8, u8),
}

/// The design's colours (`seaquel-tui.dc.html`: GitHub's dark palette).
pub const DARK: Palette = Palette {
    focus: (0x7e, 0xe7, 0x87),
    border: (0x30, 0x36, 0x3d),
    text: (0xc9, 0xd1, 0xd9),
    muted: (0x8b, 0x94, 0x9e),
    dim: (0x6e, 0x76, 0x81),
    cursor: (0x58, 0xa6, 0xff),
    modified: (0xe3, 0xb3, 0x41),
    deleted: (0xff, 0x7b, 0x72),
    added: (0x7e, 0xe7, 0x87),
    header: (0x56, 0xd4, 0xdd),
    name: (0xd2, 0xa8, 0xff),
    warning: (0xe3, 0xb3, 0x41),
    selection: (0x1f, 0x34, 0x50),
    // GitHub's `#21262d` (probe F7: the design's `#161b22` barely shows on
    // `#0d1117`, 1.09:1).
    selection_unfocused: (0x21, 0x26, 0x2d),
};

/// GitHub's light palette, for light terminals.
pub const LIGHT: Palette = Palette {
    focus: (0x1a, 0x7f, 0x37),
    border: (0xd1, 0xd9, 0xe0),
    text: (0x1f, 0x23, 0x28),
    muted: (0x59, 0x63, 0x6e),
    dim: (0x81, 0x8b, 0x98),
    cursor: (0x09, 0x69, 0xda),
    modified: (0x9a, 0x67, 0x00),
    deleted: (0xcf, 0x22, 0x2e),
    added: (0x1a, 0x7f, 0x37),
    header: (0x1b, 0x7c, 0x83),
    name: (0x82, 0x50, 0xdf),
    warning: (0x9a, 0x67, 0x00),
    // Probe F7: `#ddf4ff` and `#f6f8fa` became 195 and 231 (white) at 256
    // colours, so a selection, and an unfocused one even in truecolor, was
    // invisible on a white profile. These stay visible in both modes (the
    // contrast test below).
    selection: (0xc8, 0xe1, 0xff),
    selection_unfocused: (0xea, 0xee, 0xf2),
};

/// The theme the view draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub mode: ColorMode,
    pub palette: Palette,
}

impl Theme {
    /// Picks the theme from the environment and `--theme`.
    pub fn pick(env: &ThemeEnv, choice: ThemeChoice) -> Theme {
        let set = |v: &Option<String>| v.as_deref().is_some_and(|v| !v.is_empty());
        let mode = if set(&env.no_color) {
            ColorMode::NoColor
        } else if matches!(env.colorterm.as_deref(), Some("truecolor" | "24bit")) {
            ColorMode::TrueColor
        } else {
            ColorMode::Ansi256
        };
        let palette = match choice {
            ThemeChoice::Dark => DARK,
            ThemeChoice::Light => LIGHT,
        };
        Theme { mode, palette }
    }

    /// The foreground style of `role`.
    pub fn style(&self, role: Role) -> Style {
        if self.mode == ColorMode::NoColor {
            return match role {
                Role::Focus | Role::Added | Role::Warning => Style::new().bold(),
                Role::Border | Role::Text | Role::Name => Style::new(),
                Role::Muted | Role::Dim => Style::new().dim(),
                Role::Cursor => Style::new().reversed(),
                Role::Modified => Style::new().italic(),
                Role::Deleted => Style::new().crossed_out(),
                Role::Header => Style::new().bold().underlined(),
            };
        }
        let p = &self.palette;
        let rgb = match role {
            Role::Focus => p.focus,
            Role::Border => p.border,
            Role::Text => p.text,
            Role::Muted => p.muted,
            Role::Dim => p.dim,
            Role::Cursor => p.cursor,
            Role::Modified => p.modified,
            Role::Deleted => p.deleted,
            Role::Added => p.added,
            Role::Header => p.header,
            Role::Name => p.name,
            Role::Warning => p.warning,
        };
        let style = Style::new().fg(self.color(rgb));
        match role {
            // Focus is bold in every mode, so it never rests on colour alone.
            Role::Focus | Role::Header => style.add_modifier(Modifier::BOLD),
            _ => style,
        }
    }

    /// A selected row's style: in the focused panel, or another.
    pub fn selection(&self, focused: bool) -> Style {
        match (self.mode, focused) {
            (ColorMode::NoColor, true) => Style::new().reversed().bold(),
            (ColorMode::NoColor, false) => Style::new().underlined(),
            (_, true) => Style::new()
                .bg(self.color(self.palette.selection))
                .add_modifier(Modifier::BOLD),
            (_, false) => Style::new().bg(self.color(self.palette.selection_unfocused)),
        }
    }

    fn color(&self, rgb: (u8, u8, u8)) -> Color {
        match self.mode {
            ColorMode::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
            ColorMode::Ansi256 => Color::Indexed(nearest_256(rgb)),
            ColorMode::NoColor => Color::Reset,
        }
    }
}

/// The 256-colour palette's nearest entry to `rgb`: the 6×6×6 cube
/// (16–231) or the grey ramp (232–255), whichever is closer.
pub fn nearest_256(rgb: (u8, u8, u8)) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest_level = |v: u8| {
        (0..LEVELS.len())
            .min_by_key(|&i| (i32::from(LEVELS[i]) - i32::from(v)).abs())
            .unwrap_or(0)
    };
    let distance = |a: (u8, u8, u8), b: (u8, u8, u8)| {
        let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
        d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
    };
    let (r, g, b) = (
        nearest_level(rgb.0),
        nearest_level(rgb.1),
        nearest_level(rgb.2),
    );
    let cube = (LEVELS[r], LEVELS[g], LEVELS[b]);
    let cube_index = 16 + 36 * r + 6 * g + b;
    // The grey ramp: 232 + i is 8 + 10·i.
    let grey_index = (0..24)
        .min_by_key(|i| {
            let v = (8 + 10 * i) as u8;
            distance(rgb, (v, v, v))
        })
        .unwrap_or(0);
    let grey = (8 + 10 * grey_index) as u8;
    if distance(rgb, (grey, grey, grey)) < distance(rgb, cube) {
        232 + grey_index as u8
    } else {
        cube_index as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(colorterm: Option<&str>, no_color: Option<&str>) -> ThemeEnv {
        ThemeEnv {
            colorterm: colorterm.map(str::to_string),
            no_color: no_color.map(str::to_string),
        }
    }

    #[test]
    fn colorterm_picks_truecolor() {
        for value in ["truecolor", "24bit"] {
            let theme = Theme::pick(&env(Some(value), None), ThemeChoice::Dark);
            assert_eq!(theme.mode, ColorMode::TrueColor);
            assert_eq!(theme.palette, DARK);
            assert_eq!(
                theme.style(Role::Focus).fg,
                Some(Color::Rgb(0x7e, 0xe7, 0x87))
            );
        }
    }

    #[test]
    fn otherwise_the_nearest_of_256() {
        for colorterm in [None, Some(""), Some("yes")] {
            let theme = Theme::pick(&env(colorterm, None), ThemeChoice::Dark);
            assert_eq!(theme.mode, ColorMode::Ansi256, "{colorterm:?}");
        }
        let theme = Theme::pick(&env(None, None), ThemeChoice::Dark);
        // #7ee787: 126→135 (2), 231→215 (4), 135→135 (2): 16 + 72 + 24 + 2.
        assert_eq!(theme.style(Role::Focus).fg, Some(Color::Indexed(114)));
        assert_eq!(nearest_256((0, 0, 0)), 16);
        assert_eq!(nearest_256((255, 255, 255)), 231);
        // #30363d is nearest the grey ramp's 8 + 10·5 = 58 (index 237).
        assert_eq!(nearest_256((0x30, 0x36, 0x3d)), 237);
        // #ff7b72: 255→255 (5), 123→135 (2), 114→95 (1).
        assert_eq!(nearest_256((0xff, 0x7b, 0x72)), 209);
    }

    #[test]
    fn no_color_uses_attributes_only() {
        for colorterm in [None, Some("truecolor")] {
            let theme = Theme::pick(&env(colorterm, Some("1")), ThemeChoice::Light);
            assert_eq!(theme.mode, ColorMode::NoColor);
            for role in [
                Role::Focus,
                Role::Border,
                Role::Text,
                Role::Muted,
                Role::Dim,
                Role::Cursor,
                Role::Modified,
                Role::Deleted,
                Role::Added,
                Role::Header,
                Role::Name,
                Role::Warning,
            ] {
                let style = theme.style(role);
                assert_eq!((style.fg, style.bg), (None, None), "{role:?}");
            }
            assert!(theme
                .style(Role::Focus)
                .add_modifier
                .contains(Modifier::BOLD));
            assert!(theme
                .style(Role::Cursor)
                .add_modifier
                .contains(Modifier::REVERSED));
            assert!(theme
                .selection(true)
                .add_modifier
                .contains(Modifier::REVERSED));
            assert_ne!(theme.selection(true), theme.selection(false));
            assert_eq!(theme.selection(true).bg, None);
        }
        // An empty NO_COLOR doesn't count.
        let theme = Theme::pick(&env(None, Some("")), ThemeChoice::Dark);
        assert_eq!(theme.mode, ColorMode::Ansi256);
    }

    #[test]
    fn light_swaps_the_palette() {
        let theme = Theme::pick(&env(Some("truecolor"), None), ThemeChoice::Light);
        assert_eq!(theme.palette, LIGHT);
        assert_eq!(
            theme.style(Role::Focus).fg,
            Some(Color::Rgb(0x1a, 0x7f, 0x37))
        );
        assert_eq!(theme.selection(true).bg, Some(Color::Rgb(0xc8, 0xe1, 0xff)));
    }

    /// The colour a terminal shows for `color` (xterm's 256 table).
    fn rgb(color: Color) -> (u8, u8, u8) {
        match color {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(i @ 16..=231) => {
                const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
                let i = usize::from(i - 16);
                (LEVELS[i / 36], LEVELS[i / 6 % 6], LEVELS[i % 6])
            }
            Color::Indexed(i @ 232..=255) => {
                let v = 8 + 10 * (i - 232);
                (v, v, v)
            }
            other => panic!("not drawn by the themes: {other:?}"),
        }
    }

    /// WCAG's contrast ratio of two colours (1 to 21).
    fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let lum = |(r, g, b): (u8, u8, u8)| {
            0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
        };
        let (a, b) = (lum(a), lum(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Probe F7: every role can be seen in both palettes, in truecolor and
    /// at 256 colours, on the terminal backgrounds each palette is for (the
    /// design's `#0d1117` and black; white), and on both selections; and
    /// each selection stands out from the background and from the other.
    #[test]
    fn every_role_has_contrast_in_both_themes_and_both_colour_modes() {
        const ROLES: [Role; 12] = [
            Role::Focus,
            Role::Border,
            Role::Text,
            Role::Muted,
            Role::Dim,
            Role::Cursor,
            Role::Modified,
            Role::Deleted,
            Role::Added,
            Role::Header,
            Role::Name,
            Role::Warning,
        ];
        let mut failures = Vec::new();
        for (choice, backgrounds) in [
            (ThemeChoice::Dark, vec![(0x0d, 0x11, 0x17), (0, 0, 0)]),
            (ThemeChoice::Light, vec![(0xff, 0xff, 0xff)]),
        ] {
            for colorterm in [Some("truecolor"), None] {
                let theme = Theme::pick(&env(colorterm, None), choice);
                let bg = |focused: bool| rgb(theme.selection(focused).bg.unwrap());
                let (sel, unfocused) = (bg(true), bg(false));
                let mut check = |what: String, ratio: f64, min: f64| {
                    if ratio < min {
                        failures.push(format!(
                            "{choice:?} {colorterm:?} {what}: {ratio:.2} < {min}"
                        ));
                    }
                };
                check(
                    "selection vs unfocused selection".into(),
                    contrast(sel, unfocused),
                    1.1,
                );
                for &back in &backgrounds {
                    check(format!("selection on {back:?}"), contrast(sel, back), 1.15);
                    check(
                        format!("unfocused selection on {back:?}"),
                        contrast(unfocused, back),
                        1.15,
                    );
                }
                for role in ROLES {
                    let fg = rgb(theme.style(role).fg.unwrap());
                    let (on_back, on_selection) = match role {
                        Role::Border => (1.3, None),
                        Role::Dim => (2.0, Some(2.0)),
                        Role::Text => (4.5, Some(4.5)),
                        _ => (3.0, Some(3.0)),
                    };
                    for &back in &backgrounds {
                        check(format!("{role:?} on {back:?}"), contrast(fg, back), on_back);
                    }
                    if let Some(min) = on_selection {
                        check(format!("{role:?} on the selection"), contrast(fg, sel), min);
                        check(
                            format!("{role:?} on the unfocused selection"),
                            contrast(fg, unfocused),
                            min,
                        );
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn focus_is_bold_in_every_mode() {
        for e in [
            env(Some("truecolor"), None),
            env(None, None),
            env(None, Some("1")),
        ] {
            let theme = Theme::pick(&e, ThemeChoice::Dark);
            assert!(theme
                .style(Role::Focus)
                .add_modifier
                .contains(Modifier::BOLD));
        }
    }
}
