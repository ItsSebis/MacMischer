//! Render `assets/icon/icon.svg` to PNGs (16…1024 px) and `icon-256.rgba` (window icon).

use resvg::{tiny_skia, usvg};
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon");
    let svg = std::fs::read(root.join("icon.svg")).expect("read icon.svg");
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).expect("parse svg");
    for size in [16u32, 32, 64, 128, 256, 512, 1024] {
        let mut pix = tiny_skia::Pixmap::new(size, size).unwrap();
        let s = size as f32 / tree.size().width();
        resvg::render(&tree, tiny_skia::Transform::from_scale(s, s), &mut pix.as_mut());
        pix.save_png(root.join(format!("icon-{size}.png"))).expect("write png");
        if size == 256 {
            // Straight (non-premultiplied) RGBA for egui's IconData.
            let rgba: Vec<u8> = pix
                .pixels()
                .iter()
                .flat_map(|p| {
                    let c = p.demultiply();
                    [c.red(), c.green(), c.blue(), c.alpha()]
                })
                .collect();
            std::fs::write(root.join("icon-256.rgba"), rgba).expect("write rgba");
        }
        println!("icon-{size}.png");
    }
}
