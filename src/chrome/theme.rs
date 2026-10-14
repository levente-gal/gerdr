//! The light and dark looks: Zed's One Dark and One Light palettes.

use crate::herdr::AgentStatus;

pub(super) type Rgb = [u8; 3];

pub(super) const fn hex(v: u32) -> Rgb {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
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
    pub(super) shadow: f32,
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
    placeholder: hex(0x878a98),
    accent: hex(0x74ade8),
    on_accent: hex(0x282c33),
    success: hex(0xa1c181),
    warning: hex(0xdec184),
    shadow: 0.45,
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
    placeholder: hex(0x7e8086),
    accent: hex(0x5c78e2),
    on_accent: hex(0xfafafa),
    success: hex(0x669f59),
    warning: hex(0xa48819),
    shadow: 0.18,
};

impl Palette {
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

/// `a` moved `t` of the way to `b`.
pub(super) fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}
