//! The light and dark looks: Zed's One Dark and One Light palettes.

use crate::herdr::AgentStatus;
use crate::syntax::Kind;

pub(super) type Rgb = [u8; 3];

pub(super) const fn hex(v: u32) -> Rgb {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Which look to use: the system's choice, or one of them, as chosen with
/// the button at the top right and kept in the theme file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeMode {
    pub fn name(self) -> &'static str {
        match self {
            ThemeMode::System => "system",
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }

    /// The mode named `name`; the theme file written by older gerdrs
    /// holds "dark" or "light".
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim() {
            "system" => Some(ThemeMode::System),
            "dark" => Some(ThemeMode::Dark),
            "light" => Some(ThemeMode::Light),
            _ => None,
        }
    }

    /// The look this mode gives while the system prefers dark or not.
    pub fn resolve(self, system_dark: bool) -> Theme {
        match self {
            ThemeMode::System if system_dark => Theme::Dark,
            ThemeMode::System => Theme::Light,
            ThemeMode::Dark => Theme::Dark,
            ThemeMode::Light => Theme::Light,
        }
    }
}

impl From<Theme> for ThemeMode {
    fn from(theme: Theme) -> Self {
        match theme {
            Theme::Dark => ThemeMode::Dark,
            Theme::Light => ThemeMode::Light,
        }
    }
}

/// Light or dark look, switched with the button at the top right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn toggled(self) -> Self {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            _ => None,
        }
    }

    /// What shows between and behind the apps: Zed's workspace background.
    pub fn clear_color(self) -> [f32; 4] {
        let [r, g, b] = self.palette().background;
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
    }

    pub(crate) fn palette(self) -> &'static Palette {
        match self {
            Theme::Dark => &ONE_DARK,
            Theme::Light => &ONE_LIGHT,
        }
    }
}

/// The colours the chrome uses, named after Zed's theme keys.
pub(crate) struct Palette {
    pub(super) background: Rgb,
    pub(super) panel: Rgb,
    pub(super) tab_bar: Rgb,
    pub(super) tab_active: Rgb,
    pub(super) elevated: Rgb,
    pub(super) element: Rgb,
    pub(super) selected: Rgb,
    pub(super) border: Rgb,
    pub(super) border_variant: Rgb,
    pub(super) text: Rgb,
    pub(super) muted: Rgb,
    pub(super) placeholder: Rgb,
    pub(super) accent: Rgb,
    pub(super) on_accent: Rgb,
    pub(super) success: Rgb,
    pub(super) warning: Rgb,
    pub(super) error: Rgb,
    pub(super) shadow: f32,
    /// Code's colours in a diff: Zed's `syntax` of this look.
    pub(super) syntax: SyntaxColors,
    /// What Zed's terminal uses with this look: its background and
    /// foreground, and the 16 ANSI colours (the 8 normal, then the 8
    /// bright ones, in ANSI order), pushed to Alacritty.
    pub(super) terminal_background: Rgb,
    pub(super) terminal_foreground: Rgb,
    pub(super) ansi: [Rgb; 16],
}

pub(crate) struct SyntaxColors {
    keyword: Rgb,
    string: Rgb,
    comment: Rgb,
    constant: Rgb,
    type_: Rgb,
    function: Rgb,
}

pub(super) const ONE_DARK: Palette = Palette {
    background: hex(0x3b414d),
    panel: hex(0x2f343e),
    tab_bar: hex(0x2f343e),
    tab_active: hex(0x282c33),
    elevated: hex(0x2f343e),
    element: hex(0x3b414d),
    selected: hex(0x454a56),
    border: hex(0x464b57),
    border_variant: hex(0x363c46),
    text: hex(0xdce0e5),
    muted: hex(0xa9afbc),
    placeholder: hex(0x9da3b0),
    accent: hex(0x74ade8),
    on_accent: hex(0x282c33),
    success: hex(0xa1c181),
    warning: hex(0xdec184),
    error: hex(0xd07277),
    shadow: 0.45,
    syntax: SyntaxColors {
        keyword: hex(0xb477cf),
        string: hex(0xa1c181),
        comment: hex(0x5d636f),
        constant: hex(0xbf956a),
        type_: hex(0x6eb4bf),
        function: hex(0x73ade9),
    },
    terminal_background: hex(0x282c34),
    terminal_foreground: hex(0xabb2bf),
    ansi: [
        hex(0x282c34),
        hex(0xe06c75),
        hex(0x98c379),
        hex(0xe5c07b),
        hex(0x61afef),
        hex(0xc678dd),
        hex(0x56b6c2),
        hex(0xabb2bf),
        hex(0x636d83),
        hex(0xea858b),
        hex(0xaad581),
        hex(0xffd885),
        hex(0x85c1ff),
        hex(0xd398eb),
        hex(0x6ed5de),
        hex(0xfafafa),
    ],
};

pub(super) const ONE_LIGHT: Palette = Palette {
    background: hex(0xdcdcdd),
    panel: hex(0xebebec),
    tab_bar: hex(0xebebec),
    tab_active: hex(0xfafafa),
    elevated: hex(0xebebec),
    element: hex(0xfafafa),
    selected: hex(0xd3d3d4),
    border: hex(0xc9c9ca),
    border_variant: hex(0xdfdfe0),
    text: hex(0x242529),
    muted: hex(0x58585a),
    placeholder: hex(0x646569),
    accent: hex(0x5c78e2),
    on_accent: hex(0xfafafa),
    success: hex(0x669f59),
    warning: hex(0xa48819),
    error: hex(0xd36151),
    shadow: 0.18,
    syntax: SyntaxColors {
        keyword: hex(0xa449ab),
        string: hex(0x649f57),
        comment: hex(0xa2a3a7),
        constant: hex(0xad6e25),
        type_: hex(0x3882b7),
        function: hex(0x5b79e3),
    },
    terminal_background: hex(0xfafafa),
    terminal_foreground: hex(0x2a2c33),
    ansi: [
        hex(0x000000),
        hex(0xde3e35),
        hex(0x3f953a),
        hex(0xd2b67c),
        hex(0x2f5af3),
        hex(0x950095),
        hex(0x0997b3),
        hex(0xbbbbbb),
        hex(0x000000),
        hex(0xde3e35),
        hex(0x3f953a),
        hex(0xd2b67c),
        hex(0x2f5af3),
        hex(0xa00095),
        hex(0x0bbcd6),
        hex(0xffffff),
    ],
};

impl Palette {
    /// The colour code of `kind` is drawn in.
    pub(super) fn code(&self, kind: Kind) -> Rgb {
        let s = &self.syntax;
        match kind {
            Kind::Keyword => s.keyword,
            Kind::String => s.string,
            Kind::Comment => s.comment,
            Kind::Constant => s.constant,
            Kind::Type => s.type_,
            Kind::Function => s.function,
        }
    }

    pub(super) fn status(&self, status: AgentStatus) -> Rgb {
        match status {
            AgentStatus::Working => self.accent,
            AgentStatus::Blocked => self.warning,
            AgentStatus::Done => self.success,
            AgentStatus::Idle => self.muted,
            AgentStatus::Unknown => self.placeholder,
        }
    }
}

/// Alacritty's names for the ANSI colours, in ANSI order.
const ANSI_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// `#rrggbb` for `rgb`.
fn css(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

impl Palette {
    /// The `-o` options giving Alacritty this look's terminal colours, to
    /// append to its command line: `-o`, then `key="#rrggbb"` (a TOML
    /// string, as Alacritty reads an option's value), for the background,
    /// the foreground and the 16 ANSI colours.
    pub fn alacritty_options(&self) -> Vec<String> {
        let mut options = Vec::with_capacity(36);
        let mut option = |key: String, rgb: Rgb| {
            options.push("-o".to_owned());
            options.push(format!("{key}=\"{}\"", css(rgb)));
        };
        option("colors.primary.background".into(), self.terminal_background);
        option("colors.primary.foreground".into(), self.terminal_foreground);
        for (i, name) in ANSI_NAMES.iter().enumerate() {
            option(format!("colors.normal.{name}"), self.ansi[i]);
            option(format!("colors.bright.{name}"), self.ansi[i + 8]);
        }
        options
    }
}

/// `a` moved `t` of the way to `b`.
pub(super) fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::{ONE_DARK, ONE_LIGHT, Palette, Rgb, Theme, ThemeMode};

    /// WCAG relative luminance.
    fn luminance([r, g, b]: Rgb) -> f64 {
        let channel = |c: u8| {
            let c = c as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG contrast ratio, 1 to 21.
    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_colours_read_on_their_backgrounds() {
        for (name, p) in [("One Dark", &ONE_DARK), ("One Light", &ONE_LIGHT)] {
            let p: &Palette = p;
            for (what, colour) in [
                ("placeholder", p.placeholder),
                ("muted", p.muted),
                ("text", p.text),
            ] {
                let ratio = contrast(colour, p.panel);
                assert!(
                    ratio >= 4.5,
                    "{name}: {what} on panel is {ratio:.2}:1, under 4.5:1"
                );
            }
            assert!(contrast(p.on_accent, p.accent) >= 3.0, "{name}: on_accent");
        }
    }

    #[test]
    fn alacritty_gets_the_terminal_colours_as_toml_options() {
        let options = ONE_DARK.alacritty_options();
        // Background, foreground, 8 normal and 8 bright: 18 options.
        assert_eq!(options.len(), 36);
        assert!(options.iter().step_by(2).all(|flag| flag == "-o"));
        assert_eq!(options[1], "colors.primary.background=\"#282c34\"");
        assert_eq!(options[3], "colors.primary.foreground=\"#abb2bf\"");
        assert_eq!(options[5], "colors.normal.black=\"#282c34\"");
        assert_eq!(options[7], "colors.bright.black=\"#636d83\"");
        assert!(options.contains(&"colors.normal.red=\"#e06c75\"".to_owned()));
        assert!(options.contains(&"colors.bright.white=\"#fafafa\"".to_owned()));
        let light = ONE_LIGHT.alacritty_options();
        assert_eq!(light[1], "colors.primary.background=\"#fafafa\"");
        assert!(light.contains(&"colors.normal.blue=\"#2f5af3\"".to_owned()));
        // A dark terminal reads light on dark, and the other way round.
        for (p, dark) in [(&ONE_DARK, true), (&ONE_LIGHT, false)] {
            let p: &Palette = p;
            let bg = luminance(p.terminal_background);
            let fg = luminance(p.terminal_foreground);
            assert_eq!(fg > bg, dark);
            assert!(contrast(p.terminal_foreground, p.terminal_background) >= 4.5);
        }
    }

    #[test]
    fn a_mode_resolves_to_a_theme_and_names_round_trip() {
        assert_eq!(ThemeMode::System.resolve(true), Theme::Dark);
        assert_eq!(ThemeMode::System.resolve(false), Theme::Light);
        assert_eq!(ThemeMode::Dark.resolve(false), Theme::Dark);
        assert_eq!(ThemeMode::Light.resolve(true), Theme::Light);
        for mode in [ThemeMode::System, ThemeMode::Dark, ThemeMode::Light] {
            assert_eq!(ThemeMode::from_name(mode.name()), Some(mode));
        }
        assert_eq!(ThemeMode::from_name(" light\n"), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::from_name("blue"), None);
        assert_eq!(ThemeMode::from(Theme::Light), ThemeMode::Light);
    }
}
