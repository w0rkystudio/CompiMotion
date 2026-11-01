//! 3D effects: maps the layer onto a raymarched solid, plus 360 panorama views.
use super::*;
use crate::frame::*;
use crate::fx;
use std::f32::consts::{PI, TAU};

type V3 = [f32; 3];
fn len(a: V3) -> f32 { (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() }
fn norm(a: V3) -> V3 { let l = len(a).max(1e-6); [a[0] / l, a[1] / l, a[2] / l] }
fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }

fn rot_xyz(p: V3, r: V3) -> V3 {
    let (sx, cx) = r[0].sin_cos();
    let (sy, cy) = r[1].sin_cos();
    let (sz, cz) = r[2].sin_cos();
    let a = [p[0], p[1] * cx - p[2] * sx, p[1] * sx + p[2] * cx];
    let b = [a[0] * cy + a[2] * sy, a[1], -a[0] * sy + a[2] * cy];
    [b[0] * cz - b[1] * sz, b[0] * sz + b[1] * cz, b[2]]
}
fn rot_inv(p: V3, r: V3) -> V3 {
    let (sz, cz) = (-r[2]).sin_cos();
    let a = [p[0] * cz - p[1] * sz, p[0] * sz + p[1] * cz, p[2]];
    let (sy, cy) = (-r[1]).sin_cos();
    let b = [a[0] * cy + a[2] * sy, a[1], -a[0] * sy + a[2] * cy];
    let (sx, cx) = (-r[0]).sin_cos();
    [b[0], b[1] * cx - b[2] * sx, b[1] * sx + b[2] * cx]
}

fn sd_box(p: V3, b: V3) -> f32 {
    let q = [p[0].abs() - b[0], p[1].abs() - b[1], p[2].abs() - b[2]];
    len([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]) + q[0].max(q[1]).max(q[2]).min(0.0)
}
fn sd_cyl(p: V3, r: f32, h: f32) -> f32 {
    let d = [p[0].hypot(p[2]) - r, p[1].abs() - h];
    d[0].max(d[1]).min(0.0) + d[0].max(0.0).hypot(d[1].max(0.0))
}
/// Extruded regular polygon or star outline.
fn sd_prism(p: V3, n: f32, r: f32, inner: f32, h: f32) -> f32 {
    let a = p[1].atan2(p[0]).rem_euclid(TAU / n) - PI / n;
    let d = p[0].hypot(p[1]);
    let (q0, q1) = (a.cos() * d, a.sin().abs() * d);
    // Edge from the inner vertex to the outer vertex of one sector.
    let (ax, ay) = (inner * r, 0.0);
    let (bx, by) = ((PI / n).cos() * r, (PI / n).sin() * r);
    let (ex, ey) = (bx - ax, by - ay);
    let t = (((q0 - ax) * ex + (q1 - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
    let dist = (q0 - ax - ex * t).hypot(q1 - ay - ey * t);
    let side = (q0 - ax) * ey - (q1 - ay) * ex;
    let d2 = if side < 0.0 { -dist } else { dist };
    let z = p[2].abs() - h;
    d2.max(z).min(0.0) + d2.max(0.0).hypot(z.max(0.0))
}
fn sd_star_poly(p: V3) -> f32 {
    // Stellated octahedron: the union of two tetrahedra.
    let t1 = (p[0] + p[1] + p[2]).max(p[0] - p[1] - p[2]).max(-p[0] + p[1] - p[2]).max(-p[0] - p[1] + p[2]);
    let t2 = (-p[0] - p[1] - p[2]).max(-p[0] + p[1] + p[2]).max(p[0] - p[1] + p[2]).max(p[0] + p[1] - p[2]);
    (t1.min(t2) - 0.7) * 0.577
}
fn sd_heart(p: V3, depth: f32) -> f32 {
    let (x, y) = (p[0].abs(), -p[1] + 0.25);
    // Two lobes joined to a point, as a smooth 2D distance estimate extruded along z.
    let lobe = (x - 0.38).hypot(y - 0.28) - 0.42;
    let (ax, ay, bx, by) = (0.0, -0.95, 0.72, 0.1);
    let (ex, ey) = (bx - ax, by - ay);
    let t = (((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
    let dd = (x - ax - ex * t).hypot(y - ay - ey * t);
    let sign = if (x - ax) * ey - (y - ay) * ex < 0.0 { -1.0 } else { 1.0 };
    let d2 = lobe.min(dd * sign);
    let z = p[2].abs() - depth;
    d2.max(z).min(0.0) + d2.max(0.0).hypot(z.max(0.0))
}

const SHADES: &[&str] = &["Düz", "Noktasal ışık", "Yönlü ışık", "Derinlik haritası", "Normal haritası"];

macro_rules! solid {
    ($id:literal, $name:literal, $v:ident, $p:ident, $sdf:expr $(, $extra:expr)* $(,)?) => {
        fx!($id, $name, Solid, [
            p("ori", "Yönelim (X, Y)", -20.0, 30.0), f("rz", "Dönme (Z)", -360.0, 360.0, 0.0), p("spin", "Dönüş hızı (X, Y)", 0.0, 0.0),
            $($extra,)*
            f("scale", "Ölçek", 0.0, 10.0, 1.0), p("pos", "Konum", 0.0, 0.0),
            ch("shade", "Gölgeleme", SHADES, 1), f("int", "Yoğunluk", 0.0, 2.0, 1.0), f("shine", "Parlaklık", 0.0, 1.0, 0.8), f("specp", "Yansıma", 1.0, 150.0, 30.0), f("amb", "Ortam", 0.0, 1.0, 0.25),
            f("rx", "Doku tekrar X", 0.01, 20.0, 1.0), f("ry", "Doku tekrar Y", 0.01, 20.0, 1.0), b("fill", "Boşlukları doldur", true),
        ], Px(|fr, $v, x| {
            render(fr, $v, x, |$p: V3| $sdf);
        }))
    };
}

fn render(fr: &mut Frame, v: &V, x: &Ctx, sdf: impl Fn(V3) -> f32 + Sync) {
    let o = v.p("ori");
    let sp = v.p("spin");
    let r: V3 = [(o[0] + sp[0] * x.t * 360.0).to_radians(), (o[1] + sp[1] * x.t * 360.0).to_radians(), v.f("rz").to_radians()];
    let (sc, ps, shade) = (v.f("scale").max(0.001), v.p("pos"), v.i("shade"));
    let (int, shine, spw, amb) = (v.f("int"), v.f("shine"), v.f("specp"), v.f("amb"));
    let (rx, ry, fill) = (v.f("rx"), v.f("ry"), v.b("fill"));
    let src = fr.clone();
    let (w, h) = (fr.w as f32, fr.h as f32);
    // Mean opaque color covers transparent texture areas, so the solid's form stays readable.
    let mean = {
        let (mut s, mut n) = ([0.0f32; 3], 0.0);
        for q in src.px.iter().step_by(13) {
            for c in 0..3 { s[c] += q[c]; }
            n += q[3];
        }
        if n > 1e-3 { s.map(|k| k / n) } else { [0.8; 3] }
    };
    let unit = h * 0.28 * sc;
    let (cx, cy) = (x.c[0] + ps[0] * x.s, x.c[1] + ps[1] * x.s);
    let cam = 4.0;
    let ro = rot_inv([0.0, 0.0, -cam], r);
    let light = norm(rot_inv([-0.5, -0.7, -1.0], r));
    let lp = rot_inv([-2.0, -2.5, -3.0], r);
    fr.shade(|px, py, _| {
        let (nx, ny) = ((px - cx) / unit, (py - cy) / unit);
        let rd = norm(rot_inv([nx / cam, ny / cam, 1.0], r));
        // Skip rays that miss a generous bounding sphere.
        let b = dot(ro, rd);
        let disc = b * b - dot(ro, ro) + 4.0;
        if disc <= 0.0 { return [0.0; 4]; }
        let mut t = (-b - disc.sqrt()).max(0.0);
        let end = -b + disc.sqrt();
        let mut hit = false;
        let mut pos = ro;
        for _ in 0..72 {
            pos = [ro[0] + rd[0] * t, ro[1] + rd[1] * t, ro[2] + rd[2] * t];
            let d = sdf(pos);
            if d < 0.0015 { hit = true; break; }
            t += d * 0.9;
            if t > end { break; }
        }
        if !hit { return [0.0; 4]; }
        let e = 0.002;
        let n = norm([
            sdf([pos[0] + e, pos[1], pos[2]]) - sdf([pos[0] - e, pos[1], pos[2]]),
            sdf([pos[0], pos[1] + e, pos[2]]) - sdf([pos[0], pos[1] - e, pos[2]]),
            sdf([pos[0], pos[1], pos[2] + e]) - sdf([pos[0], pos[1], pos[2] - e]),
        ]);
        // Box-project the texture from the dominant normal axis.
        let (an, u, q) = (n.map(f32::abs), pos[0], pos[1]);
        let (u, q) = if an[2] >= an[0] && an[2] >= an[1] { (u, q) } else if an[0] >= an[1] { (pos[2], q) } else { (u, pos[2]) };
        let tx = src.sample_wrap((u * rx * 0.5 + 0.5) * w, (q * ry * 0.5 + 0.5) * h);
        let col = if fill { let tc = unpremul(tx); mix3(mean, tc, tx[3]) } else { unpremul(tx) };
        let alpha = if fill { 1.0 } else { tx[3] };
        let out = match shade {
            0 => col,
            3 => { let z = (1.0 - (t - (cam - 1.5)) / 3.0).clamp(0.0, 1.0); [z, z, z] }
            4 => { let m = rot_xyz(n, r); [m[0] * 0.5 + 0.5, m[1] * 0.5 + 0.5, 0.5 - m[2] * 0.5] }
            _ => {
                let l = if shade == 1 { norm([lp[0] - pos[0], lp[1] - pos[1], lp[2] - pos[2]]) } else { light };
                let diff = dot(n, l).max(0.0) * int;
                let vw = [-rd[0], -rd[1], -rd[2]];
                let hv = norm([l[0] + vw[0], l[1] + vw[1], l[2] + vw[2]]);
                let spec = dot(n, hv).max(0.0).powf(spw) * shine * int;
                col.map(|k| k * (amb + diff) + spec)
            }
        };
        premul(out, alpha)
    });
}

fn lat_long(d: V3) -> (f32, f32) {
    (d[0].atan2(d[2]) / TAU + 0.5, d[1].clamp(-1.0, 1.0).asin() / PI + 0.5)
}

pub const DEFS: &[Def] = &[
    fx!("360-reorient-sphere", "360º Reorient Sphere", Solid, [p("ori", "Yönelim (X, Y)", 0.0, 0.0), f("rz", "Dönme (Z)", -360.0, 360.0, 0.0)], Px(|fr, v, _| {
        let o = v.p("ori");
        let r = [o[0].to_radians(), o[1].to_radians(), v.f("rz").to_radians()];
        let (w, h) = (fr.w as f32, fr.h as f32);
        let s = fr.clone();
        fr.shade(|x, y, _| {
            let (lon, lat) = ((x / w - 0.5) * TAU, (y / h - 0.5) * PI);
            let d = rot_xyz([lon.sin() * lat.cos(), lat.sin(), lon.cos() * lat.cos()], r);
            let (u, q) = lat_long(d);
            s.sample_wrap(u * w, q * h)
        });
    })),
    fx!("360-viewer", "360º Viewer", Solid, [p("ori", "Yönelim (X, Y)", 0.0, 0.0), f("rz", "Dönme (Z)", -360.0, 360.0, 0.0), f("dist", "Uzaklık", 0.2, 2.0, 1.0), ch("aspect", "En boy oranı", &["16:9", "4:3", "1:1", "4:5", "9:16"], 0), f("scale", "Ölçek", 0.1, 2.0, 1.0)], Px(|fr, v, _| {
        let o = v.p("ori");
        let r = [o[0].to_radians(), o[1].to_radians(), v.f("rz").to_radians()];
        let (dist, sc) = (v.f("dist"), v.f("scale").max(0.01));
        let ar = [16.0 / 9.0, 4.0 / 3.0, 1.0, 0.8, 9.0 / 16.0][v.i("aspect").clamp(0, 4) as usize];
        let (w, h) = (fr.w as f32, fr.h as f32);
        let vh = (h.min(w / ar)) * sc;
        let vw = vh * ar;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let s = fr.clone();
        fr.shade(|x, y, _| {
            let (nx, ny) = ((x - cx) / (vh * 0.5), (y - cy) / (vh * 0.5));
            if (x - cx).abs() > vw / 2.0 || (y - cy).abs() > vh / 2.0 { return [0.0; 4]; }
            let d = rot_xyz(norm([nx, ny, dist * 1.5]), r);
            let (u, q) = lat_long(d);
            s.sample_wrap(u * w, q * h)
        });
    })),
    solid!("box", "Box", v, q, sd_box(q, [v.f("bw") / 200.0, v.f("bh") / 200.0, v.f("bd") / 200.0]) - v.f("smooth") / 400.0, f("bw", "Genişlik", 1.0, 1000.0, 100.0), f("bh", "Yükseklik", 1.0, 1000.0, 300.0), f("bd", "Derinlik", 1.0, 1000.0, 100.0), f("smooth", "Yumuşatma", 0.0, 500.0, 0.0)),
    solid!("cube", "Cube", v, q, { let _ = v; sd_box(q, [0.75, 0.75, 0.75]) }),
    solid!("cylinder", "Cylinder", v, q, sd_cyl(q, v.f("r"), v.f("hh")), f("r", "Yarıçap", 0.05, 1.5, 0.7), f("hh", "Yükseklik", 0.05, 1.5, 0.9)),
    solid!("ellipsoid", "Ellipsoid", v, q, { let a = [v.f("ax"), v.f("ay"), v.f("az")]; let k = len([q[0] / a[0], q[1] / a[1], q[2] / a[2]]); (k - 1.0) * a[0].min(a[1]).min(a[2]) }, f("ax", "X yarıçapı", 0.1, 1.8, 1.2), f("ay", "Y yarıçapı", 0.1, 1.8, 0.8), f("az", "Z yarıçapı", 0.1, 1.8, 0.8)),
    solid!("heart", "Heart", v, q, sd_heart(q, v.f("dep")), f("dep", "Derinlik", 0.02, 1.0, 0.25)),
    solid!("hexagonal-prism", "Hexagonal Prism", v, q, sd_prism(q, 6.0, v.f("r"), (PI / 6.0).cos(), v.f("hh")), f("r", "Yarıçap", 0.1, 1.5, 0.9), f("hh", "Derinlik", 0.02, 1.5, 0.5)),
    solid!("hollow-box", "Hollow Box", v, q, { let t = v.f("wall"); sd_box(q, [0.85, 0.85, 0.85]).max(-sd_box(q, [0.85 - t, 0.85 - t, 2.0])).max(-sd_box(q, [0.85 - t, 2.0, 0.85 - t])).max(-sd_box(q, [2.0, 0.85 - t, 0.85 - t])) }, f("wall", "Kenar kalınlığı", 0.02, 0.8, 0.15)),
    solid!("octahedron", "Octahedron", v, q, { let _ = v; (q[0].abs() + q[1].abs() + q[2].abs() - 1.2) * 0.577 }),
    solid!("pyramid", "Pyramid", v, q, {
        // Apex up at y = -0.9, square base at y = 0.8 (screen y points down).
        let _ = v;
        let (h, b) = (1.7f32, 0.9f32);
        let n = h.hypot(b);
        let side = |a: f32| (a.abs() * h - (q[1] + 0.9) * b) / n;
        side(q[0]).max(side(q[2])).max(q[1] - 0.8)
    }),
    solid!("ribbon", "Ribbon", v, q, { let (w, tw, wv) = (v.f("w"), v.f("twist"), v.f("wave")); let a = q[0] * tw; let (s, c) = a.sin_cos(); let y = q[1] - (q[0] * 3.0).sin() * wv; let (ny, nz) = (y * c - q[2] * s, y * s + q[2] * c); sd_box([q[0], ny, nz], [1.5, w, 0.015]) * 0.6 }, f("w", "Genişlik", 0.05, 1.5, 0.45), f("twist", "Burulma", -6.0, 6.0, 1.5), f("wave", "Dalga", 0.0, 1.0, 0.2)),
    solid!("star", "Star", v, q, sd_prism(q, v.f("n").round().max(3.0), v.f("r"), v.f("inner"), v.f("hh")), f("n", "Köşe", 3.0, 16.0, 5.0), f("r", "Yarıçap", 0.1, 1.8, 1.2), f("inner", "İç oran", 0.1, 0.95, 0.45), f("hh", "Derinlik", 0.01, 0.6, 0.06)),
    solid!("star-polyhedron", "Star Polyhedron", v, q, { let _ = v; sd_star_poly(q) }),
    solid!("star-prism", "Star Prism", v, q, sd_prism(q, v.f("n").round().max(3.0), v.f("r"), v.f("inner"), v.f("hh")), f("n", "Köşe", 3.0, 16.0, 5.0), f("r", "Yarıçap", 0.1, 1.8, 1.2), f("inner", "İç oran", 0.1, 0.95, 0.5), f("hh", "Derinlik", 0.02, 1.5, 0.5)),
    solid!("three-axis-cross", "Three-axis Cross", v, q, { let (l, t) = (v.f("l"), v.f("t")); sd_box(q, [l, t, t]).min(sd_box(q, [t, l, t])).min(sd_box(q, [t, t, l])) }, f("l", "Kol uzunluğu", 0.1, 1.8, 1.2), f("t", "Kalınlık", 0.02, 0.8, 0.28)),
    solid!("torus", "Torus", v, q, (q[0].hypot(q[1]) - v.f("r")).hypot(q[2]) - v.f("tube"), f("r", "Halka yarıçapı", 0.1, 1.5, 0.9), f("tube", "Boru yarıçapı", 0.02, 0.9, 0.35)),
    fx!("tunnel", "Tunnel", Solid, [f("speed", "Hız", -5.0, 5.0, 0.5), f("twist", "Burulma", -5.0, 5.0, 0.0), f("rep", "Doku tekrarı", 0.5, 20.0, 2.0), f("r", "Ağız yarıçapı", 0.01, 1.0, 0.25), f("fog", "Sis", 0.0, 1.0, 0.7), ch("sides", "Kesit", &["Daire", "Kare", "Altıgen"], 0)], Px(|fr, v, x| {
        let (sp, tw, rep, r0, fg, si) = (v.f("speed"), v.f("twist"), v.f("rep"), v.f("r"), v.f("fog"), v.i("sides"));
        let (w, h, c) = (fr.w as f32, fr.h as f32, x.c);
        let s = fr.clone();
        fr.shade(|px, py, _| {
            let (dx, dy) = ((px - c[0]) / h, (py - c[1]) / h);
            let a = dy.atan2(dx);
            let d = match si {
                1 => dx.abs().max(dy.abs()),
                2 => { let n = 6.0; let q = (a.rem_euclid(TAU / n) - PI / n).cos(); dx.hypot(dy) * q }
                _ => dx.hypot(dy),
            };
            if d < 1e-4 { return [0.0; 4]; }
            // Depth is inverse radius, as on the inside of a pipe seen from its axis.
            let z = r0 / d;
            let u = (a + z * tw) / TAU + 0.5;
            let q = z * rep * 0.25 + x.t * sp;
            let t = s.sample_wrap(u * w, q * h);
            let k = 1.0 - (fg * (z * 0.5).min(1.0));
            [t[0] * k, t[1] * k, t[2] * k, t[3]]
        });
    })),
];
