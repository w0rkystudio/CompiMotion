//! The effect catalog. Each effect declares its parameters and a function for one pipeline stage.
use crate::frame::Frame;
use crate::model::{EffectInst, Track};

mod anim;
mod color;
mod distort;
mod generate;
mod solid;
mod stylize;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    F(f32, f32, f32),
    C([f32; 4]),
    B(bool),
    Ch(&'static [&'static str], u8),
    P(f32, f32),
}

#[derive(Clone, Copy)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
}
pub const fn f(key: &'static str, label: &'static str, min: f32, max: f32, def: f32) -> Param {
    Param { key, label, kind: Kind::F(min, max, def) }
}
pub const fn c(key: &'static str, label: &'static str, v: [f32; 4]) -> Param {
    Param { key, label, kind: Kind::C(v) }
}
pub const fn b(key: &'static str, label: &'static str, v: bool) -> Param {
    Param { key, label, kind: Kind::B(v) }
}
pub const fn ch(key: &'static str, label: &'static str, o: &'static [&'static str], d: u8) -> Param {
    Param { key, label, kind: Kind::Ch(o, d) }
}
pub const fn p(key: &'static str, label: &'static str, x: f32, y: f32) -> Param {
    Param { key, label, kind: Kind::P(x, y) }
}

/// Everything an effect may need beyond its own parameters.
pub struct Ctx<'a> {
    /// Seconds since the layer started.
    pub t: f32,
    pub dur: f32,
    /// Output pixels per project pixel; size parameters are multiplied by it.
    pub s: f32,
    /// Layer position in output pixels.
    pub c: [f32; 2],
    /// Everything composited beneath this layer.
    pub bg: &'a Frame,
    pub seed: u32,
    pub fps: f32,
    /// The layer's own shape outline in output pixels, when it has one.
    pub path: &'a [[f32; 2]],
}

pub struct V<'a> {
    pub def: &'static Def,
    pub inst: &'a EffectInst,
    pub t: f64,
}
impl V<'_> {
    fn raw(&self, key: &str) -> [f32; 4] {
        if let Some(tr) = self.inst.p.get(key) {
            return tr.at(self.t);
        }
        self.def.params.iter().find(|q| q.key == key).map_or([0.0; 4], |q| default_of(q.kind))
    }
    pub fn f(&self, key: &str) -> f32 {
        self.raw(key)[0]
    }
    pub fn c(&self, key: &str) -> [f32; 4] {
        self.raw(key)
    }
    pub fn rgb(&self, key: &str) -> [f32; 3] {
        let v = self.raw(key);
        [v[0], v[1], v[2]]
    }
    pub fn b(&self, key: &str) -> bool {
        self.raw(key)[0] > 0.5
    }
    pub fn i(&self, key: &str) -> i32 {
        self.raw(key)[0].round() as i32
    }
    pub fn p(&self, key: &str) -> [f32; 2] {
        let v = self.raw(key);
        [v[0], v[1]]
    }
}

pub fn default_of(k: Kind) -> [f32; 4] {
    match k {
        Kind::F(_, _, d) => [d, 0.0, 0.0, 0.0],
        Kind::C(v) => v,
        Kind::B(v) => [v as u8 as f32, 0.0, 0.0, 0.0],
        Kind::Ch(_, d) => [d as f32, 0.0, 0.0, 0.0],
        Kind::P(x, y) => [x, y, 0.0, 0.0],
    }
}

/// Changes to the layer applied before it is rasterized.
#[derive(Clone, Debug)]
pub struct Mods {
    pub dx: f32,
    pub dy: f32,
    pub rot: f32,
    pub sx: f32,
    pub sy: f32,
    pub opacity: f32,
    /// Overrides the layer's animation clock (Time Quantization).
    pub time: Option<f64>,
    /// Echo: (copies, delay seconds, falloff).
    pub echo: Option<(u32, f32, f32)>,
    // Text
    pub text: Option<String>,
    pub text_visible: f32,
    pub text_soft: f32,
    pub tracking: f32,
    pub leading: f32,
    pub glyph: Option<GlyphAnim>,
    pub scramble: f32,
    // Stroke
    pub trim: [f32; 2],
    pub stroke_color: Option<[f32; 4]>,
    pub stroke_end_color: Option<[f32; 4]>,
    pub taper: [f32; 2],
}

/// Per-character animation for Text Transform.
#[derive(Clone, Debug)]
pub struct GlyphAnim {
    pub progress: f32,
    pub spread: f32,
    pub dx: f32,
    pub dy: f32,
    pub rot: f32,
    pub scale: f32,
    pub opacity: f32,
}

impl Default for Mods {
    fn default() -> Self {
        Mods {
            dx: 0.0, dy: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, opacity: 1.0, time: None, echo: None,
            text: None, text_visible: 1.0, text_soft: 0.0, tracking: 0.0, leading: 0.0, glyph: None, scramble: 0.0,
            trim: [0.0, 1.0], stroke_color: None, stroke_end_color: None, taper: [1.0, 1.0],
        }
    }
}

#[derive(Clone, Copy)]
pub enum Run {
    /// Runs on the rendered layer pixels, in order.
    Px(fn(&mut Frame, &V, &Ctx)),
    /// Runs before rasterizing and modifies transform, clock, text or stroke.
    Mod(fn(&mut Mods, &V, &Ctx)),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cat {
    Blur, Color, Light, Distort, Repeat, Stylize, Generate, Solid, Anim, Text, Matte,
}
impl Cat {
    pub const ALL: &'static [(Cat, &'static str)] = &[
        (Cat::Anim, "Hareket"), (Cat::Blur, "Bulanıklık"), (Cat::Color, "Renk"), (Cat::Light, "Işık ve Parıltı"),
        (Cat::Distort, "Bozulma"), (Cat::Repeat, "Tekrar"), (Cat::Stylize, "Stil"), (Cat::Generate, "Üretici"),
        (Cat::Solid, "3B Şekiller"), (Cat::Text, "Metin ve Çizgi"), (Cat::Matte, "Maske ve Anahtar"),
    ];
}

pub struct Def {
    pub id: &'static str,
    pub name: &'static str,
    pub cat: Cat,
    pub params: &'static [Param],
    pub run: Run,
}

pub fn all() -> impl Iterator<Item = &'static Def> {
    [anim::DEFS, color::DEFS, stylize::DEFS, distort::DEFS, generate::DEFS, solid::DEFS].into_iter().flatten()
}
pub fn find(id: &str) -> Option<&'static Def> {
    all().find(|d| d.id == id)
}
pub fn instance(id: &str) -> EffectInst {
    EffectInst { def: id.into(), on: true, p: Default::default() }
}
pub fn track_mut<'a>(inst: &'a mut EffectInst, q: &Param) -> &'a mut Track<[f32; 4]> {
    inst.p.entry(q.key.to_string()).or_insert_with(|| Track::new(default_of(q.kind)))
}

/// Shared helper: compact a `Def` table row.
#[macro_export]
macro_rules! fx {
    ($id:literal, $name:literal, $cat:ident, [$($p:expr),* $(,)?], $run:ident($func:expr)) => {
        $crate::effects::Def { id: $id, name: $name, cat: $crate::effects::Cat::$cat, params: &[$($p),*], run: $crate::effects::Run::$run($func) }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;

    const OFFICIAL: &str = "360-reorient-sphere 360-viewer auto-shake bend blink block-dissolve block-noise box box-blur brightness-contrast bump-map cmyk-halftone-dots channel-remap-hsv channel-remap-rgb checker chroma-key circular-ripple clouds color-temperature color-tune colorize contour-gradient contour-lines contour-strips copy-background count-up-down cube curl cylinder dark-glow directional-blur displacement-map dissolve dots drawing-progress echo-keyframes edge-glow electric-edges ellipsoid exposure-gamma fade-in-out feather fill-behind find-edges flicker flip-layer four-color-gradient fractal-ridges fractal-warp gaussian-blur glass glow glow-scan gradient-map gradient-overlay grid grid-repeat halftone-dots halftone-lines heart hexagon-array hexagon-tile-rotate hexagon-tile-shift hexagon-tiling hexagonal-prism highlights-shadows hollow-box hot-color hue-shift inner-blur inner-glow inner-pinch-bulge invert iridescence kaleidoscope lens-blur lens-flare light-glow lightning linear-repeat linear-streaks long-shadow luma-key magnify-background mask-blur matte-choker mirror mosaic motion-blur move-along-path noise octahedron offset diffusion-plus soft-glass oscillate palette-map pinch-bulge pixelate polar-coordinates polar-displacement-map posterize precise-box-blur pulse-opacity pulse-size pyramid rgb-split radial-rays radial-repeat radial-shadow radial-wipe random-displacement random-jitter raster-extrude rays repeat repeat-along-path replace-color ribbon roughen-edges saturation-vibrance scatter-repeat sharpen simple-starfield smooth-bevel smooth-edges soft-glow solid-color solid-matte spectral-map spherize spin spin-blur spin-streaks spot-color squeeze star star-polyhedron star-prism starfield stretch-axis stretch-segment stripes stroke-color stroke-taper swing swirl text-progress text-randomizer text-spacing text-transform three-axis-cross threshold tile-rotate tile-shift tiles time-quantization timecode torus tunnel turbulence turbulent-displace unsharp-mask vignette voronoi-cells wave-warp wipe zoom-blur zoom-streaks";

    #[test]
    fn catalog_contains_all_169_effects() {
        let want: Vec<&str> = OFFICIAL.split(' ').collect();
        assert_eq!(want.len(), 169);
        let missing: Vec<_> = want.iter().filter(|id| find(id).is_none()).collect();
        assert!(missing.is_empty(), "eksik: {missing:?}");
        let extra: Vec<_> = all().filter(|d| !want.contains(&d.id)).map(|d| d.id).collect();
        assert!(extra.is_empty(), "fazla: {extra:?}");
    }

    fn sample_frame() -> Frame {
        let mut fr = Frame::new(64, 48);
        fr.shade(|x, y, _| {
            let inside = (x - 32.0).abs() < 18.0 && (y - 24.0).abs() < 14.0;
            if inside { [x / 64.0, y / 48.0, 0.4, 1.0] } else { [0.0; 4] }
        });
        fr
    }

    /// Every pixel effect runs at defaults and at parameter extremes without NaNs or panics.
    #[test]
    fn every_effect_stays_finite() {
        let bg = Frame::solid(64, 48, [0.2, 0.3, 0.5, 1.0]);
        let path = [[10.0, 10.0], [50.0, 15.0], [30.0, 40.0]];
        for def in all() {
            for mode in 0..3 {
                let mut inst = instance(def.id);
                for q in def.params {
                    let val = match (q.kind, mode) {
                        (Kind::F(min, _, _), 1) => [min, 0.0, 0.0, 0.0],
                        (Kind::F(_, max, _), 2) => [max.min(400.0), 0.0, 0.0, 0.0],
                        (k, _) => default_of(k),
                    };
                    inst.p.insert(q.key.into(), Track::new(val));
                }
                let v = V { def, inst: &inst, t: 0.5 };
                let ctx = Ctx { t: 0.5, dur: 2.0, s: 0.25, c: [32.0, 24.0], bg: &bg, seed: 7, fps: 30.0, path: &path };
                match def.run {
                    Run::Px(run) => {
                        let mut fr = sample_frame();
                        run(&mut fr, &v, &ctx);
                        assert_eq!(fr.px.len(), 64 * 48, "{}", def.id);
                        assert!(fr.px.iter().flatten().all(|c| c.is_finite()), "{} mod {mode}", def.id);
                    }
                    Run::Mod(run) => {
                        let mut m = Mods::default();
                        run(&mut m, &v, &ctx);
                        assert!(m.dx.is_finite() && m.dy.is_finite() && m.opacity.is_finite(), "{}", def.id);
                    }
                }
            }
        }
    }

    /// Guards against silent no-ops: pixel effects must change the test image at their defaults.
    #[test]
    fn pixel_effects_change_the_image() {
        // These are intentionally neutral until the user moves a parameter or a key color matches.
        let neutral = ["channel-remap-rgb", "channel-remap-hsv", "brightness-contrast", "color-tune", "360-reorient-sphere", "offset", "chroma-key", "wipe", "radial-wipe", "bend", "highlights-shadows", "fill-behind", "copy-background"];
        let bg = Frame::solid(64, 48, [0.2, 0.3, 0.5, 1.0]);
        let path = [[10.0, 10.0], [50.0, 15.0], [30.0, 40.0]];
        let mut dead = vec![];
        for def in all() {
            let Run::Px(run) = def.run else { continue };
            if neutral.contains(&def.id) {
                continue;
            }
            let inst = instance(def.id);
            let v = V { def, inst: &inst, t: 0.5 };
            // Defaults are sized for full-size projects, so also try a small render scale,
            // where spacings and offsets fit inside this tiny test frame.
            let changed = [1.0, 0.1].iter().any(|&s| {
                let ctx = Ctx { t: 0.5, dur: 2.0, s, c: [32.0, 24.0], bg: &bg, seed: 7, fps: 30.0, path: &path };
                let orig = sample_frame();
                let mut fr = orig.clone();
                run(&mut fr, &v, &ctx);
                let d: f32 = fr.px.iter().flatten().zip(orig.px.iter().flatten()).map(|(a, b)| (a - b).abs()).sum();
                d >= 0.01
            });
            if !changed {
                dead.push(def.id);
            }
        }
        assert!(dead.is_empty(), "etkisiz: {dead:?}");
    }
}
