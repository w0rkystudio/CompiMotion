//! Procedural patterns. They paint inside the layer's alpha unless "Tüm kare" is on.
use super::*;
use crate::frame::*;
use crate::fx;
use std::f32::consts::TAU;

/// Applies a straight-color pattern: `f` returns RGBA with coverage in the last channel.
fn paint(fr: &mut Frame, v: &V, f: impl Fn(f32, f32) -> Px + Sync) {
    let (full, k) = (v.b("full"), v.f("mix"));
    fr.shade(|x, y, p| {
        let q = f(x, y);
        let col = premul([q[0], q[1], q[2]], q[3]);
        if full {
            mix4(p, over(col, p), k)
        } else {
            mix4(p, over(col, p).map(|n| n * if p[3] > 0.0 { 1.0 } else { 0.0 }), k)
        }
    });
}

const FULL: Param = b("full", "Tüm kare", false);
const MIX: Param = f("mix", "Karışım", 0.0, 1.0, 1.0);

fn star_layer(fr: &mut Frame, x: &Ctx, n: i32, speed: f32, size: f32, col: [f32; 3], warp: f32, sd: u32) {
    let (w, h) = (fr.w as f32, fr.h as f32);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let mut stars = Vec::with_capacity(n as usize);
    for i in 0..n {
        let (a, r) = (hash2(i, 1, sd) * TAU, hash2(i, 2, sd));
        let z = (hash2(i, 3, sd) + x.t * speed).fract();
        // Perspective: stars accelerate and brighten as they near the viewer.
        let d = r * z * z * w.hypot(h) * 0.7;
        stars.push((cx + a.cos() * d, cy + a.sin() * d, a, z));
    }
    let (fw, fh) = (fr.w, fr.h);
    for (sx, sy, a, z) in stars {
        let rad = (size * z * x.s).max(0.5);
        let streak = warp * z * 40.0 * x.s;
        let steps = streak as i32 + 1;
        for t in 0..steps {
            let (qx, qy) = (sx - a.cos() * t as f32, sy - a.sin() * t as f32);
            let b = z * (1.0 - t as f32 / steps as f32);
            let r = rad.ceil() as i32;
            for dy in -r..=r { for dx in -r..=r {
                let (px, py) = (qx as i32 + dx, qy as i32 + dy);
                if px < 0 || py < 0 || px >= fw as i32 || py >= fh as i32 { continue; }
                let e = ((dx * dx + dy * dy) as f32).sqrt() / rad;
                if e > 1.0 { continue; }
                let m = (1.0 - e) * b;
                let o = &mut fr.px[py as usize * fw + px as usize];
                for c in 0..3 { o[c] = (o[c] + col[c] * m).min(1.0); }
                o[3] = (o[3] + m).min(1.0);
            } }
        }
    }
}

pub const DEFS: &[Def] = &[
    fx!("block-noise", "Block Noise", Generate, [f("size", "Boyut", 1.0, 999.0, 20.0), p("stretch", "Esnet", 0.0, 0.0), f("str", "Güç", 0.0, 1.0, 0.25), b("mono", "Tek renk", false), b("freeze", "Dondur", false), b("ext", "Uç değerler", false), p("off", "Ofset", 0.0, 0.0), f("over", "Taşma", 0.5, 2.0, 2.0), f("seed", "Tohum", 0.0, 5.0, 0.0)], Px(|fr, v, x| {
        let (sz, st, a, m, ex, of, ov) = ((v.f("size") * x.s).max(1.0), v.p("stretch"), v.f("str"), v.b("mono"), v.b("ext"), v.p("off"), v.f("over"));
        let sd = v.f("seed") as u32 * 37 + if v.b("freeze") { 0 } else { (x.t * x.fps) as u32 };
        let (bw, bh) = (sz * (1.0 + st[0].max(0.0)), sz * (1.0 + st[1].max(0.0)));
        fr.shade(|px, py, p| {
            let (ix, iy) = (((px + of[0] * x.s) / bw).floor() as i32, ((py + of[1] * x.s) / bh).floor() as i32);
            let n = |k: u32| {
                let r = hash2(ix, iy, sd + k);
                if ex { if r > 0.5 { 1.0 } else { 0.0 } } else { r }
            };
            let t = if m { [n(0); 3] } else { [n(0), n(1), n(2)] };
            std::array::from_fn(|i| if i == 3 { p[3] } else { (p[i] + (t[i] - 0.5) * a * ov * p[3]).clamp(0.0, p[3]) })
        });
    })),
    fx!("checker", "Checker", Generate, [f("size", "Boyut", 1.0, 1000.0, 60.0), c("a", "Renk 1", [1.0, 1.0, 1.0, 1.0]), c("b", "Renk 2", [0.0, 0.0, 0.0, 1.0]), f("rot", "Dönme", -360.0, 360.0, 0.0), FULL, MIX], Px(|fr, v, x| {
        let (z, a, bb, r) = ((v.f("size") * x.s).max(1.0), v.c("a"), v.c("b"), v.f("rot").to_radians());
        let (sn, cs) = r.sin_cos();
        paint(fr, v, |px, py| {
            let (u, q) = ((px * cs + py * sn) / z, (-px * sn + py * cs) / z);
            if (u.floor() as i32 + q.floor() as i32) & 1 == 0 { a } else { bb }
        });
    })),
    fx!("clouds", "Clouds", Generate, [f("size", "Boyut", 5.0, 2000.0, 300.0), f("oct", "Detay", 1.0, 8.0, 5.0), f("speed", "Hız", -3.0, 3.0, 0.1), c("a", "Gökyüzü", [0.2, 0.45, 0.85, 1.0]), c("b", "Bulut", [1.0, 1.0, 1.0, 1.0]), f("cover", "Kapsama", 0.0, 1.0, 0.5), FULL, MIX], Px(|fr, v, x| {
        let (z, o, ph, a, bb, cv) = ((v.f("size") * x.s).max(1.0), v.i("oct") as u32, x.t * v.f("speed"), v.c("a"), v.c("b"), v.f("cover"));
        paint(fr, v, |px, py| {
            let n = smooth(1.0 - cv - 0.2, 1.0 - cv + 0.3, fbm(px / z + ph, py / z, o, 3));
            mix4(a, bb, n)
        });
    })),
    fx!("dots", "Dots", Generate, [f("size", "Aralık", 2.0, 1000.0, 40.0), f("r", "Nokta oranı", 0.01, 1.0, 0.4), c("c", "Renk", [1.0, 1.0, 1.0, 1.0]), b("stag", "Şaşırtmalı", true), FULL, MIX], Px(|fr, v, x| {
        let (z, r, k, st) = ((v.f("size") * x.s).max(2.0), v.f("r"), v.c("c"), v.b("stag"));
        paint(fr, v, |px, py| {
            let row = (py / z).floor();
            let sh = if st && (row as i32) & 1 == 1 { 0.5 } else { 0.0 };
            let (u, q) = ((px / z + sh).fract() - 0.5, (py / z).fract() - 0.5);
            let m = 1.0 - smooth(r * 0.5 - 0.02, r * 0.5 + 0.02, u.hypot(q));
            [k[0], k[1], k[2], k[3] * m]
        });
    })),
    fx!("four-color-gradient", "Four-color Gradient", Generate, [c("a", "Sol üst", [1.0, 0.25, 0.4, 1.0]), c("b", "Sağ üst", [1.0, 0.8, 0.2, 1.0]), c("c", "Sol alt", [0.2, 0.5, 1.0, 1.0]), c("d", "Sağ alt", [0.3, 0.95, 0.6, 1.0]), FULL, MIX], Px(|fr, v, _| {
        let (a, bb, cc, d) = (v.c("a"), v.c("b"), v.c("c"), v.c("d"));
        let (w, h) = (fr.w as f32, fr.h as f32);
        paint(fr, v, |x, y| mix4(mix4(a, bb, x / w), mix4(cc, d, x / w), y / h));
    })),
    fx!("fractal-ridges", "Fractal Ridges", Generate, [f("size", "Boyut", 5.0, 2000.0, 200.0), f("oct", "Detay", 1.0, 8.0, 5.0), f("speed", "Hız", -3.0, 3.0, 0.1), c("a", "Koyu", [0.05, 0.05, 0.1, 1.0]), c("b", "Açık", [0.8, 0.9, 1.0, 1.0]), FULL, MIX], Px(|fr, v, x| {
        let (z, o, ph, a, bb) = ((v.f("size") * x.s).max(1.0), v.i("oct").max(1), x.t * v.f("speed"), v.c("a"), v.c("b"));
        paint(fr, v, |px, py| {
            let (mut s, mut am, mut fq, mut n) = (0.0, 0.5, 1.0, 0.0);
            for i in 0..o {
                let q = 1.0 - (vnoise(px / z * fq + ph, py / z * fq, 4 + i as u32) * 2.0 - 1.0).abs();
                s += q * q * am;
                n += am;
                am *= 0.5;
                fq *= 2.1;
            }
            mix4(a, bb, s / n)
        });
    })),
    fx!("grid", "Grid", Generate, [f("size", "Aralık", 2.0, 1000.0, 60.0), f("w", "Çizgi kalınlığı", 0.2, 100.0, 2.0), c("c", "Renk", [1.0, 1.0, 1.0, 1.0]), p("off", "Ofset", 0.0, 0.0), FULL, MIX], Px(|fr, v, x| {
        let (z, w, k, of) = ((v.f("size") * x.s).max(2.0), v.f("w") * x.s, v.c("c"), v.p("off"));
        paint(fr, v, |px, py| {
            let line = |d: f32| { let f = d.rem_euclid(z); f.min(z - f) };
            let e = line(px + of[0] * x.s).min(line(py + of[1] * x.s));
            let m = 1.0 - smooth(w * 0.5 - 0.5, w * 0.5 + 0.5, e);
            [k[0], k[1], k[2], k[3] * m]
        });
    })),
    fx!("simple-starfield", "Simple Starfield", Generate, [f("n", "Yıldız", 1.0, 2000.0, 300.0), f("size", "Boyut", 0.5, 20.0, 2.0), f("speed", "Hız", 0.0, 2.0, 0.1), c("c", "Renk", [1.0, 1.0, 1.0, 1.0]), f("seed", "Tohum", 0.0, 100.0, 0.0)], Px(|fr, v, x| {
        star_layer(fr, x, v.i("n"), v.f("speed"), v.f("size"), v.rgb("c"), 0.0, v.i("seed") as u32 + 1);
    })),
    fx!("starfield", "Starfield", Generate, [f("n", "Yıldız", 1.0, 2000.0, 500.0), f("size", "Boyut", 0.5, 20.0, 3.0), f("speed", "Hız", 0.0, 3.0, 0.3), f("warp", "Çizgi uzunluğu", 0.0, 10.0, 1.0), c("c", "Renk", [0.8, 0.9, 1.0, 1.0]), f("seed", "Tohum", 0.0, 100.0, 0.0)], Px(|fr, v, x| {
        star_layer(fr, x, v.i("n"), v.f("speed"), v.f("size"), v.rgb("c"), v.f("warp"), v.i("seed") as u32 + 1);
    })),
    fx!("stripes", "Stripes", Generate, [f("size", "Genişlik", 1.0, 1000.0, 40.0), f("ang", "Açı", -360.0, 360.0, 45.0), f("duty", "Oran", 0.0, 1.0, 0.5), f("speed", "Hız", -1000.0, 1000.0, 0.0), c("a", "Renk 1", [1.0, 1.0, 1.0, 1.0]), c("b", "Renk 2", [0.0, 0.0, 0.0, 0.0]), FULL, MIX], Px(|fr, v, x| {
        let (z, r, du, sp, a, bb) = ((v.f("size") * x.s).max(1.0), v.f("ang").to_radians(), v.f("duty"), v.f("speed") * x.s, v.c("a"), v.c("b"));
        let (s, cs) = r.sin_cos();
        paint(fr, v, |px, py| {
            let u = ((px * cs + py * s + sp * x.t) / z).rem_euclid(1.0);
            if u < du { a } else { bb }
        });
    })),
    fx!("turbulence", "Turbulence", Generate, [f("size", "Boyut", 5.0, 2000.0, 150.0), f("oct", "Detay", 1.0, 8.0, 5.0), f("speed", "Hız", -3.0, 3.0, 0.2), c("a", "Koyu", [0.0, 0.0, 0.0, 1.0]), c("b", "Açık", [1.0, 1.0, 1.0, 1.0]), FULL, MIX], Px(|fr, v, x| {
        let (z, o, ph, a, bb) = ((v.f("size") * x.s).max(1.0), v.i("oct").max(1), x.t * v.f("speed"), v.c("a"), v.c("b"));
        paint(fr, v, |px, py| {
            let (mut s, mut am, mut fq, mut n) = (0.0, 0.5, 1.0, 0.0);
            for i in 0..o {
                s += (vnoise(px / z * fq + ph, py / z * fq - ph, 8 + i as u32) * 2.0 - 1.0).abs() * am;
                n += am;
                am *= 0.5;
                fq *= 2.0;
            }
            mix4(a, bb, (s / n * 1.8).min(1.0))
        });
    })),
    fx!("voronoi-cells", "Voronoi Cells", Generate, [f("size", "Hücre", 3.0, 1000.0, 60.0), ch("mode", "Görünüm", &["Renkli hücre", "Mesafe", "Kenarlar"], 0), f("speed", "Hız", 0.0, 5.0, 0.3), f("seed", "Tohum", 0.0, 100.0, 0.0), FULL, MIX], Px(|fr, v, x| {
        let (z, m, ph, sd) = ((v.f("size") * x.s).max(2.0), v.i("mode"), x.t * v.f("speed"), v.i("seed") as u32 + 1);
        paint(fr, v, |px, py| {
            let (u, q) = (px / z, py / z);
            let (cx, cy) = (u.floor() as i32, q.floor() as i32);
            let (mut d1, mut d2, mut id) = (9.0f32, 9.0f32, (0, 0));
            for j in -1..=1 { for i in -1..=1 {
                let (gx, gy) = (cx + i, cy + j);
                let ox = 0.5 + 0.5 * (hash2(gx, gy, sd) * TAU + ph).sin();
                let oy = 0.5 + 0.5 * (hash2(gx, gy, sd + 1) * TAU + ph).cos();
                let d = (gx as f32 + ox - u).hypot(gy as f32 + oy - q);
                if d < d1 { d2 = d1; d1 = d; id = (gx, gy); } else if d < d2 { d2 = d; }
            } }
            match m {
                1 => [d1.min(1.0), d1.min(1.0), d1.min(1.0), 1.0],
                2 => { let e = smooth(0.0, 0.08, d2 - d1); [e, e, e, 1.0] }
                _ => { let h = hsv_to_rgb([hash2(id.0, id.1, sd + 2), 0.6, 0.95]); [h[0], h[1], h[2], 1.0] }
            }
        });
    })),
];
