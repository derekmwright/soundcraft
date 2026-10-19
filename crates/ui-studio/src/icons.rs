//! UI icons: Lucide SVGs (ISC, `assets/icons/`, see ATTRIBUTION.md), rasterized once per icon and
//! pixel size by egui_extras (resvg) and cached as textures, then tinted with theme tokens.
//!
//! Lucide draws strokes in `currentColor`; we rasterize them in white so a texture tint gives the
//! token color. Transport glyphs and the favorite star are filled (DAW convention); everything
//! else is Lucide's outline. Letters stay letters where they read better (M, S, R, I, Pre/Post).

use egui::{Color32, Pos2, Rect, Response, Sense, Ui, pos2, vec2};
use std::collections::HashMap;

/// Icon size on the type scale at 1x (points).
pub const ICON: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    /// Return to zero (rewind).
    Rewind,
    Stop,
    Play,
    Record,
    Loop,
    ChevronRight,
    ChevronDown,
    Plus,
    Remove,
    Power,
    OpenEditor,
    Star,
    StarFilled,
    Filter,
    Folder,
    FolderOpen,
    FileAudio,
    File,
}

impl Icon {
    pub const ALL: [Icon; 18] = [
        Icon::Rewind,
        Icon::Stop,
        Icon::Play,
        Icon::Record,
        Icon::Loop,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::Plus,
        Icon::Remove,
        Icon::Power,
        Icon::OpenEditor,
        Icon::Star,
        Icon::StarFilled,
        Icon::Filter,
        Icon::Folder,
        Icon::FolderOpen,
        Icon::FileAudio,
        Icon::File,
    ];

    /// The Lucide name (the file under `assets/icons/`), plus `-filled` for filled variants.
    pub fn name(self) -> &'static str {
        match self {
            Icon::Rewind => "skip-back",
            Icon::Stop => "square",
            Icon::Play => "play",
            Icon::Record => "circle",
            Icon::Loop => "repeat",
            Icon::ChevronRight => "chevron-right",
            Icon::ChevronDown => "chevron-down",
            Icon::Plus => "plus",
            Icon::Remove => "x",
            Icon::Power => "power",
            Icon::OpenEditor => "external-link",
            Icon::Star => "star",
            Icon::StarFilled => "star-filled",
            Icon::Filter => "funnel",
            Icon::Folder => "folder",
            Icon::FolderOpen => "folder-open",
            Icon::FileAudio => "file-audio",
            Icon::File => "file",
        }
    }

    fn source(self) -> &'static [u8] {
        match self {
            Icon::Rewind => include_bytes!("../assets/icons/skip-back.svg"),
            Icon::Stop => include_bytes!("../assets/icons/square.svg"),
            Icon::Play => include_bytes!("../assets/icons/play.svg"),
            Icon::Record => include_bytes!("../assets/icons/circle.svg"),
            Icon::Loop => include_bytes!("../assets/icons/repeat.svg"),
            Icon::ChevronRight => include_bytes!("../assets/icons/chevron-right.svg"),
            Icon::ChevronDown => include_bytes!("../assets/icons/chevron-down.svg"),
            Icon::Plus => include_bytes!("../assets/icons/plus.svg"),
            Icon::Remove => include_bytes!("../assets/icons/x.svg"),
            Icon::Power => include_bytes!("../assets/icons/power.svg"),
            Icon::OpenEditor => include_bytes!("../assets/icons/external-link.svg"),
            Icon::Star | Icon::StarFilled => include_bytes!("../assets/icons/star.svg"),
            Icon::Filter => include_bytes!("../assets/icons/funnel.svg"),
            Icon::Folder => include_bytes!("../assets/icons/folder.svg"),
            Icon::FolderOpen => include_bytes!("../assets/icons/folder-open.svg"),
            Icon::FileAudio => include_bytes!("../assets/icons/file-audio.svg"),
            Icon::File => include_bytes!("../assets/icons/file.svg"),
        }
    }

    fn filled(self) -> bool {
        matches!(self, Icon::Rewind | Icon::Stop | Icon::Play | Icon::Record | Icon::StarFilled)
    }

    /// The SVG with white ink (and white fill for filled variants), ready to tint.
    fn svg(self) -> String {
        let s = String::from_utf8_lossy(self.source()).replace("currentColor", "#ffffff");
        if self.filled() { s.replace("fill=\"none\"", "fill=\"#ffffff\"") } else { s }
    }
}

/// Textures by (icon, pixel size), and how many rasterizations ran (tests check it stays flat).
#[derive(Default)]
struct Cache {
    textures: HashMap<(Icon, u32), egui::TextureHandle>,
    rasterized: usize,
}

/// The cache lives behind an `Arc`, so reading it from the context's data store clones a pointer,
/// never the map (a paint per icon per frame).
type Shared = std::sync::Arc<std::sync::Mutex<Cache>>;

fn cache_id() -> egui::Id {
    egui::Id::new("transit-icon-cache")
}

fn shared(ctx: &egui::Context) -> Shared {
    if let Some(c) = ctx.data(|d| d.get_temp::<Shared>(cache_id())) {
        return c;
    }
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with::<Shared>(cache_id(), Shared::default).clone())
}

/// The icon's texture at `px` pixels, rasterized on first use only.
fn texture(ctx: &egui::Context, icon: Icon, px: u32) -> Option<egui::TextureHandle> {
    let cache = shared(ctx);
    if let Some(t) = cache.lock().ok().and_then(|c| c.textures.get(&(icon, px)).cloned()) {
        return Some(t);
    }
    let hint = egui::SizeHint::Size { width: px, height: px, maintain_aspect_ratio: true };
    let image = match egui_extras::image::load_svg_bytes_with_size(icon.svg().as_bytes(), hint, &resvg::usvg::Options::default()) {
        Ok(i) => i,
        Err(e) => {
            log::warn!("icon {}: {e}", icon.name());
            return None;
        }
    };
    let tex = ctx.load_texture(format!("icon-{}-{px}", icon.name()), image, egui::TextureOptions::LINEAR);
    if let Ok(mut c) = cache.lock() {
        c.textures.insert((icon, px), tex.clone());
        c.rasterized += 1;
    }
    Some(tex)
}

/// How many icon rasterizations have run on this context.
pub fn rasterized(ctx: &egui::Context) -> usize {
    shared(ctx).lock().map_or(0, |c| c.rasterized)
}

/// Paint `icon` centered on `center`, `size` points square, in `color`, snapped to the pixel grid.
pub fn paint(p: &egui::Painter, center: Pos2, icon: Icon, size: f32, color: Color32) {
    let ppp = p.ctx().pixels_per_point();
    let px = (size * ppp).round().max(1.0) as u32;
    let Some(tex) = texture(p.ctx(), icon, px) else { return };
    let side = px as f32 / ppp;
    let min = ((center - vec2(side, side) / 2.0) * ppp).round() / ppp;
    let r = Rect::from_min_size(pos2(min.x, min.y), vec2(side, side));
    p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
}

/// Paint `icon` at [`ICON`] size in the middle of `r`.
pub fn paint_in(p: &egui::Painter, r: Rect, icon: Icon, color: Color32) {
    paint(p, r.center(), icon, ICON.min(r.width()).min(r.height()), color);
}

/// The shared helper: an icon on the type scale, in the current text color.
pub fn icon(ui: &mut Ui, icon: Icon) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ICON, ICON), Sense::hover());
    let color = ui.visuals().text_color();
    paint_in(ui.painter(), r, icon, color);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterizes_and_is_cached() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            for i in Icon::ALL {
                assert!(texture(ui.ctx(), i, 16).is_some(), "{}", i.name());
            }
            for i in Icon::ALL {
                let _ = texture(ui.ctx(), i, 16);
            }
        });
        out.textures_delta.clear();
        assert_eq!(rasterized(&ctx), Icon::ALL.len(), "the second pass parsed nothing");
    }

    #[test]
    fn filled_icons_are_white_inside() {
        // The middle pixel of a filled Play is ink; of an outlined Star it is empty.
        let px = |i: Icon| {
            let hint = egui::SizeHint::Size { width: 32, height: 32, maintain_aspect_ratio: true };
            let img = egui_extras::image::load_svg_bytes_with_size(i.svg().as_bytes(), hint, &resvg::usvg::Options::default()).unwrap();
            img.pixels[16 * 32 + 15]
        };
        assert!(px(Icon::Play).a() > 200);
        assert!(px(Icon::StarFilled).a() > 200);
        assert_eq!(px(Icon::Star).a(), 0);
    }
}
