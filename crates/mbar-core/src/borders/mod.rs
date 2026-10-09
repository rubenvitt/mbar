//! Window borders: the JankyBorders take-over (`docs/spec/borders.md`, design in
//! `docs/superpowers/specs/2026-10-09-borders-design.md`).
//!
//! The core owns the borders **configuration** (`--borders`, `--query borders`); the
//! platform owns the live set of tracked windows and draws the border windows. Every change
//! reaches the platform as one [`crate::platform::PlatformRequest::SetBorders`].

mod parse;

pub use parse::{apply_to_window, parse_arg, validate_args, ArgError};

/// What `borders -v` prints (JankyBorders v1.9.0, the version the spec describes).
pub const BORDERS_VERSION: &str = "borders-v1.9.0";

/// Direction of a two-color gradient (`gradient(top_left=…,bottom_right=…)` or
/// `gradient(top_right=…,bottom_left=…)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientDirection {
    TopLeftToBottomRight,
    TopRightToBottomLeft,
}

/// A border or background color (spec BR-PARSE-05). Colors are `0xAARRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderColor {
    Solid(u32),
    Glow(u32),
    Gradient {
        direction: GradientDirection,
        color1: u32,
        color2: u32,
    },
}

/// `style=` (BR-PARSE-03): any style char other than `s`/`u` draws round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    Round,
    Square,
    Uniform,
}

/// `order=` (BR-PARSE-04): above or below the target window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderOrder {
    Above,
    Below,
}

/// One full set of border settings (`struct settings`, spec §2.2). Used for the global
/// settings and for every `apply-to` override.
#[derive(Debug, Clone, PartialEq)]
pub struct BorderSettings {
    pub active: BorderColor,
    pub inactive: BorderColor,
    pub background: BorderColor,
    /// Whether the background is drawn (BR-PARSE-07).
    pub show_background: bool,
    pub width: f32,
    pub style: BorderStyle,
    pub order: BorderOrder,
    pub hidpi: bool,
    /// `None` = auto: on when mbar is trusted for Accessibility (JankyBorders' default).
    pub ax_focus: Option<bool>,
    /// Exact, case-sensitive BSD process names; empty = disabled.
    pub blacklist: Vec<String>,
    pub whitelist: Vec<String>,
}

impl Default for BorderSettings {
    /// JankyBorders' defaults (`src/main.c:31-46`).
    fn default() -> Self {
        Self {
            active: BorderColor::Solid(0xffe1e3e4),
            inactive: BorderColor::Solid(0x0000_0000),
            background: BorderColor::Solid(0x0000_0000),
            show_background: false,
            width: 4.0,
            style: BorderStyle::Round,
            order: BorderOrder::Below,
            hidpi: false,
            ax_focus: None,
            blacklist: Vec::new(),
            whitelist: Vec::new(),
        }
    }
}

/// Update bits of one message (spec §2.3): what the platform must redraw or recreate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateMask(pub u8);

impl UpdateMask {
    pub const ACTIVE: u8 = 1 << 0;
    pub const INACTIVE: u8 = 1 << 1;
    pub const ALL: u8 = Self::ACTIVE | Self::INACTIVE;
    pub const RECREATE_ALL: u8 = 1 << 2;
    pub const SETTING: u8 = 1 << 3;

    pub fn contains(self, bits: u8) -> bool {
        self.0 & bits == bits
    }

    pub fn intersects(self, bits: u8) -> bool {
        self.0 & bits != 0
    }
}

impl std::ops::BitOr for UpdateMask {
    type Output = UpdateMask;
    fn bitor(self, rhs: UpdateMask) -> UpdateMask {
        UpdateMask(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for UpdateMask {
    fn bitor_assign(&mut self, rhs: UpdateMask) {
        self.0 |= rhs.0;
    }
}

/// The borders configuration held in the [`crate::Model`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BordersState {
    /// Whether borders are drawn (`drawing=`, extension). Off until configured.
    pub drawing: bool,
    /// A `--borders` message was applied since start / the last `--reload`.
    pub configured: bool,
    pub settings: BorderSettings,
    /// `apply-to=<wid>` overrides, in insertion order.
    pub overrides: Vec<(u32, BorderSettings)>,
}

/// What the platform receives with [`crate::platform::PlatformRequest::SetBorders`]: the
/// complete configuration plus what changed.
#[derive(Debug, Clone, PartialEq)]
pub struct BordersUpdate {
    pub drawing: bool,
    pub settings: BorderSettings,
    pub overrides: Vec<(u32, BorderSettings)>,
    pub mask: UpdateMask,
}
