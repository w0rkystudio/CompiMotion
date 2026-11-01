//! Blurs, glows, shadows, edges, halftones, wipes and background-sampling effects.
use super::*;
use crate::frame::*;
use crate::fx;
use std::f32::consts::{PI, TAU};

fn screen_add(fr: &mut Frame, g: &Frame, k: f32, tint: Option<[f32; 3]>) {
    use rayon::prelude::*;
    fr.px.par_iter_mut().zip(g.px.par_iter()).for_each(|(d, s)| {
        let c = match tint {
            Some(t) => [t[0] * s[3], t[1] * s[3], t[2] * s[3]],
            None => [s[0], s[1], s[2]],
        };
        for i in 0..3 {
            let v = c[i] * k;
            d[i] = d[i] + v - (d[i] * v).clamp(0.0, 1.0);
        }
        d[3] = (d[3] + s[3] * k * (1.0 - d[3])).clamp(0.0, 1.0);
    });
}

fn bright_pass(fr: &Frame, th: f32) -> Frame {
    let mut g = fr.clone();
    g.shade(|_, _, p| {
        let c = unpremul(p);
        premul(c, p[3] * smooth(th, th + 0.15, luma(c)))
    });
    g
}

/// Alpha gradient magnitude: 1 near edges of the layer.
fn edges(fr: &Frame, w: f32) -> Frame {
    let mut e = fr.clone();
    let r = w.max(0.5) as i32;
    e.shade(|x, y, _| {
        let (ix, iy) = (x as i32, y as i32);
        let dx = fr.get(ix + r, iy)[3] - fr.get(ix - r, iy)[3];
        let dy = fr.get(ix, iy + r)[3] - fr.get(ix, iy - r)[3];
        let a = (dx * dx + dy * dy).sqrt().min(1.0);
        [a, a, a, a]
    });
    e
}

fn alpha_levels(fr: &mut Frame, lo: f32, hi: f32) {
    fr.shade(|_, _, p| {
        let k = smooth(lo, hi, p[3]);
        let c = unpremul(p);
        premul(c, k)
    });
}

/// Replaces alpha with `f(alpha)` from a blurred copy while keeping original color.
fn from_blurred_alpha(fr: &mut Frame, r: f32, f: impl Fn(f32, f32, f32) -> f32 + Sync) {
    let mut b = fr.clone();
    b.gauss(r, r);
    let orig = fr.clone();
    let w = fr.w;
    fr.px.iter_mut().enumerate().for_each(|(i, d)| {
        let a = f(b.px[i][3], (i % w) as f32, (i / w) as f32).clamp(0.0, 1.0);
        let o = orig.px[i];
        // Transparent source pixels borrow color from the blurred copy.
        let c = if o[3] > 0.01 { unpremul(o) } else { unpremul(b.px[i]) };
        *d = premul(c, a);
    });
}

fn halftone(fr: &mut Frame, cell: f32, ang: f32, str_: f32, lines: bool, ph: f32) {
    let (sn, cs) = ang.to_radians().sin_cos();
    let cell = cell.max(1.5);
    fr.shade(|x, y, p| {
        let c = unpremul(p);
        let (u, v) = ((x * cs + y * sn) / cell + ph, (-x * sn + y * cs) / cell + ph);
        let d = if lines { (v.fract().abs() - 0.5).abs() * 2.0 } else { ((u.rem_euclid(1.0) - 0.5).powi(2) + (v.rem_euclid(1.0) - 0.5).powi(2)).sqrt() * 1.41 };
        let k = 1.0 - smooth(-0.08, 0.08, d - (1.0 - luma(c)).sqrt() * 0.95);
        let ink = mix3([1.0, 1.0, 1.0], [0.0, 0.0, 0.0], k);
        premul(mix3(c, ink, str_), p[3])
    });
}

fn streak_rays(fr: &mut Frame, kind: i32, len: f32, center: [f32; 2], ang: f32, k: f32, th: f32) {
    let src = bright_pass(fr, th);
    let mut g = src.clone();
    let (dx, dy) = (ang.to_radians().cos() * len, ang.to_radians().sin() * len);
    g.smear(32, |u, x, y| {
        let u = u + 0.5;
        match kind {
            0 => (x - dx * u, y - dy * u),
            1 => {
                let a = u * len * 0.01;
                let (s, c) = a.sin_cos();
                let (rx, ry) = (x - center[0], y - center[1]);
                (center[0] + rx * c - ry * s, center[1] + rx * s + ry * c)
            }
            _ => {
                let z = 1.0 - u * (len * 0.002).min(0.95);
                (center[0] + (x - center[0]) * z, center[1] + (y - center[1]) * z)
            }
        }
    });
    screen_add(fr, &g, k, None);
}

fn dissolve_mask(fr: &mut Frame, m: impl Fn(f32, f32) -> f32 + Sync) {
    fr.shade(|x, y, p| p.map(|v| v * m(x, y).clamp(0.0, 1.0)));
}

fn contour_value(fr: &Frame, x: f32, y: f32, dist: &Frame) -> f32 {
    let _ = fr;
    dist.sample_clamp(x, y)[3]
}

pub const DEFS: &[Def] = &[
    // --- Blur
    fx!("box-blur", "Box Blur", Blur, [f("str", "Güç", 0.0, 2.0, 0.15)], Px(|fr, v, x| {
        let r = v.f("str") * 100.0 * x.s;
        fr.box_blur(r, r);
        fr.box_blur(r * 0.3, r * 0.3);
    })),
    fx!("precise-box-blur", "Precise Box Blur", Blur, [f("rx", "Yatay", 0.0, 500.0, 12.0), f("ry", "Dikey", 0.0, 500.0, 12.0)], Px(|fr, v, x| fr.box_blur(v.f("rx") * x.s, v.f("ry") * x.s))),
    fx!("directional-blur", "Directional Blur", Blur, [f("len", "Uzunluk", 0.0, 1000.0, 40.0), f("ang", "Açı", -360.0, 360.0, 0.0)], Px(|fr, v, x| {
        let (l, a) = (v.f("len") * x.s, v.f("ang").to_radians());
        let (dx, dy) = (a.cos() * l, a.sin() * l);
        fr.smear((l as usize).clamp(4, 64), |u, px, py| (px + dx * u, py + dy * u));
    })),
    fx!("gaussian-blur", "Gaussian Blur", Blur, [f("str", "Güç", 0.0, 500.0, 15.0), ch("dir", "Yön", &["Her iki yön", "Yatay", "Dikey"], 0)], Px(|fr, v, x| {
        let (s, d) = (v.f("str") * x.s, v.i("dir"));
        fr.gauss(if d == 2 { 0.0 } else { s }, if d == 1 { 0.0 } else { s });
    })),
    fx!("inner-blur", "Inner Blur", Blur, [f("str", "Güç", 0.0, 300.0, 12.0)], Px(|fr, v, x| {
        let r = v.f("str") * x.s;
        let orig = fr.clone();
        fr.gauss(r, r);
        let b = fr.clone();
        fr.shade(|px, py, _| {
            let (o, s) = (orig.sample(px, py), b.sample(px, py));
            premul(unpremul(s), o[3])
        });
    })),
    fx!("lens-blur", "Lens Blur", Blur, [f("r", "Yarıçap", 0.0, 200.0, 10.0), f("boost", "Parlak noktalar", 0.0, 8.0, 2.0)], Px(|fr, v, x| {
        let r = v.f("r") * x.s;
        let boost = v.f("boost");
        let src = fr.clone();
        // Golden-angle disc kernel gives round bokeh without a huge convolution.
        let n = 48;
        let offs: Vec<(f32, f32)> = (0..n).map(|i| {
            let (q, a) = (((i as f32 + 0.5) / n as f32).sqrt() * r, i as f32 * 2.399963);
            (a.cos() * q, a.sin() * q)
        }).collect();
        fr.shade(|px, py, _| {
            let (mut s, mut tw) = ([0.0; 4], 0.0);
            for (dx, dy) in &offs {
                let p = src.sample(px + dx, py + dy);
                let w = 1.0 + boost * luma([p[0], p[1], p[2]]).powi(3);
                for c in 0..4 { s[c] += p[c] * w; }
                tw += w;
            }
            s.map(|q| q / tw)
        });
    })),
    fx!("mask-blur", "Mask Blur", Matte, [f("str", "Güç", 0.0, 300.0, 15.0)], Px(|fr, v, x| from_blurred_alpha(fr, v.f("str") * x.s, |a, _, _| a))),
    fx!("motion-blur", "Motion Blur", Blur, [f("len", "Uzunluk", 0.0, 1000.0, 30.0), f("ang", "Açı", -360.0, 360.0, 30.0), f("off", "Merkez", -1.0, 1.0, 0.0)], Px(|fr, v, x| {
        let (l, a, o) = (v.f("len") * x.s, v.f("ang").to_radians(), v.f("off"));
        let (dx, dy) = (a.cos() * l, a.sin() * l);
        fr.smear((l as usize).clamp(4, 64), |u, px, py| (px + dx * (u + o * 0.5), py + dy * (u + o * 0.5)));
    })),
    fx!("spin-blur", "Spin Blur", Blur, [f("deg", "Açı", 0.0, 360.0, 15.0), p("c", "Merkez kaydır", 0.0, 0.0)], Px(|fr, v, x| {
        let (a, o) = (v.f("deg").to_radians(), v.p("c"));
        let (cx, cy) = (x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s);
        fr.smear(48, |u, px, py| {
            let (s, c) = (a * u).sin_cos();
            let (rx, ry) = (px - cx, py - cy);
            (cx + rx * c - ry * s, cy + rx * s + ry * c)
        });
    })),
    fx!("zoom-blur", "Zoom Blur", Blur, [f("str", "Güç", 0.0, 2.0, 0.2), p("c", "Merkez kaydır", 0.0, 0.0)], Px(|fr, v, x| {
        let (k, o) = (v.f("str"), v.p("c"));
        let (cx, cy) = (x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s);
        fr.smear(48, |u, px, py| {
            let z = (1.0 + k * u).max(0.01);
            (cx + (px - cx) * z, cy + (py - cy) * z)
        });
    })),
    fx!("sharpen", "Sharpen", Blur, [f("amt", "Miktar", 0.0, 5.0, 1.0)], Px(|fr, v, _| {
        let k = v.f("amt");
        let s = fr.clone();
        fr.shade(|x, y, p| {
            let (ix, iy) = (x as i32, y as i32);
            let n = [s.get_clamp(ix - 1, iy), s.get_clamp(ix + 1, iy), s.get_clamp(ix, iy - 1), s.get_clamp(ix, iy + 1)];
            let o: Px = std::array::from_fn(|c| if c == 3 { p[3] } else { (p[c] + k * (4.0 * p[c] - n[0][c] - n[1][c] - n[2][c] - n[3][c])).clamp(0.0, p[3]) });
            o
        });
    })),
    fx!("unsharp-mask", "Unsharp Mask", Blur, [f("amt", "Miktar", 0.0, 5.0, 1.0), f("r", "Yarıçap", 0.0, 100.0, 4.0), f("th", "Eşik", 0.0, 1.0, 0.0)], Px(|fr, v, x| {
        let (k, t) = (v.f("amt"), v.f("th"));
        let mut b = fr.clone();
        b.gauss(v.f("r") * x.s, v.f("r") * x.s);
        fr.shade(|px, py, p| {
            let q = b.sample(px, py);
            std::array::from_fn(|c| if c == 3 || (p[c] - q[c]).abs() < t { p[c] } else { (p[c] + (p[c] - q[c]) * k).clamp(0.0, p[3]) })
        });
    })),
    // --- Edges and mattes
    fx!("feather", "Feather", Matte, [f("r", "Yarıçap", 0.0, 300.0, 15.0)], Px(|fr, v, x| {
        // Fade inward only: the result never grows past the original edge.
        let orig = fr.clone();
        let r = v.f("r") * x.s;
        let mut b = fr.clone();
        b.gauss(r, r);
        fr.shade(|px, py, _| {
            let o = orig.sample(px, py);
            let k = ((b.sample(px, py)[3] - 0.5) * 2.0).clamp(0.0, 1.0);
            o.map(|c| c * k)
        });
    })),
    fx!("smooth-edges", "Smooth Edges", Matte, [f("r", "Yarıçap", 0.0, 100.0, 4.0), f("sharp", "Keskinlik", 0.0, 0.5, 0.25)], Px(|fr, v, x| {
        let s = v.f("sharp");
        from_blurred_alpha(fr, v.f("r") * x.s, |a, _, _| smooth(0.5 - s, 0.5 + s, a));
    })),
    fx!("matte-choker", "Matte Choker", Matte, [f("choke", "Daralt", -1.0, 1.0, 0.3), f("r", "Yumuşatma", 0.0, 100.0, 4.0)], Px(|fr, v, x| {
        let k = v.f("choke") * 0.45;
        from_blurred_alpha(fr, v.f("r") * x.s, |a, _, _| smooth(0.45 + k, 0.55 + k, a));
    })),
    fx!("roughen-edges", "Roughen Edges", Matte, [f("amt", "Miktar", 0.0, 1.0, 0.5), f("size", "Boyut", 1.0, 300.0, 18.0), f("r", "Kenar", 0.0, 100.0, 6.0), f("seed", "Tohum", 0.0, 100.0, 0.0)], Px(|fr, v, x| {
        let (k, sz, sd) = (v.f("amt") * 0.45, (v.f("size") * x.s).max(1.0), v.i("seed") as u32);
        from_blurred_alpha(fr, v.f("r") * x.s + 0.5, |a, px, py| {
            let n = fbm(px / sz, py / sz, 3, sd) - 0.5;
            smooth(0.48, 0.52, a + n * k * 2.0)
        });
    })),
    fx!("diffusion-plus", "Diffusion+", Blur, [f("r", "Yayılım", 0.0, 200.0, 12.0), f("amt", "Karışım", 0.0, 1.0, 0.6), f("grain", "Dağılım", 0.0, 1.0, 0.5), f("seed", "Tohum", 0.0, 100.0, 0.0)], Px(|fr, v, x| {
        // Each pixel takes a random-offset sample, then blends a soft copy on top.
        let (r, k, g) = (v.f("r") * x.s, v.f("amt"), v.f("grain"));
        let sd = v.i("seed") as u32 + (x.t * 12.0) as u32;
        let src = fr.clone();
        let mut soft = fr.clone();
        soft.gauss(r, r);
        fr.shade(|px, py, _| {
            let (ix, iy) = (px as i32, py as i32);
            let (a, q) = (hash2(ix, iy, sd) * TAU, hash2(ix, iy, sd + 1) * r * g);
            let d = src.sample(px + a.cos() * q, py + a.sin() * q);
            let s = soft.sample(px, py);
            mix4(d, std::array::from_fn(|c| s[c] + d[c] - s[c] * d[c]), k)
        });
    })),
    // --- Light
    fx!("glow", "Glow", Light, [f("r", "Yarıçap", 0.0, 500.0, 30.0), f("str", "Güç", 0.0, 5.0, 1.0), c("c", "Renk", [1.0, 0.9, 0.5, 1.0]), b("own", "Katman rengini kullan", true)], Px(|fr, v, x| {
        let mut g = fr.clone();
        let r = v.f("r") * x.s;
        g.gauss(r, r);
        let tint = if v.b("own") { None } else { Some(v.rgb("c")) };
        let top = fr.clone();
        screen_add(fr, &g, v.f("str"), tint);
        fr.add_over(&top);
    })),
    fx!("soft-glow", "Soft Glow", Light, [f("r", "Yarıçap", 0.0, 500.0, 25.0), f("str", "Güç", 0.0, 3.0, 0.7), f("th", "Eşik", 0.0, 1.0, 0.3)], Px(|fr, v, x| {
        let mut g = bright_pass(fr, v.f("th"));
        g.gauss(v.f("r") * x.s, v.f("r") * x.s);
        screen_add(fr, &g, v.f("str"), None);
    })),
    fx!("light-glow", "Light Glow", Light, [f("r", "Yarıçap", 0.0, 500.0, 18.0), f("str", "Güç", 0.0, 5.0, 1.5), f("th", "Eşik", 0.0, 1.0, 0.5), c("c", "Renk", [1.0, 1.0, 1.0, 1.0])], Px(|fr, v, x| {
        // Tight core plus wide halo.
        let k = v.rgb("c");
        let bright = bright_pass(fr, v.f("th"));
        for (m, w) in [(1.0, 0.6), (3.0, 0.3), (8.0, 0.15)] {
            let mut g = bright.clone();
            g.gauss(v.f("r") * x.s * m, v.f("r") * x.s * m);
            g.shade(|_, _, p| [p[0] * k[0], p[1] * k[1], p[2] * k[2], p[3]]);
            screen_add(fr, &g, v.f("str") * w, None);
        }
    })),
    fx!("edge-glow", "Edge Glow", Light, [f("w", "Kenar kalınlığı", 0.5, 50.0, 3.0), f("r", "Yarıçap", 0.0, 300.0, 18.0), f("str", "Güç", 0.0, 5.0, 1.5), c("c", "Renk", [0.3, 0.8, 1.0, 1.0])], Px(|fr, v, x| {
        let mut g = edges(fr, v.f("w") * x.s);
        g.gauss(v.f("r") * x.s, v.f("r") * x.s);
        screen_add(fr, &g, v.f("str"), Some(v.rgb("c")));
    })),
    fx!("inner-glow", "Inner Glow", Light, [f("r", "Yarıçap", 0.0, 300.0, 20.0), f("str", "Güç", 0.0, 3.0, 1.0), c("c", "Renk", [1.0, 0.8, 0.4, 1.0])], Px(|fr, v, x| {
        let k = v.rgb("c");
        let s = v.f("str");
        let mut b = fr.clone();
        b.gauss(v.f("r") * x.s, v.f("r") * x.s);
        fr.shade(|px, py, p| {
            let m = ((1.0 - b.sample(px, py)[3]) * s * 2.0).clamp(0.0, 1.0);
            premul(mix3(unpremul(p), k, m), p[3])
        });
    })),
    fx!("dark-glow", "Dark Glow", Light, [f("r", "Yarıçap", 0.0, 500.0, 25.0), f("str", "Güç", 0.0, 3.0, 1.0), c("c", "Renk", [0.0, 0.0, 0.0, 1.0])], Px(|fr, v, x| {
        let (k, s) = (v.rgb("c"), v.f("str"));
        let mut g = fr.clone();
        g.gauss(v.f("r") * x.s, v.f("r") * x.s);
        fr.shade(|px, py, p| over(p, premul(k, (g.sample(px, py)[3] * s).min(1.0))));
    })),
    fx!("glow-scan", "Glow Scan", Light, [f("speed", "Hız", -5.0, 5.0, 0.5), f("w", "Genişlik", 0.01, 1.0, 0.15), f("str", "Güç", 0.0, 5.0, 1.5), f("ang", "Eğim", -80.0, 80.0, 15.0), c("c", "Renk", [1.0, 1.0, 1.0, 1.0]), f("off", "Ofset", 0.0, 1.0, 0.25)], Px(|fr, v, x| {
        let (w, s, k) = (v.f("w"), v.f("str"), v.rgb("c"));
        let tan = v.f("ang").to_radians().tan();
        let pos = (x.t * v.f("speed") + v.f("off")).rem_euclid(1.0) * 1.4 - 0.2;
        let (fw, fh) = (fr.w as f32, fr.h as f32);
        fr.shade(|px, py, p| {
            let d = ((px + (py - fh / 2.0) * tan) / fw - pos).abs();
            let g = (1.0 - d / w).max(0.0).powi(2) * s * p[3];
            [p[0] + k[0] * g, p[1] + k[1] * g, p[2] + k[2] * g, p[3]]
        });
    })),
    fx!("electric-edges", "Electric Edges", Light, [f("w", "Kalınlık", 0.5, 40.0, 3.0), f("amp", "Sapma", 0.0, 100.0, 10.0), f("str", "Güç", 0.0, 5.0, 2.0), f("rate", "Hız", 0.0, 30.0, 10.0), c("c", "Renk", [0.4, 0.7, 1.0, 1.0])], Px(|fr, v, x| {
        let (a, sd, sc) = (v.f("amp") * x.s, (x.t * v.f("rate")) as u32, 0.04 / x.s.max(0.1));
        let e = edges(fr, v.f("w") * x.s);
        let mut g = e.clone();
        g.shade(|px, py, _| {
            let dx = (fbm(px * sc, py * sc, 3, sd) - 0.5) * a * 2.0;
            let dy = (fbm(px * sc, py * sc, 3, sd + 9) - 0.5) * a * 2.0;
            e.sample(px + dx, py + dy)
        });
        let mut halo = g.clone();
        halo.gauss(6.0 * x.s, 6.0 * x.s);
        let (k, s) = (v.rgb("c"), v.f("str"));
        screen_add(fr, &g, s, Some([1.0, 1.0, 1.0]));
        screen_add(fr, &halo, s, Some(k));
    })),
    fx!("long-shadow", "Long Shadow", Light, [f("ang", "Açı", -360.0, 360.0, 45.0), f("len", "Uzunluk", 0.0, 2000.0, 200.0), c("c", "Renk", [0.0, 0.0, 0.0, 0.6]), f("fade", "Solma", 0.0, 1.0, 0.5)], Px(|fr, v, x| {
        let (a, l, k, fd) = (v.f("ang").to_radians(), v.f("len") * x.s, v.c("c"), v.f("fade"));
        let (dx, dy) = (a.cos(), a.sin());
        let src = fr.clone();
        let n = (l as usize).clamp(1, 256);
        fr.shade(|px, py, p| {
            let mut m: f32 = 0.0;
            for i in 1..=n {
                let u = i as f32 / n as f32;
                let q = src.sample(px - dx * l * u, py - dy * l * u)[3] * (1.0 - fd * u);
                m = m.max(q);
            }
            over(p, premul([k[0], k[1], k[2]], k[3] * m))
        });
    })),
    fx!("radial-shadow", "Radial Shadow", Light, [p("l", "Işık konumu", 0.0, -300.0), f("dist", "Uzaklık", 0.0, 2.0, 0.25), f("blur", "Bulanıklık", 0.0, 200.0, 12.0), c("c", "Renk", [0.0, 0.0, 0.0, 0.6])], Px(|fr, v, x| {
        let (o, d, k) = (v.p("l"), v.f("dist"), v.c("c"));
        let (lx, ly) = (x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s);
        let mut s = fr.clone();
        s.gauss(v.f("blur") * x.s, v.f("blur") * x.s);
        let src = s;
        fr.shade(|px, py, p| {
            let z = 1.0 / (1.0 + d);
            let a = src.sample(lx + (px - lx) * z, ly + (py - ly) * z)[3];
            over(p, premul([k[0], k[1], k[2]], k[3] * a))
        });
    })),
    fx!("lens-flare", "Lens Flare", Light, [p("l", "Konum", -200.0, -150.0), f("str", "Güç", 0.0, 3.0, 1.0), f("size", "Boyut", 0.1, 5.0, 1.0), c("c", "Renk", [1.0, 0.85, 0.65, 1.0])], Px(|fr, v, x| {
        let (o, k, sz, col) = (v.p("l"), v.f("str"), v.f("size") * fr.h as f32, v.rgb("c"));
        let (lx, ly) = (x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s);
        let (cx, cy) = (fr.w as f32 / 2.0, fr.h as f32 / 2.0);
        // Ghost discs sit on the line through the light and the frame center.
        let ghosts = [(-0.4, 0.05, 0.25), (0.3, 0.03, 0.3), (0.65, 0.08, 0.18), (1.2, 0.12, 0.12), (1.6, 0.05, 0.25)];
        fr.shade(|px, py, p| {
            let (dx, dy) = ((px - lx) / sz, (py - ly) / sz);
            let d = (dx * dx + dy * dy).sqrt();
            let mut g = 0.04 / (d + 0.02) * (1.0 - d.min(1.0)).powi(2);
            g += (1.0 - (dy * 40.0).abs()).max(0.0) * (1.0 - (dx * 1.5).abs()).max(0.0) * 0.5;
            let mut out = col.map(|c| c * g);
            for (t, r, a) in ghosts {
                let (gx, gy) = (lx + (cx - lx) * (1.0 + t), ly + (cy - ly) * (1.0 + t));
                let e = ((px - gx).hypot(py - gy) / sz) / r;
                let m = (1.0 - smooth(0.85, 1.0, e)) * a * (0.5 + 0.5 * e);
                let h = hsv_to_rgb([t * 0.5 + 0.1, 0.5, 1.0]);
                for i in 0..3 { out[i] += h[i] * m; }
            }
            let a = (out[0].max(out[1]).max(out[2]) * k).min(1.0);
            let glow: Px = [out[0] * k, out[1] * k, out[2] * k, a];
            std::array::from_fn(|c| if c == 3 { (p[3] + a * (1.0 - p[3])).min(1.0) } else { (p[c] + glow[c]).min(1.0) })
        });
    })),
    fx!("rays", "Rays", Light, [f("len", "Uzunluk", 0.0, 1000.0, 200.0), f("str", "Güç", 0.0, 5.0, 1.0), f("th", "Eşik", 0.0, 1.0, 0.2), p("c", "Merkez kaydır", 0.0, 0.0)], Px(|fr, v, x| {
        let o = v.p("c");
        streak_rays(fr, 2, v.f("len") * x.s, [x.c[0] + o[0] * x.s, x.c[1] + o[1] * x.s], 0.0, v.f("str"), v.f("th"));
    })),
    fx!("radial-rays", "Radial Rays", Light, [f("n", "Işın sayısı", 2.0, 64.0, 12.0), f("sharp", "Keskinlik", 0.5, 16.0, 4.0), f("str", "Güç", 0.0, 3.0, 0.8), f("rot", "Dönme", -360.0, 360.0, 0.0), f("speed", "Dönüş hızı", -2.0, 2.0, 0.1), c("c", "Renk", [1.0, 0.95, 0.7, 1.0]), f("r", "Yarıçap", 0.05, 3.0, 1.0)], Px(|fr, v, x| {
        let (n, sh, k, col, r) = (v.f("n"), v.f("sharp"), v.f("str"), v.rgb("c"), v.f("r") * fr.h as f32);
        let rot = v.f("rot").to_radians() + x.t * v.f("speed") * TAU;
        let c = x.c;
        fr.shade(|px, py, p| {
            let (dx, dy) = (px - c[0], py - c[1]);
            let a = dy.atan2(dx) + rot;
            let m = ((a * n / 2.0).sin().abs()).powf(sh) * k * (1.0 - dx.hypot(dy) / r).max(0.0);
            std::array::from_fn(|i| if i == 3 { (p[3] + m * (1.0 - p[3])).min(1.0) } else { (p[i] + col[i] * m).min(1.0) })
        });
    })),
    fx!("linear-streaks", "Linear Streaks", Light, [f("len", "Uzunluk", 0.0, 1000.0, 150.0), f("ang", "Açı", -360.0, 360.0, 0.0), f("str", "Güç", 0.0, 5.0, 1.0), f("th", "Eşik", 0.0, 1.0, 0.3)], Px(|fr, v, x| streak_rays(fr, 0, v.f("len") * x.s, x.c, v.f("ang"), v.f("str"), v.f("th")))),
    fx!("spin-streaks", "Spin Streaks", Light, [f("len", "Uzunluk", 0.0, 1000.0, 60.0), f("str", "Güç", 0.0, 5.0, 1.0), f("th", "Eşik", 0.0, 1.0, 0.3)], Px(|fr, v, x| streak_rays(fr, 1, v.f("len"), x.c, 0.0, v.f("str"), v.f("th")))),
    fx!("zoom-streaks", "Zoom Streaks", Light, [f("len", "Uzunluk", 0.0, 1000.0, 200.0), f("str", "Güç", 0.0, 5.0, 1.0), f("th", "Eşik", 0.0, 1.0, 0.3)], Px(|fr, v, x| streak_rays(fr, 2, v.f("len"), x.c, 0.0, v.f("str"), v.f("th")))),
    fx!("lightning", "Lightning", Light, [p("a", "Başlangıç", 0.0, -300.0), p("b", "Bitiş", 0.0, 300.0), f("w", "Kalınlık", 0.2, 30.0, 2.5), f("amp", "Kırılma", 0.0, 1.0, 0.25), f("rate", "Hız", 0.0, 30.0, 10.0), c("c", "Renk", [0.5, 0.7, 1.0, 1.0]), f("branch", "Dal", 0.0, 1.0, 0.5), f("seed", "Tohum", 0.0, 100.0, 0.0)], Px(|fr, v, x| {
        let (a, b) = (v.p("a"), v.p("b"));
        let (ax, ay) = (x.c[0] + a[0] * x.s, x.c[1] + a[1] * x.s);
        let (bx, by) = (x.c[0] + b[0] * x.s, x.c[1] + b[1] * x.s);
        let (w, amp, col) = (v.f("w") * x.s, v.f("amp"), v.rgb("c"));
        let sd = (x.t * v.f("rate")) as u32 + v.i("seed") as u32 * 31;
        let mut segs: Vec<([f32; 2], [f32; 2], f32)> = vec![];
        let mut stack = vec![([ax, ay], [bx, by], amp, 7u32, 1.0f32, 1u32)];
        let branch = v.f("branch");
        while let Some((s, e, am, depth, wd, id)) = stack.pop() {
            if depth == 0 || segs.len() > 400 {
                segs.push((s, e, wd));
                continue;
            }
            let (dx, dy) = (e[0] - s[0], e[1] - s[1]);
            let r = hash2(id as i32, depth as i32, sd) - 0.5;
            let m = [(s[0] + e[0]) / 2.0 - dy * r * am, (s[1] + e[1]) / 2.0 + dx * r * am];
            stack.push((s, m, am, depth - 1, wd, id * 2));
            stack.push((m, e, am, depth - 1, wd, id * 2 + 1));
            if depth > 3 && hash2(id as i32, 77, sd) < branch * 0.25 {
                let side = hash2(id as i32, 78, sd) - 0.5;
                let t = [m[0] + dx * 0.5 - dy * side, m[1] + dy * 0.5 + dx * side];
                stack.push((m, t, am, depth - 2, wd * 0.5, id * 4 + 3));
            }
        }
        let (fw, fh) = (fr.w, fr.h);
        let mut field = vec![0.0f32; fw * fh];
        let halo = (w * 6.0).max(3.0);
        for (s, e, wd) in &segs {
            let (x0, x1) = ((s[0].min(e[0]) - halo).max(0.0) as usize, ((s[0].max(e[0]) + halo) as usize + 1).min(fw));
            let (y0, y1) = ((s[1].min(e[1]) - halo).max(0.0) as usize, ((s[1].max(e[1]) + halo) as usize + 1).min(fh));
            let (dx, dy) = (e[0] - s[0], e[1] - s[1]);
            let l2 = (dx * dx + dy * dy).max(1e-4);
            for py in y0..y1 {
                for px in x0..x1 {
                    let (qx, qy) = (px as f32 + 0.5 - s[0], py as f32 + 0.5 - s[1]);
                    let t = ((qx * dx + qy * dy) / l2).clamp(0.0, 1.0);
                    let d = (qx - dx * t).hypot(qy - dy * t);
                    let g = (w * wd / (d + w * wd * 0.5)).powi(2).min(1.5) * (1.0 - d / halo).max(0.0);
                    let cell = &mut field[py * fw + px];
                    *cell = cell.max(g);
                }
            }
        }
        for (i, q) in fr.px.iter_mut().enumerate() {
            let g = field[i];
            if g > 0.0 {
                let core = (g - 0.7).max(0.0);
                for c in 0..3 { q[c] = (q[c] + col[c] * g + core).min(1.0); }
                q[3] = (q[3] + g.min(1.0) * (1.0 - q[3])).min(1.0);
            }
        }
    })),
    // --- Stylize
    fx!("mosaic", "Mosaic", Stylize, [f("nx", "Yatay blok", 1.0, 400.0, 40.0), f("ny", "Dikey blok", 1.0, 400.0, 24.0)], Px(|fr, v, _| {
        let (bw, bh) = (fr.w as f32 / v.f("nx").max(1.0), fr.h as f32 / v.f("ny").max(1.0));
        let s = fr.clone();
        fr.shade(|x, y, _| s.sample(((x / bw).floor() + 0.5) * bw, ((y / bh).floor() + 0.5) * bh));
    })),
    fx!("pixelate", "Pixelate", Stylize, [f("size", "Piksel boyutu", 1.0, 300.0, 16.0)], Px(|fr, v, x| {
        let b = (v.f("size") * x.s).max(1.0);
        let s = fr.clone();
        fr.shade(|px, py, _| {
            // Average a few samples so the block color is stable while the source moves.
            let (cx, cy) = ((px / b).floor() * b, (py / b).floor() * b);
            let mut o = [0.0; 4];
            for (u, w) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let q = s.sample(cx + u * b, cy + w * b);
                for c in 0..4 { o[c] += q[c] * 0.25; }
            }
            o
        });
    })),
    fx!("halftone-dots", "Halftone Dots", Stylize, [f("size", "Boyut", 2.0, 200.0, 12.0), f("ang", "Açı", -360.0, 360.0, 45.0), f("str", "Güç", 0.0, 1.0, 1.0)], Px(|fr, v, x| halftone(fr, v.f("size") * x.s, v.f("ang"), v.f("str"), false, 0.0))),
    fx!("halftone-lines", "Halftone Lines", Stylize, [f("size", "Boyut", 2.0, 200.0, 10.0), f("ang", "Açı", -360.0, 360.0, 30.0), f("str", "Güç", 0.0, 1.0, 1.0)], Px(|fr, v, x| halftone(fr, v.f("size") * x.s, v.f("ang"), v.f("str"), true, 0.0))),
    fx!("cmyk-halftone-dots", "CMYK Halftone Dots", Stylize, [f("amt", "Miktar", 1.0, 500.0, 100.0), f("str", "Güç", 0.0, 1.0, 0.5), f("k", "Siyah", 0.0, 1.0, 0.0), f("ang", "Açı", 0.0, 3600.0, 45.0), f("ph", "Evre", -1000.0, 1000.0, 0.5)], Px(|fr, v, _| {
        let cell = (fr.h as f32 / v.f("amt").max(1.0)).max(1.5);
        let (st, kk, ang, ph) = (v.f("str"), v.f("k"), v.f("ang").to_radians(), v.f("ph"));
        fr.shade(|x, y, p| {
            let c = unpremul(p);
            let k = (1.0 - c[0].max(c[1]).max(c[2])) * kk;
            let inks = [1.0 - c[0] - k, 1.0 - c[1] - k, 1.0 - c[2] - k, k];
            let mut o = [1.0, 1.0, 1.0];
            for (i, offs) in [0.26f32, 0.0, 0.52, 0.78].iter().enumerate() {
                let (s, cs) = (ang + offs).sin_cos();
                let (u, w) = ((x * cs + y * s) / cell + ph, (-x * s + y * cs) / cell + ph);
                let d = ((u.rem_euclid(1.0) - 0.5).powi(2) + (w.rem_euclid(1.0) - 0.5).powi(2)).sqrt() * 1.41;
                let m = 1.0 - smooth(-0.08, 0.08, d - inks[i].max(0.0).sqrt() * 0.95);
                match i {
                    0 => o[0] *= 1.0 - m,
                    1 => o[1] *= 1.0 - m,
                    2 => o[2] *= 1.0 - m,
                    _ => o = o.map(|q| q * (1.0 - m)),
                }
            }
            premul(mix3(c, o, st.max(0.0) * 2.0_f32.min(1.0)), p[3])
        });
    })),
    fx!("rgb-split", "RGB Split", Stylize, [f("dist", "Mesafe", 0.0, 500.0, 12.0), f("ang", "Açı", -360.0, 360.0, 0.0), ch("mode", "Yön", &["Doğrusal", "Dairesel"], 0)], Px(|fr, v, x| {
        let (d, a, m, c) = (v.f("dist") * x.s, v.f("ang").to_radians(), v.i("mode"), x.c);
        let (dx, dy) = (a.cos() * d, a.sin() * d);
        let s = fr.clone();
        let diag = (fr.w as f32).hypot(fr.h as f32) * 0.5;
        fr.shade(|px, py, _| {
            let (ox, oy) = if m == 1 {
                let (rx, ry) = (px - c[0], py - c[1]);
                (rx / diag * d, ry / diag * d)
            } else { (dx, dy) };
            let (r, g, b) = (s.sample(px + ox, py + oy), s.sample(px, py), s.sample(px - ox, py - oy));
            [r[0], g[1], b[2], r[3].max(g[3]).max(b[3])]
        });
    })),
    fx!("find-edges", "Find Edges", Stylize, [f("str", "Güç", 0.0, 10.0, 2.0), b("inv", "Tersine çevir", false), f("mix", "Karışım", 0.0, 1.0, 1.0)], Px(|fr, v, _| {
        let (st, inv, k) = (v.f("str"), v.b("inv"), v.f("mix"));
        let s = fr.clone();
        fr.shade(|x, y, p| {
            let (ix, iy) = (x as i32, y as i32);
            let l = |a: i32, b: i32| { let q = s.get_clamp(ix + a, iy + b); luma([q[0], q[1], q[2]]) };
            let gx = l(1, -1) + 2.0 * l(1, 0) + l(1, 1) - l(-1, -1) - 2.0 * l(-1, 0) - l(-1, 1);
            let gy = l(-1, 1) + 2.0 * l(0, 1) + l(1, 1) - l(-1, -1) - 2.0 * l(0, -1) - l(1, -1);
            let e = ((gx * gx + gy * gy).sqrt() * st).min(1.0);
            let e = if inv { 1.0 - e } else { e };
            premul(mix3(unpremul(p), [e, e, e], k), p[3])
        });
    })),
    fx!("contour-gradient", "Contour Gradient", Stylize, [f("r", "Genişlik", 1.0, 500.0, 40.0), c("a", "Dış", [1.0, 0.2, 0.5, 1.0]), c("b", "İç", [0.2, 0.6, 1.0, 1.0])], Px(|fr, v, x| {
        let (a, b) = (v.rgb("a"), v.rgb("b"));
        let mut d = fr.clone();
        d.gauss(v.f("r") * x.s, v.f("r") * x.s);
        let src = fr.clone();
        fr.shade(|px, py, p| premul(mix3(a, b, smooth(0.5, 1.0, contour_value(&src, px, py, &d))), p[3]));
    })),
    fx!("contour-lines", "Contour Lines", Stylize, [f("r", "Yumuşatma", 1.0, 500.0, 40.0), f("n", "Çizgi sayısı", 1.0, 40.0, 8.0), f("w", "Kalınlık", 0.01, 0.5, 0.15), c("c", "Renk", [1.0, 1.0, 1.0, 1.0])], Px(|fr, v, x| {
        let (n, w, k) = (v.f("n"), v.f("w"), v.c("c"));
        let mut d = fr.clone();
        d.gauss(v.f("r") * x.s, v.f("r") * x.s);
        fr.shade(|px, py, p| {
            let a = d.sample(px, py)[3];
            let f = (a * n).fract();
            let line = if a > 0.01 { 1.0 - smooth(w * 0.5, w, (f - 0.5).abs()) } else { 0.0 };
            over(premul([k[0], k[1], k[2]], k[3] * line), p)
        });
    })),
    fx!("contour-strips", "Contour Strips", Stylize, [f("r", "Yumuşatma", 1.0, 500.0, 40.0), f("n", "Şerit", 1.0, 40.0, 6.0), c("a", "Renk 1", [1.0, 0.8, 0.2, 1.0]), c("b", "Renk 2", [0.1, 0.2, 0.5, 1.0])], Px(|fr, v, x| {
        let (n, a, b) = (v.f("n"), v.rgb("a"), v.rgb("b"));
        let mut d = fr.clone();
        d.gauss(v.f("r") * x.s, v.f("r") * x.s);
        fr.shade(|px, py, p| {
            let q = d.sample(px, py)[3] * n;
            premul(if (q.floor() as i32) % 2 == 0 { a } else { b }, p[3])
        });
    })),
    fx!("smooth-bevel", "Smooth Bevel", Stylize, [f("r", "Genişlik", 0.5, 200.0, 10.0), f("str", "Güç", 0.0, 5.0, 1.5), f("ang", "Işık açısı", -360.0, 360.0, -45.0)], Px(|fr, v, x| {
        let (st, a) = (v.f("str"), v.f("ang").to_radians());
        let (lx, ly) = (a.cos(), a.sin());
        let r = v.f("r") * x.s;
        let mut h = fr.clone();
        h.gauss(r, r);
        let e = r.max(1.0);
        fr.shade(|px, py, p| {
            let dx = h.sample(px + e, py)[3] - h.sample(px - e, py)[3];
            let dy = h.sample(px, py + e)[3] - h.sample(px, py - e)[3];
            let d = (dx * lx + dy * ly) * st;
            let c = unpremul(p);
            premul(if d > 0.0 { mix3(c, [1.0; 3], d.min(1.0)) } else { mix3(c, [0.0; 3], (-d).min(1.0)) }, p[3])
        });
    })),
    fx!("bump-map", "Bump Map", Stylize, [ch("src", "Doku", &["Katman parlaklığı", "Arka plan parlaklığı", "Kanvas", "Deri", "Çamur", "Parşömen", "Ahşap", "Örgü"], 2), f("scale", "Ölçek", 0.01, 10.0, 1.0), f("h", "Yükseklik", -1.0, 1.0, 0.1), f("amb", "Ortam", 0.0, 1.0, 0.6), f("int", "Yoğunluk", 0.0, 2.0, 1.0), p("dir", "Işık yönü", 75.0, -75.0)], Px(|fr, v, x| {
        let (mode, sc, ht, amb, it, d) = (v.i("src"), v.f("scale").max(0.01) * x.s, v.f("h") * 12.0, v.f("amb"), v.f("int"), v.p("dir"));
        let bg = x.bg;
        let src = fr.clone();
        let l = d[0].hypot(d[1]).max(1.0);
        let (lx, ly) = (d[0] / l, d[1] / l);
        let height = |px: f32, py: f32| -> f32 {
            let (u, w) = (px / sc, py / sc);
            match mode {
                0 => { let q = src.sample_clamp(px, py); luma([q[0], q[1], q[2]]) }
                1 => { let q = bg.sample_clamp(px, py); luma([q[0], q[1], q[2]]) }
                2 => ((u * 0.7).sin() * (w * 0.7).sin()) * 0.25 + fbm(u * 0.5, w * 0.5, 2, 3) * 0.4,
                3 => { let n = fbm(u * 0.04, w * 0.04, 4, 5); (n * 14.0).sin().abs().powf(0.4) }
                4 => fbm(u * 0.02, w * 0.02, 5, 9),
                5 => fbm(u * 0.015, w * 0.015, 5, 2) * 0.7 + fbm(u * 0.3, w * 0.3, 2, 7) * 0.1,
                6 => ((u * 0.01 + fbm(u * 0.005, w * 0.05, 3, 4) * 3.0) * 30.0).sin() * 0.5 + 0.5,
                _ => { let (a, b) = ((u * 0.5).sin(), (w * 0.5).sin()); (a * b).abs() }
            }
        };
        fr.shade(|px, py, p| {
            let e = 1.0;
            let dx = height(px + e, py) - height(px - e, py);
            let dy = height(px, py + e) - height(px, py - e);
            let n = (dx * lx + dy * ly) * ht;
            let sh = (amb + (0.5 - n) * (1.0 - amb) * it).max(0.0);
            p_scale(p, sh)
        });
    })),
    fx!("noise", "Noise", Stylize, [f("amt", "Miktar", 0.0, 1.0, 0.2), b("mono", "Tek renk", true), b("anim", "Hareketli", true), f("size", "Tane", 1.0, 16.0, 1.0)], Px(|fr, v, x| {
        let (a, mono, sz) = (v.f("amt"), v.b("mono"), v.f("size").max(1.0));
        let sd = if v.b("anim") { (x.t * x.fps) as u32 } else { 0 };
        fr.shade(|px, py, p| {
            let (ix, iy) = ((px / sz) as i32, (py / sz) as i32);
            let r = |k: u32| (hash2(ix, iy, sd * 3 + k) - 0.5) * 2.0 * a;
            let n = if mono { [r(0); 3] } else { [r(0), r(1), r(2)] };
            std::array::from_fn(|i| if i == 3 { p[3] } else { (p[i] + n[i] * p[3]).clamp(0.0, p[3]) })
        });
    })),
    fx!("dissolve", "Dissolve", Matte, [f("str", "Güç", 0.0, 1.0, 0.75), f("seed", "Tohum", 0.0, 5.0, 0.0)], Px(|fr, v, _| {
        // Strength is how much of the layer remains cut away; 0 leaves it whole.
        let (k, sd) = (v.f("str"), v.f("seed") as u32 * 17 + 1);
        dissolve_mask(fr, |x, y| if hash2(x as i32, y as i32, sd) < k { 0.0 } else { 1.0 });
    })),
    fx!("block-dissolve", "Block Dissolve", Matte, [f("str", "Güç", 0.0, 1.0, 0.75), f("size", "Boyut", 1.0, 500.0, 10.0), p("stretch", "Esnet", 1.0, 1.0), f("seed", "Tohum", 0.0, 5.0, 0.0), f("fade", "Solma", 0.0, 0.5, 0.0), p("off", "Ofset", 0.0, 0.0)], Px(|fr, v, x| {
        let (k, sz, st, sd, fd, of) = (v.f("str"), (v.f("size") * x.s).max(1.0), v.p("stretch"), v.f("seed") as u32 * 19 + 1, v.f("fade"), v.p("off"));
        let (bw, bh) = (sz * st[0].abs().max(0.01), sz * st[1].abs().max(0.01));
        dissolve_mask(fr, |px, py| {
            let h = hash2(((px + of[0] * x.s) / bw).floor() as i32, ((py + of[1] * x.s) / bh).floor() as i32, sd);
            if fd > 0.0 { smooth(k - fd, k + fd, h) } else if h < k { 0.0 } else { 1.0 }
        });
    })),
    fx!("wipe", "Wipe", Matte, [f("a", "Baştan kes", 0.0, 1.0, 0.0), f("b", "Sondan kes", 0.0, 1.0, 0.0), f("ang", "Açı", -360.0, 360.0, 0.0), f("soft", "Yumuşaklık", 0.0, 1.0, 0.0), f("n", "Segment", 1.0, 64.0, 1.0)], Px(|fr, v, _| {
        let (a, bb, so, n) = (v.f("a"), v.f("b"), v.f("soft").max(1e-3), v.f("n").max(1.0));
        let r = v.f("ang").to_radians();
        let (dx, dy) = (r.cos(), r.sin());
        let ext = (fr.w as f32 * dx.abs() + fr.h as f32 * dy.abs()).max(1.0);
        let (cx, cy) = (fr.w as f32 / 2.0, fr.h as f32 / 2.0);
        dissolve_mask(fr, |x, y| {
            let u = ((x - cx) * dx + (y - cy) * dy) / ext + 0.5;
            let u = (u * n).rem_euclid(1.0);
            if a <= 0.0 && bb <= 0.0 { return 1.0; }
            smooth(a - so * 0.5, a + so * 0.5, u + so * 0.5 * (a <= 0.0) as u8 as f32) * (1.0 - smooth(1.0 - bb - so * 0.5, 1.0 - bb + so * 0.5, u - so * 0.5 * (bb <= 0.0) as u8 as f32))
        });
    })),
    fx!("radial-wipe", "Radial Wipe", Matte, [f("prog", "İlerleme", 0.0, 1.0, 0.0), f("start", "Başlangıç açısı", -360.0, 360.0, -90.0), f("soft", "Yumuşaklık", 0.0, 0.5, 0.0), b("cw", "Saat yönünde", true)], Px(|fr, v, x| {
        let (pr, st, so, cw, c) = (v.f("prog"), v.f("start").to_radians(), v.f("soft").max(1e-3), v.b("cw"), x.c);
        dissolve_mask(fr, |px, py| {
            let mut a = (py - c[1]).atan2(px - c[0]) - st;
            if !cw { a = -a }
            let u = (a / TAU).rem_euclid(1.0);
            smooth(pr, pr + so, u + so * (pr <= 0.0) as u8 as f32)
        });
    })),
    fx!("copy-background", "Copy Background", Stylize, [f("mix", "Karışım", 0.0, 1.0, 1.0), p("off", "Kaydır", 0.0, 0.0), f("blur", "Bulanıklık", 0.0, 200.0, 0.0)], Px(|fr, v, x| {
        let (k, o) = (v.f("mix"), v.p("off"));
        let mut bg = x.bg.clone();
        let b = v.f("blur") * x.s;
        if b > 0.0 { bg.gauss(b, b); }
        fr.shade(|px, py, p| {
            let q = bg.sample_clamp(px + o[0] * x.s, py + o[1] * x.s);
            mix4(p, q.map(|c| c * p[3]), k)
        });
    })),
    fx!("magnify-background", "Magnify Background", Stylize, [f("mag", "Büyütme", 0.1, 10.0, 2.0), f("mix", "Karışım", 0.0, 1.0, 1.0)], Px(|fr, v, x| {
        let (m, k, c) = (v.f("mag").max(0.01), v.f("mix"), x.c);
        let bg = x.bg;
        fr.shade(|px, py, p| {
            let q = bg.sample_clamp(c[0] + (px - c[0]) / m, c[1] + (py - c[1]) / m);
            mix4(p, q.map(|n| n * p[3]), k)
        });
    })),
    fx!("raster-extrude", "Raster Extrude", Stylize, [f("len", "Derinlik", 0.0, 500.0, 40.0), f("ang", "Açı", -360.0, 360.0, 45.0), f("shade", "Gölgeleme", 0.0, 1.0, 0.5)], Px(|fr, v, x| {
        let (l, a, sh) = (v.f("len") * x.s, v.f("ang").to_radians(), v.f("shade"));
        let (dx, dy) = (a.cos(), a.sin());
        let src = fr.clone();
        let n = (l as usize).clamp(1, 128);
        fr.shade(|px, py, p| {
            let mut o = p;
            for i in 1..=n {
                if o[3] >= 0.999 { break; }
                let u = i as f32 / n as f32;
                let q = src.sample(px - dx * l * u, py - dy * l * u);
                let k = 1.0 - sh * (0.3 + 0.7 * u) * PI / PI;
                o = over(o, [q[0] * k, q[1] * k, q[2] * k, q[3]]);
            }
            o
        });
    })),
];

fn p_scale(p: Px, k: f32) -> Px {
    [(p[0] * k).min(p[3]), (p[1] * k).min(p[3]), (p[2] * k).min(p[3]), p[3]]
}
#[allow(dead_code)]
fn unused(fr: &mut Frame) {
    alpha_levels(fr, 0.0, 1.0);
}
