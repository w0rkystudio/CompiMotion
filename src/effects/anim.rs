//! Effects that act before rasterizing: motion, opacity pulses, clock, text and stroke changes.
use super::*;
use crate::frame::{hash1, wiggle};
use crate::fx;
use std::f32::consts::TAU;

const WAVES: &[&str] = &["Sinüs", "Üçgen", "Kare", "Testere"];
fn wave(kind: i32, ph: f32) -> f32 {
    let u = ph.rem_euclid(1.0);
    match kind {
        1 => 1.0 - 4.0 * (u - 0.5).abs(),
        2 => if u < 0.5 { 1.0 } else { -1.0 },
        3 => 2.0 * u - 1.0,
        _ => (u * TAU).sin(),
    }
}

/// Point at fraction `u` along a polyline.
pub fn along(path: &[[f32; 2]], u: f32) -> Option<([f32; 2], f32)> {
    if path.len() < 2 {
        return None;
    }
    let lens: Vec<f32> = path.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).collect();
    let mut d = lens.iter().sum::<f32>() * u.clamp(0.0, 1.0);
    for (i, l) in lens.iter().enumerate() {
        if d <= *l || i + 1 == lens.len() {
            let k = if *l > 0.0 { (d / l).min(1.0) } else { 0.0 };
            let (a, b) = (path[i], path[i + 1]);
            return Some(([a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k], (b[1] - a[1]).atan2(b[0] - a[0])));
        }
        d -= l;
    }
    None
}

fn format_count(val: f64, dec: i32, sep: bool, pre: i32) -> String {
    let s = format!("{:.*}", dec.max(0) as usize, val.abs());
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if sep && i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(ch);
    }
    let sign = if val < 0.0 { "-" } else { "" };
    let unit = ["", "%", "$", "₺", "€"][pre.clamp(0, 4) as usize];
    let frac = if frac.is_empty() { String::new() } else { format!(",{frac}") };
    if pre == 1 { format!("{sign}{grouped}{frac}{unit}") } else { format!("{sign}{unit}{grouped}{frac}") }
}

pub const DEFS: &[Def] = &[
    fx!("auto-shake", "Auto-Shake", Anim, [f("str", "Güç", 0.0, 500.0, 20.0), f("freq", "Sıklık", 0.0, 60.0, 8.0), f("rot", "Dönme", 0.0, 90.0, 0.0), f("seed", "Tohum", 0.0, 100.0, 0.0)], Mod(|m, v, x| {
        let (t, s) = (x.t * v.f("freq"), v.i("seed") as u32 * 7 + 1);
        m.dx += wiggle(t, s) * v.f("str");
        m.dy += wiggle(t, s + 11) * v.f("str");
        m.rot += wiggle(t, s + 23) * v.f("rot");
    })),
    fx!("blink", "Blink", Anim, [f("freq", "Sıklık", 0.0, 16.0, 2.0)], Mod(|m, v, x| {
        if (x.t * v.f("freq")).rem_euclid(1.0) >= 0.5 { m.opacity = 0.0 }
    })),
    fx!("echo-keyframes", "Echo Keyframes", Anim, [f("count", "Kopya", 1.0, 32.0, 5.0), f("delay", "Gecikme (sn)", 0.01, 2.0, 0.1), f("fall", "Azalma", 0.0, 1.0, 0.25)], Mod(|m, v, _| {
        m.echo = Some((v.i("count").max(1) as u32, v.f("delay"), v.f("fall")));
    })),
    fx!("fade-in-out", "Fade In/Out", Anim, [f("in", "Giriş (sn)", 0.0, 10.0, 0.5), f("out", "Çıkış (sn)", 0.0, 10.0, 0.5)], Mod(|m, v, x| {
        let (i, o) = (v.f("in"), v.f("out"));
        if i > 0.0 { m.opacity *= (x.t / i).clamp(0.0, 1.0) }
        if o > 0.0 { m.opacity *= ((x.dur - x.t) / o).clamp(0.0, 1.0) }
    })),
    fx!("flicker", "Flicker", Anim, [f("freq", "Sıklık", 0.0, 60.0, 12.0), f("min", "En az opaklık", 0.0, 1.0, 0.1), f("seed", "Tohum", 0.0, 100.0, 0.0)], Mod(|m, v, x| {
        let n = hash1((x.t * v.f("freq")).floor() as i32, v.i("seed") as u32 + 3);
        m.opacity *= v.f("min") + (1.0 - v.f("min")) * n;
    })),
    fx!("move-along-path", "Move Along Path", Anim, [f("prog", "İlerleme", 0.0, 1.0, 0.5), b("auto", "Otomatik", true), f("speed", "Tur / sn", 0.0, 10.0, 0.25), b("orient", "Yöne dön", true), p("a", "Başlangıç", -300.0, 0.0), p("b", "Bitiş", 300.0, 0.0), f("curve", "Kavis", -1000.0, 1000.0, -200.0)], Mod(|m, v, x| {
        let u = if v.b("auto") { (x.t * v.f("speed")).rem_euclid(1.0) } else { v.f("prog") };
        let (a, b, k) = (v.p("a"), v.p("b"), v.f("curve"));
        // Quadratic arc whose control point sits off the A-B midpoint.
        let (nx, ny) = (-(b[1] - a[1]), b[0] - a[0]);
        let l = nx.hypot(ny).max(1e-3);
        let ctl = [(a[0] + b[0]) * 0.5 + nx / l * k, (a[1] + b[1]) * 0.5 + ny / l * k];
        let q = |w: f32, a: f32, c: f32, b: f32| (1.0 - w) * (1.0 - w) * a + 2.0 * (1.0 - w) * w * c + w * w * b;
        m.dx += q(u, a[0], ctl[0], b[0]);
        m.dy += q(u, a[1], ctl[1], b[1]);
        if v.b("orient") {
            let dx = 2.0 * (1.0 - u) * (ctl[0] - a[0]) + 2.0 * u * (b[0] - ctl[0]);
            let dy = 2.0 * (1.0 - u) * (ctl[1] - a[1]) + 2.0 * u * (b[1] - ctl[1]);
            m.rot += dy.atan2(dx).to_degrees();
        }
    })),
    fx!("oscillate", "Oscillate", Anim, [ch("wave", "Dalga", WAVES, 0), f("amp", "Genlik", 0.0, 1000.0, 50.0), f("freq", "Sıklık", 0.0, 30.0, 1.0), f("angle", "Açı", -360.0, 360.0, 0.0), f("phase", "Evre", 0.0, 1.0, 0.0)], Mod(|m, v, x| {
        let d = wave(v.i("wave"), x.t * v.f("freq") + v.f("phase")) * v.f("amp");
        let a = v.f("angle").to_radians();
        m.dx += a.cos() * d;
        m.dy += a.sin() * d;
    })),
    fx!("pulse-opacity", "Pulse Opacity", Anim, [ch("wave", "Dalga", WAVES, 0), f("freq", "Sıklık", 0.0, 30.0, 1.0), f("min", "En az", 0.0, 1.0, 0.0), f("max", "En çok", 0.0, 1.0, 1.0)], Mod(|m, v, x| {
        let w = wave(v.i("wave"), x.t * v.f("freq")) * 0.5 + 0.5;
        m.opacity *= v.f("min") + (v.f("max") - v.f("min")) * w;
    })),
    fx!("pulse-size", "Pulse Size", Anim, [ch("wave", "Dalga", WAVES, 0), f("freq", "Sıklık", 0.0, 30.0, 1.0), f("min", "En az", 0.0, 5.0, 0.8), f("max", "En çok", 0.0, 5.0, 1.2)], Mod(|m, v, x| {
        let w = wave(v.i("wave"), x.t * v.f("freq")) * 0.5 + 0.5;
        let s = v.f("min") + (v.f("max") - v.f("min")) * w;
        m.sx *= s;
        m.sy *= s;
    })),
    fx!("random-jitter", "Random Jitter", Anim, [f("amt", "Miktar", 0.0, 500.0, 15.0), f("rate", "Değişim / sn", 1.0, 60.0, 12.0), f("rot", "Dönme", 0.0, 180.0, 5.0), f("scale", "Ölçek", 0.0, 1.0, 0.0), f("seed", "Tohum", 0.0, 100.0, 0.0)], Mod(|m, v, x| {
        let (n, s) = ((x.t * v.f("rate")).floor() as i32, v.i("seed") as u32 * 13);
        let r = |o: u32| hash1(n, s + o) * 2.0 - 1.0;
        m.dx += r(1) * v.f("amt");
        m.dy += r(2) * v.f("amt");
        m.rot += r(3) * v.f("rot");
        let sc = 1.0 + r(4) * v.f("scale");
        m.sx *= sc;
        m.sy *= sc;
    })),
    fx!("spin", "Spin", Anim, [f("speed", "Tur / sn", -20.0, 20.0, 0.5), f("start", "Başlangıç açısı", -360.0, 360.0, 0.0)], Mod(|m, v, x| {
        m.rot += x.t * v.f("speed") * 360.0 + v.f("start");
    })),
    fx!("swing", "Swing", Anim, [ch("wave", "Dalga", WAVES, 0), f("angle", "Açı", 0.0, 180.0, 20.0), f("freq", "Sıklık", 0.0, 30.0, 1.0), f("phase", "Evre", 0.0, 1.0, 0.0)], Mod(|m, v, x| {
        m.rot += wave(v.i("wave"), x.t * v.f("freq") + v.f("phase")) * v.f("angle");
    })),
    fx!("time-quantization", "Time Quantization", Anim, [f("fps", "Kare / sn", 1.0, 60.0, 8.0)], Mod(|m, v, x| {
        let q = v.f("fps").max(1.0) as f64;
        m.time = Some((x.t as f64 * q).floor() / q);
    })),
    // Text and stroke
    fx!("count-up-down", "Count Up/Down", Text, [f("from", "Başlangıç", -1e6, 1e6, 0.0), f("to", "Bitiş", -1e6, 1e6, 100.0), f("dec", "Ondalık", 0.0, 6.0, 0.0), b("sep", "Binlik ayırıcı", true), ch("unit", "Birim", &["Yok", "%", "$", "₺", "€"], 0)], Mod(|m, v, x| {
        let k = (x.t / x.dur.max(1e-3)).clamp(0.0, 1.0) as f64;
        let val = v.f("from") as f64 + (v.f("to") - v.f("from")) as f64 * k;
        m.text = Some(format_count(val, v.i("dec"), v.b("sep"), v.i("unit")));
    })),
    fx!("drawing-progress", "Drawing Progress", Text, [f("start", "Başlangıç", 0.0, 1.0, 0.0), f("end", "Bitiş", 0.0, 1.0, 0.75), b("auto", "Süreye göre", false)], Mod(|m, v, x| {
        let e = if v.b("auto") { (x.t / x.dur.max(1e-3)).clamp(0.0, 1.0) } else { v.f("end") };
        m.trim = [v.f("start"), e];
        m.text_visible = e;
    })),
    fx!("stroke-color", "Stroke Color", Text, [c("a", "Baş rengi", [1.0, 0.3, 0.5, 1.0]), c("b", "Son rengi", [0.2, 0.6, 1.0, 1.0])], Mod(|m, v, _| {
        m.stroke_color = Some(v.c("a"));
        m.stroke_end_color = Some(v.c("b"));
    })),
    fx!("stroke-taper", "Stroke Taper", Text, [f("head", "Baş kalınlığı", 0.0, 1.0, 0.0), f("tail", "Son kalınlığı", 0.0, 1.0, 1.0)], Mod(|m, v, _| {
        m.taper = [v.f("head"), v.f("tail")];
    })),
    fx!("text-progress", "Text Progress", Text, [f("prog", "İlerleme", 0.0, 1.0, 0.6), f("soft", "Yumuşaklık", 0.0, 1.0, 0.0), b("auto", "Süreye göre", false)], Mod(|m, v, x| {
        m.text_visible = if v.b("auto") { (x.t / x.dur.max(1e-3)).clamp(0.0, 1.0) } else { v.f("prog") };
        m.text_soft = v.f("soft");
    })),
    fx!("text-randomizer", "Text Randomizer", Text, [f("amt", "Karışım", 0.0, 1.0, 0.5), f("rate", "Değişim / sn", 1.0, 60.0, 15.0)], Mod(|m, v, x| {
        m.scramble = v.f("amt") + (x.t * v.f("rate")).floor() * 1000.0;
    })),
    fx!("text-spacing", "Text Spacing", Text, [f("track", "Harf aralığı", -200.0, 1000.0, 20.0), f("lead", "Satır aralığı", -200.0, 1000.0, 0.0)], Mod(|m, v, _| {
        m.tracking += v.f("track");
        m.leading += v.f("lead");
    })),
    fx!("text-transform", "Text Transform", Text, [f("prog", "İlerleme", 0.0, 1.0, 0.5), f("spread", "Yayılım", 0.01, 1.0, 0.3), p("off", "Kaydır", 0.0, 60.0), f("rot", "Dönme", -360.0, 360.0, 0.0), f("scale", "Ölçek", 0.0, 5.0, 1.0), f("op", "Opaklık", 0.0, 1.0, 0.0)], Mod(|m, v, _| {
        let o = v.p("off");
        m.glyph = Some(GlyphAnim { progress: v.f("prog"), spread: v.f("spread"), dx: o[0], dy: o[1], rot: v.f("rot"), scale: v.f("scale"), opacity: v.f("op") });
    })),
    fx!("timecode", "Timecode", Text, [ch("fmt", "Biçim", &["SS:DD:SN:KR", "DD:SN", "Saniye", "Kare"], 0), f("off", "Ofset (sn)", -3600.0, 3600.0, 0.0)], Mod(|m, v, x| {
        let t = (x.t + v.f("off")).max(0.0);
        let (s, fr) = (t.floor() as u32, ((t.fract()) * x.fps) as u32);
        m.text = Some(match v.i("fmt") {
            1 => format!("{:02}:{:02}", s / 60, s % 60),
            2 => format!("{t:.2}"),
            3 => format!("{}", (t * x.fps) as u32),
            _ => format!("{:02}:{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60, fr),
        });
    })),
];
