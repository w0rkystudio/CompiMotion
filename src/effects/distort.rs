//! Warps and repeaters. Warps pull from a copy; repeaters stamp the layer several times.
use super::anim::along;
use super::*;
use crate::frame::*;
use crate::fx;
use std::f32::consts::{PI, TAU};

fn rot(x: f32, y: f32, a: f32) -> (f32, f32) {
    let (s, c) = a.sin_cos();
    (x * c - y * s, x * s + y * c)
}

fn bulge(fr: &mut Frame, cx: f32, cy: f32, r: f32, k: f32, inner_only: bool) {
    let r = r.max(1.0);
    let k = k.clamp(-0.99, 4.0);
    if inner_only {
        let s = fr.clone();
        fr.shade(|x, y, p| {
            let d = (x - cx).hypot(y - cy) / r;
            if d >= 1.0 { return p; }
            let f = (1.0 - d * d).max(0.0);
            let z = 1.0 - k * f;
            s.sample_clamp(cx + (x - cx) * z, cy + (y - cy) * z)
        });
    } else {
        fr.warp(|x, y| {
            let d = (x - cx).hypot(y - cy) / r;
            if d >= 1.0 { return (x, y); }
            let f = (1.0 - d * d).max(0.0);
            let z = 1.0 - k * f;
            (cx + (x - cx) * z, cy + (y - cy) * z)
        });
    }
}

/// Stamps `src` at each (dx, dy, rotation, scale, opacity) around the layer center.
fn stamps(fr: &mut Frame, c: [f32; 2], list: &[(f32, f32, f32, f32, f32)]) {
    let src = fr.clone();
    fr.shade(|x, y, _| {
        let mut o = [0.0; 4];
        for &(dx, dy, r, sc, op) in list.iter().rev() {
            let (u, v) = rot(x - c[0] - dx, y - c[1] - dy, -r);
            let sc = sc.max(0.001);
            let q = src.sample(c[0] + u / sc, c[1] + v / sc);
            if q[3] > 0.0 { o = over(q.map(|n| n * op), o); }
        }
        o
    });
}

/// Hexagon cell center and the local offset for an axial hex grid of circumradius `r`.
fn hex_cell(x: f32, y: f32, r: f32) -> (i32, i32, f32, f32) {
    let q = (3f32.sqrt() / 3.0 * x - y / 3.0) / r;
    let s = (2.0 / 3.0 * y) / r;
    let (mut a, mut c) = (q.round(), s.round());
    let b = (-q - s).round();
    let (da, db, dc) = ((a - q).abs(), (b + q + s).abs(), (c - s).abs());
    if da > db && da > dc { a = -b - c } else if dc > db { c = -a - b }
    let (cx, cy) = (r * 3f32.sqrt() * (a + c / 2.0), r * 1.5 * c);
    (a as i32, c as i32, x - cx, y - cy)
}

fn displace_by(fr: &mut Frame, amt_x: f32, amt_y: f32, f: impl Fn(f32, f32) -> (f32, f32) + Sync) {
    fr.warp(|x, y| {
        let (a, b) = f(x, y);
        (x + a * amt_x, y + b * amt_y)
    });
}

pub const DEFS: &[Def] = &[
    fx!("bend", "Bend", Distort, [ch("org", "Köken", &["Sol", "Sağ", "Üst", "Alt"], 3), f("ang", "Açı", -180.0, 180.0, 0.0), f("start", "Başlangıç", 0.0, 1.0, 0.0), ch("before", "Başlangıçtan önce", &["Sabit", "Aynı yöne", "Ters yöne"], 0), f("head", "Baş ayarı", 0.0, 1.0, 0.75), f("tail", "Son ayarı", 0.0, 1.0, 0.25)], Px(|fr, v, x| {
        let (org, ang, st, bf, hd, tl) = (v.i("org"), v.f("ang").to_radians(), v.f("start"), v.i("before"), v.f("head"), v.f("tail"));
        if ang.abs() < 1e-4 { return; }
        let (w, h) = (fr.w as f32, fr.h as f32);
        let c = x.c;
        // Work in axis space: `d` runs along the bend from the origin edge, `s` across it.
        let horizontal = org < 2;
        let len = if horizontal { w } else { h };
        fr.warp(|px, py| {
            let (d, s) = match org {
                0 => (px, py - c[1]),
                1 => (w - px, py - c[1]),
                2 => (py, px - c[0]),
                _ => (h - py, px - c[0]),
            };
            let start = st * len;
            let u = ((d - start) / (len - start).max(1.0)).clamp(0.0, 1.0);
            let weight = smooth(0.0, tl.max(1e-3), u) * (1.0 - smooth(hd, 1.0 + 1e-3, u) * 0.5);
            let a = if d < start {
                match bf { 1 => ang * (d - start) / len, 2 => -ang * (d - start) / len, _ => 0.0 }
            } else { ang * (d - start) / len * weight.max(0.01) };
            let (ds, cs) = a.sin_cos();
            // Rotate about the start line, so far points swing farther.
            let (nd, ns) = (start + (d - start) * cs - s * ds, s * cs + (d - start) * ds);
            match org {
                0 => (nd, c[1] + ns),
                1 => (w - nd, c[1] + ns),
                2 => (c[0] + ns, nd),
                _ => (c[0] + ns, h - nd),
            }
        });
    })),
    fx!("circular-ripple", "Circular Ripple", Distort, [f("amp", "Genlik", 0.0, 200.0, 12.0), f("wave", "Dalga boyu", 2.0, 1000.0, 60.0), f("speed", "Hız", -10.0, 10.0, 1.0), f("r", "Yarıçap", 0.0, 3000.0, 1500.0), f("fall", "Azalma", 0.0, 1.0, 0.3)], Px(|fr, v, x| {
        let (a, w, sp, r, fl, c) = (v.f("amp") * x.s, (v.f("wave") * x.s).max(1.0), v.f("speed"), (v.f("r") * x.s).max(1.0), v.f("fall"), x.c);
        fr.warp(|px, py| {
            let (dx, dy) = (px - c[0], py - c[1]);
            let d = dx.hypot(dy).max(1e-3);
            let m = (1.0 - d / r).max(0.0) * (1.0 - fl) + fl * (1.0 - d / r).max(0.0).powi(2);
            let o = (d / w * TAU - x.t * sp * TAU).sin() * a * m;
            (px + dx / d * o, py + dy / d * o)
        });
    })),
    fx!("curl", "Curl", Distort, [f("amt", "Kıvrılma", 0.0, 1.0, 0.5), f("r", "Yarıçap", 1.0, 1000.0, 80.0), f("ang", "Yön", -360.0, 360.0, 135.0)], Px(|fr, v, x| {
        // Page curl: the part past the fold line wraps around a cylinder.
        let (am, r, a) = (v.f("amt"), (v.f("r") * x.s).max(1.0), v.f("ang").to_radians());
        let (dx, dy) = (a.cos(), a.sin());
        let (cx, cy) = (fr.w as f32 / 2.0, fr.h as f32 / 2.0);
        let ext = (fr.w as f32 * dx.abs() + fr.h as f32 * dy.abs()).max(1.0);
        let fold = ext * (0.5 - am);
        let s = fr.clone();
        fr.shade(|px, py, _| {
            let d = (px - cx) * dx + (py - cy) * dy;
            let t = d - fold;
            if t < 0.0 { return s.sample(px, py); }
            if t > r { return [0.0; 4]; }
            // Front of the sheet at the arc, and the back flips over it.
            let th = (t / r).asin();
            let front = fold + r * th;
            let back = fold + r * (PI - th);
            let at = |u: f32| s.sample(px + (u - d) * dx, py + (u - d) * dy);
            let bk = at(back);
            let sh = 0.55 + 0.45 * th.cos();
            let bk2 = [bk[0] * sh, bk[1] * sh, bk[2] * sh, bk[3]];
            if bk2[3] > 0.01 { over(bk2, at(front)) } else { at(front) }
        });
    })),
    fx!("displacement-map", "Displacement Map", Distort, [ch("src", "Kaynak", &["Katman", "Arka plan"], 1), f("ax", "Yatay", -500.0, 500.0, 30.0), f("ay", "Dikey", -500.0, 500.0, 30.0)], Px(|fr, v, x| {
        let (ax, ay) = (v.f("ax") * x.s, v.f("ay") * x.s);
        let map = if v.i("src") == 0 { fr.clone() } else { x.bg.clone() };
        displace_by(fr, ax, ay, |px, py| {
            let c = unpremul(map.sample_clamp(px, py));
            (c[0] - 0.5, c[1] - 0.5)
        });
    })),
    fx!("flip-layer", "Flip Layer", Distort, [ch("axis", "Eksen", &["Yatay", "Dikey", "İkisi"], 0)], Px(|fr, v, x| {
        let (a, c) = (v.i("axis"), x.c);
        fr.warp(|px, py| (if a != 1 { 2.0 * c[0] - px } else { px }, if a != 0 { 2.0 * c[1] - py } else { py }));
    })),
    fx!("fractal-warp", "Fractal Warp", Distort, [f("amt", "Miktar", 0.0, 500.0, 40.0), f("size", "Boyut", 5.0, 2000.0, 150.0), f("oct", "Detay", 1.0, 6.0, 4.0), f("speed", "Hız", 0.0, 5.0, 0.3)], Px(|fr, v, x| {
        let (a, sz, o) = (v.f("amt") * x.s, (v.f("size") * x.s).max(1.0), v.i("oct") as u32);
        let ph = x.t * v.f("speed");
        displace_by(fr, a, a, |px, py| {
            let (u, w) = (px / sz + ph, py / sz);
            (fbm(u, w, o, 1) - 0.5, fbm(u + 5.2, w + 1.3, o, 2) - 0.5)
        });
    })),
    fx!("glass", "Glass", Distort, [f("h", "Yükseklik", 0.0, 300.0, 30.0), f("blur", "Kenar", 0.5, 200.0, 12.0), f("spec", "Parlaklık", 0.0, 2.0, 0.6)], Px(|fr, v, x| {
        // Refract the background through the layer's soft alpha "height field".
        let (hh, sp) = (v.f("h") * x.s, v.f("spec"));
        let mut b = fr.clone();
        b.gauss(v.f("blur") * x.s, v.f("blur") * x.s);
        let bg = x.bg;
        fr.shade(|px, py, p| {
            if p[3] <= 0.0 { return p; }
            let dx = b.sample(px + 1.0, py)[3] - b.sample(px - 1.0, py)[3];
            let dy = b.sample(px, py + 1.0)[3] - b.sample(px, py - 1.0)[3];
            let q = bg.sample_clamp(px + dx * hh, py + dy * hh);
            let hi = ((-dx - dy).max(0.0) * sp * 4.0).min(1.0);
            let c = mix3(unpremul(q), [1.0; 3], hi);
            premul(c, p[3])
        });
    })),
    fx!("soft-glass", "Soft Glass", Distort, [f("h", "Kırılma", 0.0, 300.0, 24.0), f("frost", "Buzlanma", 0.0, 100.0, 10.0), f("tint", "Renk karışımı", 0.0, 1.0, 0.2)], Px(|fr, v, x| {
        let (hh, tn) = (v.f("h") * x.s, v.f("tint"));
        let mut bg = x.bg.clone();
        let r = v.f("frost") * x.s;
        bg.gauss(r, r);
        let mut b = fr.clone();
        b.gauss(8.0 * x.s, 8.0 * x.s);
        fr.shade(|px, py, p| {
            if p[3] <= 0.0 { return p; }
            let dx = b.sample(px + 1.0, py)[3] - b.sample(px - 1.0, py)[3];
            let dy = b.sample(px, py + 1.0)[3] - b.sample(px, py - 1.0)[3];
            let q = bg.sample_clamp(px + dx * hh, py + dy * hh);
            premul(mix3(unpremul(q), unpremul(p), tn), p[3])
        });
    })),
    fx!("inner-pinch-bulge", "Inner Pinch/Bulge", Distort, [f("str", "Güç", -1.0, 1.0, 0.5), f("r", "Yarıçap", 1.0, 3000.0, 400.0), p("c", "Merkez kaydır", 0.0, 0.0)], Px(|fr, v, x| {
        let o = v.p("c");
        bulge(fr, x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s, v.f("r") * x.s, v.f("str"), true);
    })),
    fx!("pinch-bulge", "Pinch/Bulge", Distort, [f("str", "Güç", -1.0, 1.0, 0.5), f("r", "Yarıçap", 1.0, 3000.0, 400.0), p("c", "Merkez kaydır", 0.0, 0.0)], Px(|fr, v, x| {
        let o = v.p("c");
        bulge(fr, x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s, v.f("r") * x.s, v.f("str"), false);
    })),
    fx!("kaleidoscope", "Kaleidoscope", Distort, [f("n", "Dilim", 2.0, 64.0, 6.0), f("rot", "Dönme", -360.0, 360.0, 0.0), f("zoom", "Yakınlaştır", 0.1, 5.0, 1.0)], Px(|fr, v, x| {
        let (n, r, z, c) = (v.f("n").max(2.0).round(), v.f("rot").to_radians(), v.f("zoom").max(0.01), x.c);
        let seg = TAU / n;
        fr.warp(|px, py| {
            let (dx, dy) = (px - c[0], py - c[1]);
            let d = dx.hypot(dy) / z;
            let a = (dy.atan2(dx) + r).rem_euclid(seg);
            let a = (a - seg / 2.0).abs();
            (c[0] + a.cos() * d, c[1] + a.sin() * d)
        });
    })),
    fx!("mirror", "Mirror", Distort, [ch("axis", "Eksen", &["Yatay", "Dikey", "İkisi"], 0), p("off", "Eksen kaydır", 0.0, 0.0), ch("side", "Korunan taraf", &["Sol / üst", "Sağ / alt"], 0), f("alpha", "Yansıma opaklığı", 0.0, 1.0, 1.0)], Px(|fr, v, x| {
        let (a, o, sd, al) = (v.i("axis"), v.p("off"), v.i("side"), v.f("alpha"));
        let (mx, my) = (x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s);
        let s = fr.clone();
        fr.shade(|px, py, p| {
            let flip_x = a != 1 && ((sd == 0) == (px > mx));
            let flip_y = a != 0 && ((sd == 0) == (py > my));
            if !flip_x && !flip_y { return p; }
            let q = s.sample(if flip_x { 2.0 * mx - px } else { px }, if flip_y { 2.0 * my - py } else { py });
            over(q.map(|n| n * al), [0.0; 4])
        });
    })),
    fx!("offset", "Offset", Distort, [p("off", "Kaydır", 0.0, 0.0), b("wrap", "Sar", true)], Px(|fr, v, x| {
        let o = v.p("off");
        let (ox, oy, w) = (o[0] * x.s, o[1] * x.s, v.b("wrap"));
        let s = fr.clone();
        fr.shade(|px, py, _| if w { s.sample_wrap(px - ox, py - oy) } else { s.sample(px - ox, py - oy) });
    })),
    fx!("polar-coordinates", "Polar Coordinates", Distort, [ch("dir", "Yön", &["Dikdörtgenden kutupsala", "Kutupsaldan dikdörtgene"], 0), f("mix", "Karışım", 0.0, 1.0, 1.0)], Px(|fr, v, x| {
        let (d, k) = (v.i("dir"), v.f("mix"));
        let (w, h, c) = (fr.w as f32, fr.h as f32, x.c);
        let rmax = w.hypot(h) * 0.5;
        fr.warp(|px, py| {
            let (u, q) = if d == 0 {
                let (dx, dy) = (px - c[0], py - c[1]);
                (((dy.atan2(dx) + PI) / TAU) * w, dx.hypot(dy) / rmax * h)
            } else {
                let a = px / w * TAU - PI;
                let r = py / h * rmax;
                (c[0] + a.cos() * r, c[1] + a.sin() * r)
            };
            (px + (u - px) * k, py + (q - py) * k)
        });
    })),
    fx!("polar-displacement-map", "Polar Displacement Map", Distort, [ch("src", "Kaynak", &["Katman", "Arka plan"], 1), f("r", "Radyal", -500.0, 500.0, 40.0), f("a", "Açısal", -180.0, 180.0, 20.0)], Px(|fr, v, x| {
        let (ra, an, c) = (v.f("r") * x.s, v.f("a").to_radians(), x.c);
        let map = if v.i("src") == 0 { fr.clone() } else { x.bg.clone() };
        fr.warp(|px, py| {
            let q = unpremul(map.sample_clamp(px, py));
            let (dx, dy) = (px - c[0], py - c[1]);
            let d = dx.hypot(dy) + (q[0] - 0.5) * ra;
            let a = dy.atan2(dx) + (q[1] - 0.5) * an;
            (c[0] + a.cos() * d, c[1] + a.sin() * d)
        });
    })),
    fx!("random-displacement", "Random Displacement", Distort, [f("amt", "Miktar", 0.0, 500.0, 20.0), f("size", "Blok", 1.0, 500.0, 30.0), f("rate", "Hız", 0.0, 60.0, 8.0), f("seed", "Tohum", 0.0, 100.0, 0.0)], Px(|fr, v, x| {
        let (a, sz) = (v.f("amt") * x.s, (v.f("size") * x.s).max(1.0));
        let sd = (x.t * v.f("rate")) as u32 * 5 + v.i("seed") as u32;
        displace_by(fr, a, a, |px, py| {
            let (ix, iy) = ((px / sz) as i32, (py / sz) as i32);
            (hash2(ix, iy, sd) - 0.5, hash2(ix, iy, sd + 2) - 0.5)
        });
    })),
    fx!("spherize", "Spherize", Distort, [f("r", "Yarıçap", 1.0, 3000.0, 400.0), f("str", "Güç", -1.0, 1.0, 1.0), f("rot", "Dönme (hareket)", -5.0, 5.0, 0.0)], Px(|fr, v, x| {
        let (r, st, c) = ((v.f("r") * x.s).max(1.0), v.f("str"), x.c);
        let ph = x.t * v.f("rot") * r;
        let w = fr.w as f32;
        let s = fr.clone();
        fr.shade(|px, py, p| {
            let (dx, dy) = ((px - c[0]) / r, (py - c[1]) / r);
            let d = dx.hypot(dy);
            if d >= 1.0 { return if ph != 0.0 { [0.0; 4] } else { p }; }
            let z = (1.0 - d * d).max(0.0).sqrt();
            let k = 1.0 + (d.asin() / d.max(1e-4) * (2.0 / PI) * 1.3 - 1.0) * st;
            let (u, q) = (c[0] + dx * k * r + ph, c[1] + dy * k * r);
            let m = if ph != 0.0 { s.sample_wrap(u.rem_euclid(w), q) } else { s.sample(u, q) };
            m.map(|n| n) .iter().enumerate().fold([0.0; 4], |mut o, (i, n)| { o[i] = if i == 3 { *n } else { *n * (0.6 + 0.4 * z) }; o })
        });
    })),
    fx!("squeeze", "Squeeze", Distort, [f("x", "Yatay", -1.0, 1.0, 0.3), f("y", "Dikey", -1.0, 1.0, 0.0)], Px(|fr, v, x| {
        let (ax, ay, c) = (v.f("x"), v.f("y"), x.c);
        let (w, h) = (fr.w as f32, fr.h as f32);
        fr.warp(|px, py| {
            let (nx, ny) = ((px - c[0]) / (w * 0.5), (py - c[1]) / (h * 0.5));
            let (kx, ky) = (1.0 + ax * (1.0 - ny * ny).max(0.0), 1.0 + ay * (1.0 - nx * nx).max(0.0));
            (c[0] + (px - c[0]) * kx, c[1] + (py - c[1]) * ky)
        });
    })),
    fx!("stretch-axis", "Stretch Axis", Distort, [f("ang", "Eksen açısı", -360.0, 360.0, 0.0), f("amt", "Esnetme", 0.05, 10.0, 1.5)], Px(|fr, v, x| {
        let (a, k, c) = (v.f("ang").to_radians(), v.f("amt").max(0.01), x.c);
        fr.warp(|px, py| {
            let (u, q) = rot(px - c[0], py - c[1], -a);
            let (nx, ny) = rot(u / k, q, a);
            (c[0] + nx, c[1] + ny)
        });
    })),
    fx!("stretch-segment", "Stretch Segment", Distort, [f("pos", "Bölüm konumu", 0.0, 1.0, 0.5), f("w", "Bölüm genişliği", 0.0, 1.0, 0.1), f("amt", "Esnetme", 1.0, 10.0, 3.0), ch("axis", "Eksen", &["Yatay", "Dikey"], 0)], Px(|fr, v, _| {
        let (pos, sw, k, ax) = (v.f("pos"), v.f("w").max(1e-3), v.f("amt").max(1.0), v.i("axis"));
        let (w, h) = (fr.w as f32, fr.h as f32);
        let len = if ax == 0 { w } else { h };
        let (a, b) = ((pos - sw / 2.0) * len, (pos + sw / 2.0) * len);
        let grow = (b - a) * (k - 1.0) * 0.5;
        // Inverse piecewise map: outside the stretched zone, content is pushed apart.
        let f = |d: f32| {
            if d < a - grow { d + grow } else if d > b + grow { d - grow } else { a + (d - (a - grow)) / k }
        };
        fr.warp(|px, py| if ax == 0 { (f(px), py) } else { (px, f(py)) });
    })),
    fx!("swirl", "Swirl", Distort, [f("deg", "Açı", -3600.0, 3600.0, 180.0), f("r", "Yarıçap", 1.0, 3000.0, 400.0), p("c", "Merkez kaydır", 0.0, 0.0)], Px(|fr, v, x| {
        let (a, r, o) = (v.f("deg").to_radians(), (v.f("r") * x.s).max(1.0), v.p("c"));
        let (cx, cy) = (x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s);
        fr.warp(|px, py| {
            let (dx, dy) = (px - cx, py - cy);
            let d = (dx.hypot(dy) / r).min(1.0);
            let (u, q) = rot(dx, dy, a * (1.0 - d).powi(2));
            (cx + u, cy + q)
        });
    })),
    fx!("turbulent-displace", "Turbulent Displace", Distort, [f("amt", "Miktar", 0.0, 500.0, 30.0), f("size", "Boyut", 5.0, 2000.0, 100.0), f("speed", "Hız", 0.0, 5.0, 0.5), f("seed", "Tohum", 0.0, 100.0, 0.0)], Px(|fr, v, x| {
        let (a, sz, ph, sd) = (v.f("amt") * x.s, (v.f("size") * x.s).max(1.0), x.t * v.f("speed"), v.i("seed") as u32);
        let t = |u: f32, w: f32, s: u32| {
            // Turbulence: folded noise gives sharp creases.
            let (mut o, mut am, mut fq) = (0.0, 0.5, 1.0);
            for i in 0..4 {
                o += ((vnoise(u * fq, w * fq, s + i) - 0.5).abs() * 2.0) * am;
                am *= 0.5;
                fq *= 2.0;
            }
            o - 0.45
        };
        displace_by(fr, a, a, |px, py| (t(px / sz + ph, py / sz, sd + 1), t(px / sz, py / sz + ph, sd + 7)));
    })),
    fx!("wave-warp", "Wave Warp", Distort, [f("amp", "Genlik", 0.0, 500.0, 20.0), f("wave", "Dalga boyu", 2.0, 2000.0, 120.0), f("ang", "Yön", -360.0, 360.0, 0.0), f("speed", "Hız", -10.0, 10.0, 1.0), f("phase", "Evre", 0.0, 1.0, 0.0)], Px(|fr, v, x| {
        let (a, w, r) = (v.f("amp") * x.s, (v.f("wave") * x.s).max(1.0), v.f("ang").to_radians());
        let ph = x.t * v.f("speed") + v.f("phase");
        let (dx, dy) = (r.cos(), r.sin());
        fr.warp(|px, py| {
            let m = (((px * dx + py * dy) / w + ph) * TAU).sin() * a;
            (px - dy * m, py + dx * m)
        });
    })),
    // --- Repeaters
    fx!("grid-repeat", "Grid Repeat", Repeat, [f("nx", "Sütun", 1.0, 30.0, 3.0), f("ny", "Satır", 1.0, 30.0, 3.0), p("sp", "Aralık", 200.0, 200.0), f("scale", "Ölçek", 0.01, 2.0, 1.0)], Px(|fr, v, x| {
        let (nx, ny, s, sc) = (v.i("nx").max(1), v.i("ny").max(1), v.p("sp"), v.f("scale"));
        let mut l = vec![];
        for j in 0..ny { for i in 0..nx {
            l.push(((i as f32 - (nx - 1) as f32 / 2.0) * s[0] * x.s, (j as f32 - (ny - 1) as f32 / 2.0) * s[1] * x.s, 0.0, sc, 1.0));
        } }
        stamps(fr, x.c, &l);
    })),
    fx!("linear-repeat", "Linear Repeat", Repeat, [f("n", "Kopya", 1.0, 64.0, 5.0), p("step", "Adım", 100.0, 0.0), f("rot", "Adım dönmesi", -180.0, 180.0, 0.0), f("scale", "Adım ölçeği", 0.5, 1.5, 1.0), f("fade", "Solma", 0.0, 1.0, 0.0)], Px(|fr, v, x| {
        let (n, st, r, sc, fd) = (v.i("n").max(1), v.p("step"), v.f("rot").to_radians(), v.f("scale"), v.f("fade"));
        let l: Vec<_> = (0..n).map(|i| {
            let k = i as f32;
            (st[0] * k * x.s, st[1] * k * x.s, r * k, sc.powf(k), 1.0 - fd * k / n as f32)
        }).collect();
        stamps(fr, x.c, &l);
    })),
    fx!("radial-repeat", "Radial Repeat", Repeat, [f("n", "Kopya", 1.0, 64.0, 8.0), f("r", "Yarıçap", 0.0, 2000.0, 200.0), f("rot", "Başlangıç", -360.0, 360.0, 0.0), b("orient", "Yöne dön", true), f("speed", "Dönüş hızı", -5.0, 5.0, 0.0)], Px(|fr, v, x| {
        let (n, rr, or) = (v.i("n").max(1), v.f("r") * x.s, v.b("orient"));
        let start = v.f("rot").to_radians() + x.t * v.f("speed") * TAU;
        let l: Vec<_> = (0..n).map(|i| {
            let a = start + i as f32 / n as f32 * TAU;
            (a.cos() * rr, a.sin() * rr, if or { a + PI / 2.0 } else { 0.0 }, 1.0, 1.0)
        }).collect();
        stamps(fr, x.c, &l);
    })),
    fx!("repeat", "Repeat", Repeat, [f("n", "Kopya", 1.0, 64.0, 4.0), p("pos", "Konum adımı", 60.0, 40.0), f("rot", "Dönme adımı", -360.0, 360.0, 15.0), f("scale", "Ölçek adımı", 0.2, 2.0, 0.9), f("op", "Opaklık adımı", 0.0, 1.0, 0.85)], Px(|fr, v, x| {
        let (n, st, r, sc, op) = (v.i("n").max(1), v.p("pos"), v.f("rot").to_radians(), v.f("scale"), v.f("op"));
        let l: Vec<_> = (0..n).map(|i| {
            let k = i as f32;
            (st[0] * k * x.s, st[1] * k * x.s, r * k, sc.powf(k), op.powf(k))
        }).collect();
        stamps(fr, x.c, &l);
    })),
    fx!("scatter-repeat", "Scatter Repeat", Repeat, [f("n", "Kopya", 1.0, 200.0, 20.0), p("area", "Alan", 800.0, 500.0), f("scale", "Ölçek değişimi", 0.0, 1.0, 0.5), f("rot", "Dönme değişimi", 0.0, 180.0, 90.0), f("seed", "Tohum", 0.0, 100.0, 0.0)], Px(|fr, v, x| {
        let (n, ar, sv, rv, sd) = (v.i("n").max(1), v.p("area"), v.f("scale"), v.f("rot").to_radians(), v.i("seed") as u32 * 29 + 1);
        let l: Vec<_> = (0..n).map(|i| {
            let r = |k: u32| hash2(i, k as i32, sd);
            ((r(1) - 0.5) * ar[0] * x.s, (r(2) - 0.5) * ar[1] * x.s, (r(3) - 0.5) * 2.0 * rv, 1.0 - sv * r(4), 1.0)
        }).collect();
        stamps(fr, x.c, &l);
    })),
    fx!("repeat-along-path", "Repeat Along Path", Repeat, [f("n", "Kopya", 1.0, 128.0, 12.0), b("orient", "Yöne dön", true), f("off", "Başlangıç", 0.0, 1.0, 0.0), f("scale", "Ölçek", 0.01, 2.0, 0.5), f("r", "Şekil yoksa daire yarıçapı", 1.0, 2000.0, 250.0)], Px(|fr, v, x| {
        let (n, or, of, sc, rr) = (v.i("n").max(1), v.b("orient"), v.f("off"), v.f("scale"), v.f("r") * x.s);
        let circle: Vec<[f32; 2]> = (0..=64).map(|i| {
            let a = i as f32 / 64.0 * TAU;
            [x.c[0] + a.cos() * rr, x.c[1] + a.sin() * rr]
        }).collect();
        let path: &[[f32; 2]] = if x.path.len() >= 2 { x.path } else { &circle };
        let l: Vec<_> = (0..n).filter_map(|i| {
            let (q, a) = along(path, (i as f32 / n as f32 + of).rem_euclid(1.0))?;
            Some((q[0] - x.c[0], q[1] - x.c[1], if or { a } else { 0.0 }, sc, 1.0))
        }).collect();
        stamps(fr, x.c, &l);
    })),
    fx!("tile-rotate", "Tile Rotate", Repeat, [f("size", "Kare boyutu", 4.0, 1000.0, 120.0), f("deg", "Açı", -360.0, 360.0, 30.0), f("vary", "Rastgele", 0.0, 1.0, 0.0)], Px(|fr, v, x| {
        let (z, a, va) = ((v.f("size") * x.s).max(1.0), v.f("deg").to_radians(), v.f("vary"));
        fr.warp(|px, py| {
            let (ix, iy) = ((px / z).floor(), (py / z).floor());
            let (cx, cy) = ((ix + 0.5) * z, (iy + 0.5) * z);
            let k = 1.0 + (hash2(ix as i32, iy as i32, 3) - 0.5) * 2.0 * va;
            let (u, q) = rot(px - cx, py - cy, -a * k);
            (cx + u, cy + q)
        });
    })),
    fx!("tile-shift", "Tile Shift", Repeat, [f("size", "Şerit", 2.0, 1000.0, 60.0), f("amt", "Kayma", -1000.0, 1000.0, 40.0), ch("axis", "Eksen", &["Satır", "Sütun"], 0), f("rate", "Hız", 0.0, 30.0, 0.0)], Px(|fr, v, x| {
        let (z, a, ax) = ((v.f("size") * x.s).max(1.0), v.f("amt") * x.s, v.i("axis"));
        let sd = (x.t * v.f("rate")) as u32;
        let s = fr.clone();
        fr.shade(|px, py, _| {
            let band = if ax == 0 { py } else { px };
            let i = (band / z).floor() as i32;
            let d = (hash1(i, sd + 1) - 0.5) * 2.0 * a;
            if ax == 0 { s.sample_wrap(px + d, py) } else { s.sample_wrap(px, py + d) }
        });
    })),
    fx!("tiles", "Tiles", Repeat, [f("nx", "Sütun", 1.0, 40.0, 4.0), f("ny", "Satır", 1.0, 40.0, 4.0), f("gap", "Boşluk", 0.0, 0.9, 0.03), b("mirror", "Aynalı", false)], Px(|fr, v, _| {
        let (nx, ny, g, m) = (v.f("nx").max(1.0), v.f("ny").max(1.0), v.f("gap"), v.b("mirror"));
        let (w, h) = (fr.w as f32, fr.h as f32);
        let s = fr.clone();
        fr.shade(|x, y, _| {
            let (u, q) = (x / w * nx, y / h * ny);
            let (fu, fq) = (u.fract(), q.fract());
            if fu < g / 2.0 || fu > 1.0 - g / 2.0 || fq < g / 2.0 || fq > 1.0 - g / 2.0 { return [0.0; 4]; }
            let (i, j) = (u as i32, q as i32);
            let a = if m && i % 2 == 1 { 1.0 - fu } else { fu };
            let b = if m && j % 2 == 1 { 1.0 - fq } else { fq };
            s.sample(a * w, b * h)
        });
    })),
    fx!("hexagon-array", "Hexagon Array", Repeat, [f("r", "Halka", 0.0, 8.0, 2.0), f("sp", "Aralık", 1.0, 2000.0, 180.0), f("scale", "Ölçek", 0.01, 2.0, 0.5)], Px(|fr, v, x| {
        let (n, sp, sc) = (v.i("r").max(0), v.f("sp") * x.s, v.f("scale"));
        let mut l = vec![];
        for q in -n..=n { for r in (-n).max(-q - n)..=n.min(-q + n) {
            l.push((sp * (q as f32 + r as f32 / 2.0), sp * 0.866 * r as f32, 0.0, sc, 1.0));
        } }
        stamps(fr, x.c, &l);
    })),
    fx!("hexagon-tile-rotate", "Hexagon Tile Rotate", Repeat, [f("size", "Boyut", 4.0, 1000.0, 80.0), f("deg", "Açı", -360.0, 360.0, 30.0), f("vary", "Rastgele", 0.0, 1.0, 0.0)], Px(|fr, v, x| {
        let (z, a, va) = ((v.f("size") * x.s).max(2.0), v.f("deg").to_radians(), v.f("vary"));
        fr.warp(|px, py| {
            let (i, j, u, q) = hex_cell(px, py, z);
            let k = 1.0 + (hash2(i, j, 3) - 0.5) * 2.0 * va;
            let (nu, nq) = rot(u, q, -a * k);
            (px - u + nu, py - q + nq)
        });
    })),
    fx!("hexagon-tile-shift", "Hexagon Tile Shift", Repeat, [f("size", "Boyut", 4.0, 1000.0, 80.0), f("amt", "Kayma", 0.0, 500.0, 30.0), f("rate", "Hız", 0.0, 30.0, 0.0)], Px(|fr, v, x| {
        let (z, a, sd) = ((v.f("size") * x.s).max(2.0), v.f("amt") * x.s, (x.t * v.f("rate")) as u32);
        fr.warp(|px, py| {
            let (i, j, _, _) = hex_cell(px, py, z);
            (px + (hash2(i, j, sd + 5) - 0.5) * 2.0 * a, py + (hash2(i, j, sd + 6) - 0.5) * 2.0 * a)
        });
    })),
    fx!("hexagon-tiling", "Hexagon Tiling", Repeat, [f("size", "Boyut", 4.0, 1000.0, 60.0), f("gap", "Boşluk", 0.0, 0.9, 0.1), b("flat", "Düz renk", true)], Px(|fr, v, x| {
        let (z, g, fl) = ((v.f("size") * x.s).max(2.0), v.f("gap"), v.b("flat"));
        let s = fr.clone();
        fr.shade(|px, py, _| {
            let (_, _, u, q) = hex_cell(px, py, z);
            let (au, aq) = (u.abs(), q.abs());
            let d = (au * 0.866 + aq * 0.5).max(aq) / z;
            if d > (1.0 - g) * 0.866 { return [0.0; 4]; }
            if fl { s.sample(px - u, py - q) } else { s.sample(px, py) }
        });
    })),
];
