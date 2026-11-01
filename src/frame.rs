//! Premultiplied RGBA float frame, sampling, blur and blending primitives.
use crate::model::Blend;
use rayon::prelude::*;

pub type Px = [f32; 4];

#[derive(Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Px>,
}

pub fn unpremul(p: Px) -> [f32; 3] {
    if p[3] > 1e-5 { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] } else { [0.0; 3] }
}
pub fn premul(c: [f32; 3], a: f32) -> Px {
    [c[0] * a, c[1] * a, c[2] * a, a]
}
pub fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}
pub fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
pub fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
pub fn mix4(a: Px, b: Px, t: f32) -> Px {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}
pub fn over(top: Px, bot: Px) -> Px {
    let k = 1.0 - top[3];
    std::array::from_fn(|i| top[i] + bot[i] * k)
}

pub fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    let d = mx - mn;
    let h = if d < 1e-6 {
        0.0
    } else if mx == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if mx == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    [h / 6.0, if mx > 1e-6 { d / mx } else { 0.0 }, mx]
}
pub fn hsv_to_rgb(c: [f32; 3]) -> [f32; 3] {
    let h = c[0].rem_euclid(1.0) * 6.0;
    let f = |n: f32| {
        let k = (n + h).rem_euclid(6.0);
        c[2] - c[2] * c[1] * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

impl Frame {
    pub fn new(w: usize, h: usize) -> Self {
        Frame { w, h, px: vec![[0.0; 4]; w * h] }
    }
    pub fn solid(w: usize, h: usize, c: Px) -> Self {
        Frame { w, h, px: vec![c; w * h] }
    }
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Px {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            [0.0; 4]
        } else {
            self.px[y as usize * self.w + x as usize]
        }
    }
    #[inline]
    pub fn get_clamp(&self, x: i32, y: i32) -> Px {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        self.px[y * self.w + x]
    }
    /// Bilinear sample in pixel coordinates (pixel centers at +0.5); transparent outside.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> Px {
        let (x, y) = (x - 0.5, y - 0.5);
        let (fx, fy) = (x.floor(), y.floor());
        let (tx, ty) = (x - fx, y - fy);
        let (ix, iy) = (fx as i32, fy as i32);
        let a = mix4(self.get(ix, iy), self.get(ix + 1, iy), tx);
        let b = mix4(self.get(ix, iy + 1), self.get(ix + 1, iy + 1), tx);
        mix4(a, b, ty)
    }
    #[inline]
    pub fn sample_clamp(&self, x: f32, y: f32) -> Px {
        let (x, y) = (x - 0.5, y - 0.5);
        let (fx, fy) = (x.floor(), y.floor());
        let (tx, ty) = (x - fx, y - fy);
        let (ix, iy) = (fx as i32, fy as i32);
        let a = mix4(self.get_clamp(ix, iy), self.get_clamp(ix + 1, iy), tx);
        let b = mix4(self.get_clamp(ix, iy + 1), self.get_clamp(ix + 1, iy + 1), tx);
        mix4(a, b, ty)
    }
    #[inline]
    pub fn sample_wrap(&self, x: f32, y: f32) -> Px {
        self.sample_clamp(x.rem_euclid(self.w as f32), y.rem_euclid(self.h as f32))
    }
    /// Rewrites every pixel in parallel; `f` gets pixel-center coordinates and the old value.
    pub fn shade(&mut self, f: impl Fn(f32, f32, Px) -> Px + Sync) {
        let w = self.w.max(1);
        self.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                *p = f(x as f32 + 0.5, y as f32 + 0.5, *p);
            }
        });
    }
    /// Per-pixel color change on straight RGB, leaving alpha unchanged.
    pub fn map_rgb(&mut self, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
        self.shade(|_, _, p| if p[3] <= 1e-5 { p } else { premul(f(unpremul(p)), p[3]) });
    }
    /// Pulls each output pixel from `f(x, y)` in a copy of the frame.
    pub fn warp(&mut self, f: impl Fn(f32, f32) -> (f32, f32) + Sync) {
        let src = self.clone();
        self.shade(|x, y, _| {
            // Clamping extends a full-frame layer past the edge instead of opening black gaps;
            // a layer that does not reach the edge has transparent edge pixels, so nothing changes.
            let (u, v) = f(x, y);
            src.sample_clamp(u, v)
        });
    }
    fn transposed(&self) -> Frame {
        let mut o = Frame::new(self.h, self.w);
        let (w, h) = (self.w, self.h);
        o.px.par_chunks_mut(h.max(1)).enumerate().for_each(|(x, col)| {
            for (y, p) in col.iter_mut().enumerate() {
                *p = self.px[y * w + x];
            }
        });
        o
    }
    fn box_rows(&mut self, r: usize) {
        if r == 0 {
            return;
        }
        let w = self.w;
        let norm = 1.0 / (2 * r + 1) as f32;
        self.px.par_chunks_mut(w.max(1)).for_each(|row| {
            let src = row.to_vec();
            // Edge pixels repeat, so full-frame layers do not darken at the border when blurred.
            let at = |i: i64| -> Px { src[i.clamp(0, w as i64 - 1) as usize] };
            let mut sum = [0.0f32; 4];
            for i in -(r as i64)..=(r as i64) {
                let p = at(i);
                for c in 0..4 {
                    sum[c] += p[c];
                }
            }
            for x in 0..w as i64 {
                row[x as usize] = sum.map(|v| (v * norm).max(0.0));
                let (a, b) = (at(x + r as i64 + 1), at(x - r as i64));
                for c in 0..4 {
                    sum[c] += a[c] - b[c];
                }
            }
        });
    }
    pub fn box_blur(&mut self, rx: f32, ry: f32) {
        self.box_rows(rx.max(0.0).round() as usize);
        let r = ry.max(0.0).round() as usize;
        if r > 0 {
            let mut t = self.transposed();
            t.box_rows(r);
            *self = t.transposed();
        }
    }
    /// Gaussian approximated by three box passes.
    pub fn gauss(&mut self, sx: f32, sy: f32) {
        for _ in 0..3 {
            self.box_blur(sx * 0.58, sy * 0.58);
        }
    }
    /// Averages samples along an offset function; `f(i_fraction, x, y)` gives the sample position.
    pub fn smear(&mut self, n: usize, f: impl Fn(f32, f32, f32) -> (f32, f32) + Sync) {
        let src = self.clone();
        let n = n.clamp(1, 96);
        let k = 1.0 / n as f32;
        self.shade(|x, y, _| {
            let mut s = [0.0; 4];
            for i in 0..n {
                let (u, v) = f((i as f32 + 0.5) * k - 0.5, x, y);
                let p = src.sample_clamp(u, v);
                for c in 0..4 {
                    s[c] += p[c] * k;
                }
            }
            s
        });
    }
    pub fn add_over(&mut self, top: &Frame) {
        self.px.par_iter_mut().zip(top.px.par_iter()).for_each(|(d, s)| *d = over(*s, *d));
    }
    pub fn add_under(&mut self, bot: &Frame) {
        self.px.par_iter_mut().zip(bot.px.par_iter()).for_each(|(d, s)| *d = over(*d, *s));
    }
    pub fn blend_from(&mut self, src: &Frame, mode: Blend, opacity: f32) {
        self.px.par_iter_mut().zip(src.px.par_iter()).for_each(|(d, s)| *d = blend_px(*d, *s, mode, opacity));
    }
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.px.len() * 4];
        out.par_chunks_mut(4).zip(self.px.par_iter()).for_each(|(o, p)| {
            let c = unpremul(*p);
            for i in 0..3 {
                o[i] = (c[i] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
            o[3] = (p[3] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        });
        out
    }
    pub fn from_rgba8(w: usize, h: usize, d: &[u8]) -> Frame {
        let px = d
            .par_chunks(4)
            .map(|b| {
                let a = b[3] as f32 / 255.0;
                [b[0] as f32 / 255.0 * a, b[1] as f32 / 255.0 * a, b[2] as f32 / 255.0 * a, a]
            })
            .collect();
        Frame { w, h, px }
    }
}

fn sep(m: Blend, b: f32, s: f32) -> f32 {
    use Blend::*;
    match m {
        Add => b + s,
        Subtract => b - s,
        Multiply => b * s,
        Screen => b + s - b * s,
        Overlay => sep(HardLight, s, b),
        Darken => b.min(s),
        Lighten => b.max(s),
        ColorDodge => {
            if b <= 0.0 { 0.0 } else if s >= 1.0 { 1.0 } else { (b / (1.0 - s)).min(1.0) }
        }
        ColorBurn => {
            if b >= 1.0 { 1.0 } else if s <= 0.0 { 0.0 } else { 1.0 - ((1.0 - b) / s).min(1.0) }
        }
        LinearBurn => b + s - 1.0,
        HardLight => {
            if s <= 0.5 { b * 2.0 * s } else { sep(Screen, b, 2.0 * s - 1.0) }
        }
        SoftLight => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.max(0.0).sqrt() };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }
        VividLight => {
            if s <= 0.5 { sep(ColorBurn, b, 2.0 * s) } else { sep(ColorDodge, b, 2.0 * s - 1.0) }
        }
        LinearLight => b + 2.0 * s - 1.0,
        PinLight => {
            if s <= 0.5 { b.min(2.0 * s) } else { b.max(2.0 * s - 1.0) }
        }
        HardMix => {
            if b + s >= 1.0 { 1.0 } else { 0.0 }
        }
        Difference => (b - s).abs(),
        Exclusion => b + s - 2.0 * b * s,
        Divide => {
            if s <= 1e-4 { 1.0 } else { (b / s).min(1.0) }
        }
        _ => s,
    }
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - luma(c);
    let c = [c[0] + d, c[1] + d, c[2] + d];
    let (l, n, x) = (luma(c), c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
    let mut o = c;
    if n < 0.0 {
        o = o.map(|v| l + (v - l) * l / (l - n).max(1e-5));
    }
    if x > 1.0 {
        o = o.map(|v| l + (v - l) * (1.0 - l) / (x - l).max(1e-5));
    }
    o
}
fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let (n, x) = (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
    if x - n < 1e-6 { [0.0; 3] } else { c.map(|v| (v - n) * s / (x - n)) }
}

/// Composites premultiplied `s` onto premultiplied `d`.
pub fn blend_px(d: Px, s: Px, m: Blend, op: f32) -> Px {
    use Blend::*;
    let s = s.map(|v| v * op);
    match m {
        Normal => return over(s, d),
        AlphaIn => return d.map(|v| v * s[3]),
        AlphaOut => return d.map(|v| v * (1.0 - s[3])),
        Atop => {
            let k = 1.0 - s[3];
            return std::array::from_fn(|i| if i == 3 { d[3] } else { s[i] * d[3] + d[i] * k });
        }
        Behind => return over(d, s),
        Xor => return std::array::from_fn(|i| s[i] * (1.0 - d[3]) + d[i] * (1.0 - s[3])),
        _ => {}
    }
    if s[3] <= 1e-5 {
        return d;
    }
    let (cb, cs) = (unpremul(d), unpremul(s));
    let mixed: [f32; 3] = match m {
        Hue => set_lum(set_sat(cs, sat(cb)), luma(cb)),
        Saturation => set_lum(set_sat(cb, sat(cs)), luma(cb)),
        Color => set_lum(cs, luma(cb)),
        Luminosity => set_lum(cb, luma(cs)),
        DarkerColor => if luma(cs) < luma(cb) { cs } else { cb },
        LighterColor => if luma(cs) > luma(cb) { cs } else { cb },
        _ => std::array::from_fn(|i| sep(m, cb[i], cs[i]).clamp(0.0, 1.0)),
    };
    let a = s[3] + d[3] * (1.0 - s[3]);
    let col = std::array::from_fn(|i| {
        // Standard compositing: the blend only applies where the backdrop is present.
        s[i] * (1.0 - d[3]) + d[i] * (1.0 - s[3]) + s[3] * d[3] * mixed[i]
    });
    let c: [f32; 3] = col;
    [c[0], c[1], c[2], a]
}

// ---- Procedural noise ----

#[inline]
pub fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6b343) ^ (y as u32).wrapping_mul(0xd8163841) ^ seed.wrapping_mul(0xcb1ab31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1e995);
    h ^= h >> 15;
    (h & 0xffffff) as f32 / 16777215.0
}
pub fn hash1(x: i32, seed: u32) -> f32 {
    hash2(x, 91, seed)
}
pub fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i32, fy as i32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (tx, ty) = (s(x - fx), s(y - fy));
    let a = hash2(ix, iy, seed) + (hash2(ix + 1, iy, seed) - hash2(ix, iy, seed)) * tx;
    let b = hash2(ix, iy + 1, seed) + (hash2(ix + 1, iy + 1, seed) - hash2(ix, iy + 1, seed)) * tx;
    a + (b - a) * ty
}
pub fn fbm(x: f32, y: f32, oct: u32, seed: u32) -> f32 {
    let (mut v, mut a, mut f, mut n) = (0.0, 0.5, 1.0, 0.0);
    for i in 0..oct {
        v += vnoise(x * f, y * f, seed + i) * a;
        n += a;
        a *= 0.5;
        f *= 2.03;
    }
    v / n.max(1e-5)
}
/// Smoothly animated 1-D noise in [-1, 1], used by the wiggle-style effects.
pub fn wiggle(t: f32, seed: u32) -> f32 {
    let f = t.floor();
    let u = t - f;
    let u = u * u * (3.0 - 2.0 * u);
    let (a, b) = (hash1(f as i32, seed), hash1(f as i32 + 1, seed));
    (a + (b - a) * u) * 2.0 - 1.0
}
