//! The minimal theme: tokens only (no fonts or images yet, plan 06 slice 5). Dark, neutral greys,
//! one accent; record, mute and solo keep their conventional state colors.

use egui::Color32;

/// Color and size tokens. Everything the studio UI paints takes its colors from here.
pub struct Tokens {
    pub bg: Color32,
    pub panel: Color32,
    pub panel_raised: Color32,
    pub header: Color32,
    pub header_selected: Color32,
    pub lane: Color32,
    pub lane_alt: Color32,
    pub ruler: Color32,
    pub line: Color32,
    pub grid_bar: Color32,
    pub grid_beat: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub cursor: Color32,
    pub record: Color32,
    pub mute: Color32,
    pub solo: Color32,
    pub monitor: Color32,
    pub button: Color32,
    pub button_hover: Color32,
    pub selection_outline: Color32,
}

pub const TOKENS: Tokens = Tokens {
    bg: Color32::from_rgb(0x1b, 0x1c, 0x1e),
    panel: Color32::from_rgb(0x24, 0x25, 0x28),
    panel_raised: Color32::from_rgb(0x2d, 0x2e, 0x32),
    header: Color32::from_rgb(0x2a, 0x2b, 0x2f),
    header_selected: Color32::from_rgb(0x3a, 0x3c, 0x42),
    lane: Color32::from_rgb(0x1f, 0x20, 0x23),
    lane_alt: Color32::from_rgb(0x22, 0x23, 0x26),
    ruler: Color32::from_rgb(0x2b, 0x2c, 0x30),
    line: Color32::from_rgb(0x14, 0x14, 0x16),
    grid_bar: Color32::from_rgb(0x38, 0x39, 0x3e),
    grid_beat: Color32::from_rgb(0x2a, 0x2b, 0x2f),
    text: Color32::from_rgb(0xdc, 0xdd, 0xe0),
    text_dim: Color32::from_rgb(0x8c, 0x8e, 0x94),
    accent: Color32::from_rgb(0x4c, 0xb1, 0xf0),
    cursor: Color32::from_rgb(0xe8, 0xe8, 0xea),
    record: Color32::from_rgb(0xe5, 0x48, 0x48),
    mute: Color32::from_rgb(0xe0, 0xa2, 0x36),
    solo: Color32::from_rgb(0xe6, 0xd2, 0x4a),
    monitor: Color32::from_rgb(0x6f, 0xc2, 0x76),
    button: Color32::from_rgb(0x36, 0x37, 0x3c),
    button_hover: Color32::from_rgb(0x42, 0x44, 0x4a),
    selection_outline: Color32::from_rgb(0xf4, 0xf4, 0xf6),
};

/// Track list row metrics (Cubase-like density).
pub const RULER_H: f32 = 26.0;
pub const TRANSPORT_H: f32 = 40.0;
pub const STATUS_H: f32 = 22.0;
pub const CLIP_NAME_H: f32 = 14.0;

/// Apply the tokens to egui's own widgets (menus, buttons, windows).
pub fn apply(ctx: &egui::Context) {
    let t = &TOKENS;
    let mut v = egui::Visuals::dark();
    v.panel_fill = t.panel;
    v.window_fill = t.panel_raised;
    v.extreme_bg_color = t.bg;
    v.faint_bg_color = t.lane_alt;
    v.selection.bg_fill = t.accent.gamma_multiply(0.6);
    v.selection.stroke = egui::Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent;
    v.widgets.noninteractive.bg_fill = t.panel;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, t.text);
    v.widgets.inactive.bg_fill = t.button;
    v.widgets.inactive.weak_bg_fill = t.button;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, t.text);
    v.widgets.hovered.bg_fill = t.button_hover;
    v.widgets.hovered.weak_bg_fill = t.button_hover;
    v.widgets.active.bg_fill = t.accent.gamma_multiply(0.7);
    v.widgets.active.weak_bg_fill = t.accent.gamma_multiply(0.7);
    ctx.set_visuals(v);
}

/// A readable text color on `bg`: near-black on light clip colors, near-white on dark ones.
pub fn text_on(bg: Color32) -> Color32 {
    let l = 0.2126 * f32::from(bg.r()) + 0.7152 * f32::from(bg.g()) + 0.0722 * f32::from(bg.b());
    if l > 150.0 { Color32::from_rgb(0x14, 0x14, 0x16) } else { Color32::from_rgb(0xf6, 0xf6, 0xf8) }
}

/// `c` mixed toward black by `k` (0..1).
pub fn darken(c: Color32, k: f32) -> Color32 {
    let f = (1.0 - k).clamp(0.0, 1.0);
    Color32::from_rgb((f32::from(c.r()) * f) as u8, (f32::from(c.g()) * f) as u8, (f32::from(c.b()) * f) as u8)
}

/// `c` mixed toward white by `k` (0..1).
pub fn lighten(c: Color32, k: f32) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    let m = |x: u8| (f32::from(x) + (255.0 - f32::from(x)) * k) as u8;
    Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}
