//! Optional background image behind the whole UI. The grey panel colour is layered on top of it:
//! `opacity` is how much of the image shows through (0 = plain grey, 1 = image only).

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Rect, TextureHandle, TextureOptions};
use serde::{Deserialize, Serialize};

use crate::theme;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundPrefs {
    pub path: Option<PathBuf>,
    /// 0.0..=1.0, how visible the image is through the grey.
    pub opacity: f32,
}

impl Default for BackgroundPrefs {
    fn default() -> Self {
        Self { path: None, opacity: 0.25 }
    }
}

/// Decoded image plus the visuals last applied, so we only touch the egui style on change.
#[derive(Default)]
pub struct Background {
    loaded: Option<(PathBuf, TextureHandle)>,
    /// Path we already failed to load (don't retry every frame).
    failed: Option<PathBuf>,
    pub error: Option<String>,
    applied_alpha: Option<f32>,
}

impl Background {
    /// Bring the texture in line with prefs, and make the panel fill see-through when an image is shown.
    pub fn sync(&mut self, ctx: &egui::Context, prefs: &BackgroundPrefs) {
        let want = prefs.path.as_deref();
        if self.loaded.as_ref().map(|(p, _)| p.as_path()) != want {
            self.loaded = None;
        }
        if let Some(p) = want {
            if self.loaded.is_none() && self.failed.as_deref() != Some(p) {
                match load(ctx, p) {
                    Ok(t) => {
                        self.loaded = Some((p.to_path_buf(), t));
                        self.error = None;
                    }
                    Err(e) => {
                        tracing::warn!("background image {}: {e}", p.display());
                        self.error = Some(e);
                        self.failed = Some(p.to_path_buf());
                    }
                }
            }
        } else {
            self.failed = None;
            self.error = None;
        }
        let alpha = if self.loaded.is_some() { 1.0 - prefs.opacity.clamp(0.0, 1.0) } else { 1.0 };
        if self.applied_alpha != Some(alpha) {
            self.applied_alpha = Some(alpha);
            theme::set_overlay_alpha(alpha);
            ctx.all_styles_mut(|s| s.visuals.panel_fill = theme::PANEL.linear_multiply(alpha));
            ctx.request_repaint();
        }
    }

    /// Whether an image is currently shown.
    pub fn active(&self) -> bool {
        self.loaded.is_some()
    }

    /// Give all text a light dark drop shadow so it stays readable on the image without boxes behind it.
    /// Call at the end of the frame, after everything has been painted.
    pub fn outline_text(&self, ctx: &egui::Context) {
        if !self.active() {
            return;
        }
        // A light drop shadow (down and down-right): keeps glyph shapes clean, unlike a full outline.
        const OFFSETS: [(f32, f32); 2] = [(0.0, 1.0), (1.0, 1.0)];
        let halo = Color32::from_black_alpha(110);
        let mut layers: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().collect());
        layers.push(egui::LayerId::background());
        ctx.graphics_mut(|g| {
            for id in layers {
                let Some(list) = g.get_mut(id) else { continue };
                let n = list.all_entries().len();
                for i in 0..n {
                    list.mutate_shape(egui::layers::ShapeIdx(i), |cs| {
                        if let egui::Shape::Text(t) = &cs.shape {
                            let mut shapes: Vec<egui::Shape> = OFFSETS
                                .iter()
                                .map(|(dx, dy)| {
                                    let mut s = t.clone();
                                    s.pos += egui::vec2(*dx, *dy);
                                    s.override_text_color = Some(halo);
                                    egui::Shape::Text(s)
                                })
                                .collect();
                            shapes.push(cs.shape.clone());
                            cs.shape = egui::Shape::Vec(shapes);
                        }
                    });
                }
            }
        });
    }

    /// Paint the image (cover-fit) over the whole window; call before any panel.
    pub fn paint(&self, ui: &egui::Ui) {
        let Some((_, tex)) = &self.loaded else { return };
        let screen = ui.ctx().content_rect();
        let uv = cover_uv(tex.size_vec2(), screen.size());
        ui.painter().rect_filled(screen, 0.0, theme::BG);
        ui.painter().image(tex.id(), screen, uv, Color32::WHITE);
    }
}

pub fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
}

/// UV rectangle that crops the image to the target aspect ratio (CSS `background-size: cover`).
fn cover_uv(img: egui::Vec2, target: egui::Vec2) -> Rect {
    let (ia, ta) = (img.x / img.y, target.x / target.y);
    if !ia.is_finite() || !ta.is_finite() {
        return Rect::from_min_max([0.0, 0.0].into(), [1.0, 1.0].into());
    }
    if ia > ta {
        let w = ta / ia;
        Rect::from_min_max([(1.0 - w) / 2.0, 0.0].into(), [(1.0 + w) / 2.0, 1.0].into())
    } else {
        let h = ia / ta;
        Rect::from_min_max([0.0, (1.0 - h) / 2.0].into(), [1.0, (1.0 + h) / 2.0].into())
    }
}

fn load(ctx: &egui::Context, p: &Path) -> Result<TextureHandle, String> {
    let mut img = image::open(p).map_err(|e| e.to_string())?;
    // Keep GPU memory sane for huge photos.
    const MAX: u32 = 2560;
    if img.width() > MAX || img.height() > MAX {
        img = img.resize(MAX, MAX, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Ok(ctx.load_texture("background", color, TextureOptions::LINEAR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_the_long_side() {
        // Wide image into a square: crop left/right.
        let uv = cover_uv([200.0, 100.0].into(), [100.0, 100.0].into());
        assert_eq!((uv.min.x, uv.max.x, uv.min.y, uv.max.y), (0.25, 0.75, 0.0, 1.0));
        // Tall image into a square: crop top/bottom.
        let uv = cover_uv([100.0, 200.0].into(), [100.0, 100.0].into());
        assert_eq!((uv.min.y, uv.max.y, uv.min.x, uv.max.x), (0.25, 0.75, 0.0, 1.0));
    }

    #[test]
    fn old_prefs_default_opacity() {
        assert_eq!(BackgroundPrefs::default().opacity, 0.25);
        let p: BackgroundPrefs = toml::from_str("path = \"/a.png\"").unwrap();
        assert_eq!(p.opacity, 0.25);
    }
}
