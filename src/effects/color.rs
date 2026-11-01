//! Color correction, keying and matte effects.
use super::*;
use crate::frame::*;
use crate::fx;

const REMAP_RGB: &[&str] = &["R", "G", "B", "A", "Y", "0", "1"];
const REMAP_HSV: &[&str] = &["H", "S", "V", "A", "Y", "0", "1"];

fn tune_wheel(hue: f32, strength: f32, lum: f32) -> [f32; 3] {
    let c = hsv_to_rgb([hue / 360.0, 1.0, 1.0]);
    c.map(|v| (v - 0.5) * strength + lum)
}

fn key_alpha(c: [f32; 3], key: [f32; 3], th: f32, fe: f32) -> f32 {
    // Distance in a chroma-weighted space, so shadows of the key color are keyed too.
    let (a, b) = (rgb_to_hsv(c), rgb_to_hsv(key));
    let dh = (a[0] - b[0]).abs();
    let dh = dh.min(1.0 - dh) * 2.0;
    let d = (dh * dh * 3.0 + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2) * 0.25).sqrt();
    smooth(th, th + fe, d)
}

const PALETTES: &[&[[f32; 3]]] = &[
    &[[0.06, 0.22, 0.06], [0.19, 0.38, 0.19], [0.55, 0.67, 0.06], [0.61, 0.74, 0.06]],
    &[[0.0, 0.0, 0.0], [0.33, 1.0, 1.0], [1.0, 0.33, 1.0], [1.0, 1.0, 1.0]],
    &[[0.1, 0.05, 0.2], [0.5, 0.1, 0.5], [1.0, 0.3, 0.5], [1.0, 0.8, 0.4]],
    &[[0.05, 0.1, 0.2], [0.1, 0.35, 0.5], [0.4, 0.75, 0.75], [0.95, 0.95, 0.85]],
    &[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
];

pub const DEFS: &[Def] = &[
    fx!("brightness-contrast", "Brightness / Contrast", Color, [f("b", "Parlaklık", -1.0, 1.0, 0.0), f("c", "Kontrast", -1.0, 3.0, 0.0)], Px(|fr, v, _| {
        let (b, k) = (v.f("b"), v.f("c") + 1.0);
        fr.map_rgb(|c| c.map(|x| (x - 0.5) * k + 0.5 + b));
    })),
    fx!("channel-remap-hsv", "Channel Remap (HSV)", Color, [ch("h", "H", REMAP_HSV, 0), ch("s", "S", REMAP_HSV, 1), ch("v", "V", REMAP_HSV, 2), ch("a", "A", REMAP_HSV, 3)], Px(|fr, v, _| {
        let m = [v.i("h"), v.i("s"), v.i("v"), v.i("a")];
        fr.shade(|_, _, p| {
            let c = unpremul(p);
            let h = rgb_to_hsv(c);
            let g = |i: i32| match i { 0..=2 => h[i as usize], 3 => p[3], 4 => luma(c), 5 => 0.0, _ => 1.0 };
            premul(hsv_to_rgb([g(m[0]), g(m[1]), g(m[2])]), g(m[3]).clamp(0.0, 1.0))
        });
    })),
    fx!("channel-remap-rgb", "Channel Remap (RGB)", Color, [ch("r", "R", REMAP_RGB, 0), ch("g", "G", REMAP_RGB, 1), ch("b", "B", REMAP_RGB, 2), ch("a", "A", REMAP_RGB, 3)], Px(|fr, v, _| {
        let m = [v.i("r"), v.i("g"), v.i("b"), v.i("a")];
        fr.shade(|_, _, p| {
            let c = unpremul(p);
            let g = |i: i32| match i { 0..=2 => c[i as usize], 3 => p[3], 4 => luma(c), 5 => 0.0, _ => 1.0 };
            premul([g(m[0]), g(m[1]), g(m[2])], g(m[3]).clamp(0.0, 1.0))
        });
    })),
    fx!("chroma-key", "Chroma Key", Matte, [c("key", "Anahtar renk", [0.0, 1.0, 0.0, 1.0]), f("th", "Eşik", 0.0, 1.0, 0.1), f("fe", "Yumuşatma", 0.01, 0.75, 0.05), b("def", "Kenar rengini temizle", false), b("inv", "Tersine çevir", false)], Px(|fr, v, _| {
        let (k, th, fe, de, inv) = (v.rgb("key"), v.f("th"), v.f("fe"), v.b("def"), v.b("inv"));
        fr.shade(|_, _, p| {
            let mut c = unpremul(p);
            let mut a = key_alpha(c, k, th, fe);
            if inv { a = 1.0 - a }
            if de && a < 1.0 {
                // Remove the key color's contribution from semi-transparent edge pixels.
                let l = luma(c);
                c = mix3([l, l, l], c, a);
            }
            premul(c, p[3] * a)
        });
    })),
    fx!("color-temperature", "Color Temperature", Color, [f("temp", "Sıcaklık", -1.0, 1.0, 0.25), f("tint", "Ton", -1.0, 1.0, 0.0)], Px(|fr, v, _| {
        let (t, g) = (v.f("temp"), v.f("tint"));
        fr.map_rgb(|c| [c[0] * (1.0 + t * 0.4), c[1] * (1.0 - g * 0.3), c[2] * (1.0 - t * 0.4)]);
    })),
    fx!("color-tune", "Color Tune", Color, [
        f("lh", "Lift ton", 0.0, 360.0, 0.0), f("ls", "Lift güç", 0.0, 1.0, 0.0), f("ll", "Lift parlaklık", -1.0, 1.0, 0.0),
        f("gh", "Gamma ton", 0.0, 360.0, 0.0), f("gs", "Gamma güç", 0.0, 1.0, 0.0), f("gl", "Gamma parlaklık", -1.0, 1.0, 0.0),
        f("ah", "Gain ton", 0.0, 360.0, 0.0), f("as", "Gain güç", 0.0, 1.0, 0.0), f("al", "Gain parlaklık", -1.0, 1.0, 0.0),
        f("oh", "Offset ton", 0.0, 360.0, 0.0), f("os", "Offset güç", 0.0, 1.0, 0.0), f("ol", "Offset parlaklık", -1.0, 1.0, 0.0),
    ], Px(|fr, v, _| {
        let l = tune_wheel(v.f("lh"), v.f("ls"), v.f("ll"));
        let g = tune_wheel(v.f("gh"), v.f("gs"), v.f("gl"));
        let a = tune_wheel(v.f("ah"), v.f("as"), v.f("al"));
        let o = tune_wheel(v.f("oh"), v.f("os"), v.f("ol"));
        fr.map_rgb(|c| std::array::from_fn(|i| {
            let x = c[i] * (1.0 + a[i]) + l[i] * (1.0 - c[i]) + o[i];
            x.max(0.0).powf(1.0 / (1.0 + g[i]).max(0.05))
        }));
    })),
    fx!("colorize", "Colorize", Color, [f("hue", "Ton", 0.0, 360.0, 200.0), f("sat", "Doygunluk", 0.0, 1.0, 0.6), f("amt", "Miktar", 0.0, 1.0, 1.0)], Px(|fr, v, _| {
        let (h, s, a) = (v.f("hue") / 360.0, v.f("sat"), v.f("amt"));
        fr.map_rgb(|c| mix3(c, hsv_to_rgb([h, s, rgb_to_hsv(c)[2]]), a));
    })),
    fx!("exposure-gamma", "Exposure / Gamma", Color, [f("exp", "Pozlama", -5.0, 5.0, 0.5), f("gamma", "Gamma", 0.1, 5.0, 1.0), f("off", "Ofset", -1.0, 1.0, 0.0)], Px(|fr, v, _| {
        let (e, g, o) = (2f32.powf(v.f("exp")), 1.0 / v.f("gamma").max(0.05), v.f("off"));
        fr.map_rgb(|c| c.map(|x| (x * e + o).max(0.0).powf(g)));
    })),
    fx!("fill-behind", "Fill Behind", Matte, [c("c", "Renk", [0.0, 0.0, 0.0, 0.0])], Px(|fr, v, _| {
        let k = v.c("c");
        let b = premul([k[0], k[1], k[2]], k[3]);
        fr.shade(|_, _, p| over(p, b));
    })),
    fx!("gradient-map", "Gradient Map", Color, [c("a", "Koyu", [0.08, 0.02, 0.25, 1.0]), c("m", "Orta", [0.9, 0.2, 0.45, 1.0]), c("b", "Açık", [1.0, 0.9, 0.5, 1.0]), f("amt", "Miktar", 0.0, 1.0, 1.0)], Px(|fr, v, _| {
        let (a, m, b, k) = (v.rgb("a"), v.rgb("m"), v.rgb("b"), v.f("amt"));
        fr.map_rgb(|c| {
            let l = luma(c).clamp(0.0, 1.0) * 2.0;
            mix3(c, if l < 1.0 { mix3(a, m, l) } else { mix3(m, b, l - 1.0) }, k)
        });
    })),
    fx!("gradient-overlay", "Gradient Overlay", Color, [c("a", "Renk 1", [1.0, 0.3, 0.4, 1.0]), c("b", "Renk 2", [0.2, 0.4, 1.0, 1.0]), f("ang", "Açı", -360.0, 360.0, 90.0), f("amt", "Miktar", 0.0, 1.0, 1.0), f("span", "Yayılım", 0.05, 4.0, 1.0)], Px(|fr, v, _| {
        let (a, b, k, sp) = (v.rgb("a"), v.rgb("b"), v.f("amt"), v.f("span"));
        let r = v.f("ang").to_radians();
        let (dx, dy) = (r.cos(), r.sin());
        let ext = (fr.w as f32 * dx.abs() + fr.h as f32 * dy.abs()).max(1.0) * sp;
        let (cx, cy) = (fr.w as f32 / 2.0, fr.h as f32 / 2.0);
        fr.shade(|x, y, p| {
            let u = (((x - cx) * dx + (y - cy) * dy) / ext + 0.5).clamp(0.0, 1.0);
            premul(mix3(unpremul(p), mix3(a, b, u), k), p[3])
        });
    })),
    fx!("highlights-shadows", "Highlights and Shadows", Color, [f("hi", "Açık tonlar", -1.0, 1.0, 0.0), f("sh", "Koyu tonlar", -1.0, 1.0, 0.0)], Px(|fr, v, _| {
        let (h, s) = (v.f("hi"), v.f("sh"));
        fr.map_rgb(|c| {
            let l = luma(c).clamp(0.0, 1.0);
            let d = h * smooth(0.4, 1.0, l) * 0.5 + s * (1.0 - smooth(0.0, 0.6, l)) * 0.5;
            c.map(|x| x + d)
        });
    })),
    fx!("hot-color", "Hot Color", Color, [f("max", "En yüksek parlaklık", 0.1, 1.0, 0.7), f("chroma", "En yüksek doygunluk", 0.1, 1.0, 0.6)], Px(|fr, v, _| {
        let (m, cm) = (v.f("max"), v.f("chroma"));
        fr.map_rgb(|c| {
            // Pull out-of-range broadcast colors back toward a legal level.
            let mut h = rgb_to_hsv(c);
            h[1] = h[1].min(cm);
            h[2] = h[2].min(m);
            hsv_to_rgb(h)
        });
    })),
    fx!("hue-shift", "Hue Shift", Color, [f("deg", "Ton", -360.0, 360.0, 60.0), f("sat", "Doygunluk", 0.0, 2.0, 1.0), f("val", "Parlaklık", 0.0, 2.0, 1.0)], Px(|fr, v, _| {
        let (d, s, l) = (v.f("deg") / 360.0, v.f("sat"), v.f("val"));
        fr.map_rgb(|c| {
            let h = rgb_to_hsv(c);
            hsv_to_rgb([h[0] + d, (h[1] * s).min(1.0), h[2] * l])
        });
    })),
    fx!("invert", "Invert", Color, [ch("mode", "Kanal", &["RGB", "Parlaklık", "Ton", "Alfa"], 0), f("amt", "Miktar", 0.0, 1.0, 1.0)], Px(|fr, v, _| {
        let (m, k) = (v.i("mode"), v.f("amt"));
        fr.shade(|_, _, p| {
            let c = unpremul(p);
            match m {
                1 => {
                    let l = luma(c);
                    let d = 1.0 - 2.0 * l;
                    premul(mix3(c, c.map(|x| x + d), k), p[3])
                }
                2 => {
                    let h = rgb_to_hsv(c);
                    premul(mix3(c, hsv_to_rgb([h[0] + 0.5, h[1], h[2]]), k), p[3])
                }
                3 => premul(c, p[3] + (1.0 - 2.0 * p[3]) * k),
                _ => premul(mix3(c, c.map(|x| 1.0 - x), k), p[3]),
            }
        });
    })),
    fx!("iridescence", "Iridescence", Color, [f("amt", "Miktar", 0.0, 1.0, 0.7), f("freq", "Bant sayısı", 0.1, 10.0, 2.0), f("phase", "Evre", 0.0, 1.0, 0.0), f("speed", "Hız", -5.0, 5.0, 0.0)], Px(|fr, v, x| {
        let (a, q, ph) = (v.f("amt"), v.f("freq"), v.f("phase") + x.t * v.f("speed"));
        let d = fr.h as f32;
        fr.shade(|px, py, p| {
            let c = unpremul(p);
            let u = luma(c) * q + (px + py) / d * 0.25 + ph;
            let film = hsv_to_rgb([u, 0.65, 1.0]);
            premul(mix3(c, [c[0] * film[0] * 1.5, c[1] * film[1] * 1.5, c[2] * film[2] * 1.5], a), p[3])
        });
    })),
    fx!("luma-key", "Luma Key", Matte, [f("th", "Eşik", 0.0, 1.0, 0.25), f("fe", "Yumuşatma", 0.0, 1.0, 0.1), b("inv", "Açıkları kes", false)], Px(|fr, v, _| {
        let (t, e, i) = (v.f("th"), v.f("fe").max(1e-3), v.b("inv"));
        fr.shade(|_, _, p| {
            let c = unpremul(p);
            let mut a = smooth(t - e, t + e, luma(c));
            if i { a = 1.0 - a }
            premul(c, p[3] * a)
        });
    })),
    fx!("palette-map", "Palette Map", Color, [ch("pal", "Palet", &["Gameboy", "CGA", "Gün batımı", "Okyanus", "Siyah beyaz"], 0), f("amt", "Miktar", 0.0, 1.0, 1.0)], Px(|fr, v, _| {
        let (pal, k) = (PALETTES[v.i("pal").clamp(0, 4) as usize], v.f("amt"));
        fr.map_rgb(|c| {
            let d = |q: &[f32; 3]| (0..3).map(|i| (c[i] - q[i]).powi(2)).sum::<f32>();
            let best = pal.iter().min_by(|a, b| d(a).total_cmp(&d(b))).copied().unwrap_or(c);
            mix3(c, best, k)
        });
    })),
    fx!("posterize", "Posterize", Color, [f("n", "Seviye", 2.0, 32.0, 5.0)], Px(|fr, v, _| {
        let n = v.f("n").max(2.0) - 1.0;
        fr.map_rgb(|c| c.map(|x| (x * n).round() / n));
    })),
    fx!("replace-color", "Replace Color", Color, [c("from", "Hedef renk", [1.0, 0.0, 0.0, 1.0]), c("to", "Yeni renk", [0.0, 0.5, 1.0, 1.0]), f("th", "Eşik", 0.0, 2.0, 0.4), f("fe", "Yumuşatma", 0.001, 1.0, 0.1)], Px(|fr, v, _| {
        let (a, b, t, e) = (v.rgb("from"), v.rgb("to"), v.f("th"), v.f("fe"));
        fr.map_rgb(|c| mix3(b, c, key_alpha(c, a, t, e)));
    })),
    fx!("saturation-vibrance", "Saturation / Vibrance", Color, [f("sat", "Doygunluk", 0.0, 3.0, 1.3), f("vib", "Canlılık", -1.0, 1.0, 0.0)], Px(|fr, v, _| {
        let (s, vb) = (v.f("sat"), v.f("vib"));
        fr.map_rgb(|c| {
            let l = luma(c);
            let cur = rgb_to_hsv(c)[1];
            // Vibrance boosts muted colors more than already-saturated ones.
            let k = s * (1.0 + vb * (1.0 - cur));
            c.map(|x| l + (x - l) * k)
        });
    })),
    fx!("solid-color", "Solid Color", Color, [c("c", "Renk", [1.0, 0.4, 0.2, 1.0]), f("amt", "Miktar", 0.0, 1.0, 1.0)], Px(|fr, v, _| {
        let (k, a) = (v.rgb("c"), v.f("amt"));
        fr.map_rgb(|c| mix3(c, k, a));
    })),
    fx!("solid-matte", "Solid Matte", Matte, [c("c", "Renk", [0.1, 0.1, 0.15, 1.0]), f("op", "Opaklık", 0.0, 1.0, 1.0)], Px(|fr, v, _| {
        let k = v.c("c");
        let m = premul([k[0], k[1], k[2]], k[3] * v.f("op"));
        fr.shade(|_, _, p| over(p, m));
    })),
    fx!("spectral-map", "Spectral Map", Color, [f("range", "Aralık", 0.1, 3.0, 0.8), f("phase", "Evre", 0.0, 1.0, 0.0), f("amt", "Miktar", 0.0, 1.0, 1.0)], Px(|fr, v, _| {
        let (r, ph, k) = (v.f("range"), v.f("phase"), v.f("amt"));
        fr.map_rgb(|c| mix3(c, hsv_to_rgb([luma(c) * r + ph, 1.0, 1.0]), k));
    })),
    fx!("spot-color", "Spot Color", Color, [c("c", "Korunan renk", [1.0, 0.1, 0.1, 1.0]), f("th", "Eşik", 0.0, 2.0, 0.5), f("fe", "Yumuşatma", 0.001, 1.0, 0.2)], Px(|fr, v, _| {
        let (k, t, e) = (v.rgb("c"), v.f("th"), v.f("fe"));
        fr.map_rgb(|c| {
            let l = luma(c);
            mix3(c, [l, l, l], key_alpha(c, k, t, e))
        });
    })),
    fx!("threshold", "Threshold", Color, [f("th", "Eşik", 0.0, 1.0, 0.5), f("soft", "Yumuşaklık", 0.0, 0.5, 0.0)], Px(|fr, v, _| {
        let (t, s) = (v.f("th"), v.f("soft").max(1e-3));
        fr.map_rgb(|c| {
            let k = smooth(t - s, t + s, luma(c));
            [k, k, k]
        });
    })),
    fx!("vignette", "Vignette", Color, [f("amt", "Miktar", 0.0, 1.0, 0.6), f("r", "Yarıçap", 0.0, 2.0, 0.7), f("soft", "Yumuşaklık", 0.01, 2.0, 0.5), c("c", "Renk", [0.0, 0.0, 0.0, 1.0])], Px(|fr, v, _| {
        let (a, r, s, k) = (v.f("amt"), v.f("r"), v.f("soft"), v.rgb("c"));
        let (w, h) = (fr.w as f32, fr.h as f32);
        fr.shade(|x, y, p| {
            let d = ((x / w - 0.5).powi(2) + (y / h - 0.5).powi(2)).sqrt() * 2.0;
            let m = smooth(r, r + s, d) * a;
            premul(mix3(unpremul(p), k, m), p[3])
        });
    })),
];
