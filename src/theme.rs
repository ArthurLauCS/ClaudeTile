//! Design tokens: the `term` (developer terminal) style from the ui-style catalog.
//! Every color, font, size and radius lives here; drawing code names tokens, never hex.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

/// Type roles (ui-style "Type roles"): UI, DISPLAY and MONO are Sarasa Mono SC (CJK exactly two cells wide, so
/// term columns align); PROSE (session titles, skill summaries) is LXGW WenKai Mono, which keeps the grid.
/// Both ship in the `fonts` folder (scripts/fonts.ps1); a missing file falls back to Cascadia Mono, and CJK
/// glyphs the face lacks go to YaHei UI.
pub const FONT: &str = "Sarasa Mono SC";
pub const FONT_PROSE: &str = "LXGW WenKai Mono";
pub const FONT_FALLBACK: &str = "Cascadia Mono";
pub const FONT_CJK: &str = "Microsoft YaHei UI";
/// File names inside the fonts folder: UI regular, UI bold, PROSE regular.
pub const FONT_FILES: [&str; 3] = ["SarasaMonoSC-Regular.ttf", "SarasaMonoSC-Bold.ttf", "LXGWWenKaiMono-Regular.ttf"];
pub const SIZE_BODY: f32 = 13.0;
pub const SIZE_STATUS: f32 = 12.0;
/// Corners are square in this style.
pub const RADIUS: f32 = 0.0;
/// Row height and the horizontal pane edge.
pub const ROW: f32 = 20.0;
pub const PAD: f32 = 12.0;
pub const STATUS_H: f32 = 24.0;

const fn hex(v: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: ((v >> 16) & 0xFF) as f32 / 255.0, g: ((v >> 8) & 0xFF) as f32 / 255.0, b: (v & 0xFF) as f32 / 255.0, a: 1.0 }
}

pub fn with_a(c: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { a: c.a * a, ..c }
}

#[allow(dead_code)]
pub struct Palette {
    pub bg: D2D1_COLOR_F,
    pub surface: D2D1_COLOR_F,
    pub raised: D2D1_COLOR_F,
    pub border: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub muted: D2D1_COLOR_F,
    pub accent: D2D1_COLOR_F,
    pub on_accent: D2D1_COLOR_F,
    pub red: D2D1_COLOR_F,
    pub orange: D2D1_COLOR_F,
    pub yellow: D2D1_COLOR_F,
    pub green: D2D1_COLOR_F,
    pub blue: D2D1_COLOR_F,
}

/// Base palette from styles.md `term`; accent / on-accent from the terracotta row of "App accents".
pub const LIGHT: Palette = Palette {
    bg: hex(0xF7F7F2),
    surface: hex(0xEEEEE7),
    raised: hex(0xE4E4DA),
    border: hex(0xCBCBBF),
    text: hex(0x1D2329),
    muted: hex(0x5A6670),
    accent: hex(0xAD4F35),
    on_accent: hex(0xFFFFFF),
    red: hex(0xC0262E),
    orange: hex(0xA85300),
    yellow: hex(0x806500),
    green: hex(0x237A2F),
    blue: hex(0x1F5FBF),
};

pub const DARK: Palette = Palette {
    bg: hex(0x0B0F13),
    surface: hex(0x10161C),
    raised: hex(0x17202A),
    border: hex(0x26323E),
    text: hex(0xC9D4DC),
    muted: hex(0x7A8899),
    accent: hex(0xE07F5F),
    on_accent: hex(0x1A1A1A),
    red: hex(0xFF5F6B),
    orange: hex(0xFFB86C),
    yellow: hex(0xF1E05A),
    green: hex(0x50E07B),
    blue: hex(0x6CB6FF),
};

/// Resolved colors for one frame. `selected` (inverse) is text on bg swapped: `inverse_bg` / `inverse_fg`.
pub struct Theme {
    pub dark: bool,
    /// Window fill: `bg` with the user's opacity applied.
    pub tint: D2D1_COLOR_F,
    pub bg: D2D1_COLOR_F,
    pub surface: D2D1_COLOR_F,
    pub raised: D2D1_COLOR_F,
    pub border: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub muted: D2D1_COLOR_F,
    pub accent: D2D1_COLOR_F,
    pub on_accent: D2D1_COLOR_F,
    pub red: D2D1_COLOR_F,
    pub orange: D2D1_COLOR_F,
    pub green: D2D1_COLOR_F,
    pub inverse_bg: D2D1_COLOR_F,
    pub inverse_fg: D2D1_COLOR_F,
    /// Text goes over a thin, see-through fill: add a shadow like desktop icons do.
    pub shadow: bool,
}

impl Theme {
    pub fn new(dark: bool, opacity: f32) -> Theme {
        let p = if dark { &DARK } else { &LIGHT };
        Theme {
            dark,
            tint: with_a(p.bg, opacity),
            bg: p.bg,
            surface: p.surface,
            raised: p.raised,
            border: p.border,
            text: p.text,
            muted: p.muted,
            accent: p.accent,
            on_accent: p.on_accent,
            red: p.red,
            orange: p.orange,
            green: p.green,
            inverse_bg: p.text,
            inverse_fg: p.bg,
            shadow: opacity < 0.5,
        }
    }

    /// Opaque panel for the menu and toasts: `surface`, not tied to the tile's opacity.
    pub fn panel(dark: bool) -> Theme {
        let t = Theme::new(dark, 1.0);
        Theme { tint: t.surface, ..t }
    }

    /// Quota level color: accent below 70 %, orange from 70, red from 90.
    pub fn level(&self, pct: f64) -> D2D1_COLOR_F {
        if pct >= 90.0 {
            self.red
        } else if pct >= 70.0 {
            self.orange
        } else {
            self.accent
        }
    }
}
