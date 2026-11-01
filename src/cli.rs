//! Command-line modes for rendering without the window. Output goes to the given files;
//! the exit code reports success, since a windows-subsystem exe has no console.
use crate::export::{self, Format, Progress, Settings};
use crate::frame::Frame;
use crate::media::Assets;
use crate::model::*;
use crate::{demo, effects, render};
use std::path::Path;

/// `Some(exit_code)` when the arguments asked for a command-line mode.
pub fn run(args: &[String]) -> Option<i32> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    let result = match arg(1)? {
        "--contact-sheet" => contact_sheet(Path::new(arg(2)?)),
        "--export" => {
            // --export <project.cmproj | demo> <output> [mp4|gif|png]
            let p = if arg(2)? == "demo" { demo::project() } else { crate::app::load_project(Path::new(arg(2)?)).ok()? };
            let format = match arg(4).unwrap_or("mp4") {
                "gif" => Format::Gif,
                "png" => Format::PngFrame,
                _ => Format::Mp4,
            };
            let s = Settings { format, scale: 1.0, fps: p.fps, bitrate: 12.0, audio: true, transparent: false };
            export::run(&p, Path::new(arg(3)?), &s, 1.5, &Assets::default(), &Progress::default())
        }
        _ => return None,
    };
    if let Err(e) = &result {
        eprintln!("{e}");
    }
    Some(if result.is_ok() { 0 } else { 1 })
}

/// A test card that shows off color, alpha and edges, so effects have something to act on.
pub fn sample_layers(p: &mut Project) -> Vec<Layer> {
    let (w, h) = (p.w as f32, p.h as f32);
    let sh = ShapeData { kind: ShapeKind::Star, size: Track::new([h * 0.8, h * 0.8]), radius: Track::new(0.0), sides: Track::new(5.0), inner: Track::new(0.5), path: vec![], closed: true, freehand: false };
    let mut star = Layer::new(p.alloc_id(), "yıldız", Content::Shape(sh), [w / 2.0, h / 2.0], 10.0);
    star.fill.kind = FillKind::Linear;
    star.fill.a = Track::new([1.0, 0.3, 0.45, 1.0]);
    star.fill.b = Track::new([0.25, 0.65, 1.0, 1.0]);
    vec![star]
}

/// Renders every effect on the test card into one labeled image.
fn contact_sheet(out: &Path) -> Result<(), String> {
    let defs: Vec<_> = effects::all().collect();
    let (tw, th, cols) = (160usize, 110usize, 13usize);
    let rows = defs.len().div_ceil(cols);
    let mut sheet = Frame::solid(cols * tw, rows * th, [0.08, 0.08, 0.1, 1.0]);
    let assets = Assets::default();
    for (i, d) in defs.iter().enumerate() {
        let mut p = Project::new("t", (tw - 4) as u32, (th - 4) as u32, 30.0, [0.15, 0.17, 0.22, 1.0]);
        let mut layers = sample_layers(&mut p);
        layers[0].effects.push(effects::instance(d.id));
        // Size parameters are in project pixels, so render a large project scaled down.
        p.w *= 4;
        p.h *= 4;
        for l in &mut layers {
            l.tf.pos = Track::new([p.w as f32 / 2.0, p.h as f32 / 2.0]);
            l.tf.scale = Track::new([4.0, 4.0]);
        }
        p.layers = layers;
        let f = render::render_project(&p, 0.8, 0.25, &assets);
        let (ox, oy) = ((i % cols) * tw + 2, (i / cols) * th + 2);
        for y in 0..f.h.min(th - 4) {
            for x in 0..f.w.min(tw - 4) {
                sheet.px[(oy + y) * sheet.w + ox + x] = f.px[y * f.w + x];
            }
        }
    }
    image::save_buffer(out, &sheet.to_rgba8(), sheet.w as u32, sheet.h as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())
}
