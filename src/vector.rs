//! Rasterizes shapes and text in layer-local coordinates.
use crate::effects::Mods;
use crate::frame::{self, Frame};
use crate::media::Assets;
use crate::model::*;
use ab_glyph::{Font, OutlineCurve};
use std::f32::consts::{PI, TAU};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub struct Raster {
    pub frame: Frame,
    pub ss: f32,
    pub min: [f32; 2],
}

type Poly = Vec<[f32; 2]>;
const MAX_RASTER: f32 = 4096.0;

fn regular(n: usize, r: f32, rot: f32) -> Poly {
    (0..n).map(|i| {
        let a = rot + i as f32 / n as f32 * TAU;
        [a.cos() * r, a.sin() * r]
    }).collect()
}
fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32, n: usize) -> Poly {
    (0..=n).map(|i| {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        [cx + a.cos() * r, cy + a.sin() * r]
    }).collect()
}

/// Samples an editable bezier path into a polyline.
pub fn flatten_path(pts: &[PathPt], closed: bool) -> Poly {
    let mut o = vec![];
    let n = pts.len();
    if n == 0 {
        return o;
    }
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs {
        let (a, b) = (&pts[i], &pts[(i + 1) % n]);
        let c1 = [a.p[0] + a.h_out[0], a.p[1] + a.h_out[1]];
        let c2 = [b.p[0] + b.h_in[0], b.p[1] + b.h_in[1]];
        let straight = a.h_out == [0.0, 0.0] && b.h_in == [0.0, 0.0];
        let steps = if straight { 1 } else { 20 };
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let m = 1.0 - t;
            let f = |k: usize| m * m * m * a.p[k] + 3.0 * m * m * t * c1[k] + 3.0 * m * t * t * c2[k] + t * t * t * b.p[k];
            o.push([f(0), f(1)]);
        }
    }
    if !closed {
        o.push(pts[n - 1].p);
    }
    o
}

/// Outline polygons of a shape; the second value says whether each is closed.
pub fn shape_polys(sh: &ShapeData, t: f64) -> (Vec<Poly>, bool) {
    let sz = sh.size.at(t);
    let (hw, hh) = (sz[0] / 2.0, sz[1] / 2.0);
    let radius = sh.radius.at(t);
    let inner = sh.inner.at(t).clamp(0.01, 0.99);
    let sides = sh.sides.at(t).round().clamp(3.0, 64.0) as usize;
    let squish = |p: Poly| -> Poly {
        p.into_iter().map(|q| [q[0] * hw, q[1] * hh]).collect()
    };
    let polys = match sh.kind {
        ShapeKind::Rect => vec![vec![[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]]],
        ShapeKind::RoundRect => {
            let r = radius.min(hw).min(hh).max(0.0);
            let mut p = arc(hw - r, -hh + r, r, -PI / 2.0, 0.0, 12);
            p.extend(arc(hw - r, hh - r, r, 0.0, PI / 2.0, 12));
            p.extend(arc(-hw + r, hh - r, r, PI / 2.0, PI, 12));
            p.extend(arc(-hw + r, -hh + r, r, PI, PI * 1.5, 12));
            vec![p]
        }
        ShapeKind::Ellipse => vec![squish(regular(128, 1.0, 0.0))],
        ShapeKind::Triangle => vec![vec![[0.0, -hh], [hw, hh], [-hw, hh]]],
        ShapeKind::Polygon => vec![squish(regular(sides, 1.0, -PI / 2.0))],
        ShapeKind::Star => {
            let p = (0..sides * 2).map(|i| {
                let a = -PI / 2.0 + i as f32 / (sides * 2) as f32 * TAU;
                let r = if i % 2 == 0 { 1.0 } else { inner };
                [a.cos() * r, a.sin() * r]
            }).collect();
            vec![squish(p)]
        }
        ShapeKind::Ring => {
            let mut hole = squish(regular(128, inner, 0.0));
            hole.reverse();
            vec![squish(regular(128, 1.0, 0.0)), hole]
        }
        ShapeKind::Heart => {
            let p = (0..128).map(|i| {
                let a = i as f32 / 128.0 * TAU;
                let x = 16.0 * a.sin().powi(3);
                let y = 13.0 * a.cos() - 5.0 * (2.0 * a).cos() - 2.0 * (3.0 * a).cos() - (4.0 * a).cos();
                [x / 16.0, -(y - 2.5) / 14.5]
            }).collect();
            vec![squish(p)]
        }
        ShapeKind::Arrow => vec![squish(vec![[-1.0, -0.3], [0.2, -0.3], [0.2, -0.8], [1.0, 0.0], [0.2, 0.8], [0.2, 0.3], [-1.0, 0.3]])],
        ShapeKind::Cross => {
            let k = inner.min(0.9);
            vec![squish(vec![[-k, -1.0], [k, -1.0], [k, -k], [1.0, -k], [1.0, k], [k, k], [k, 1.0], [-k, 1.0], [-k, k], [-1.0, k], [-1.0, -k], [-k, -k]])]
        }
        ShapeKind::Crescent => {
            let mut p = arc(0.0, 0.0, 1.0, PI * 0.25, PI * 1.75, 64);
            // Inner arc runs back between the tips, with a bulge set by `inner`.
            let (tx, ty) = (PI * 0.25f32.cos(), (PI * 0.25).sin());
            let _ = tx;
            let off = (PI * 0.25).cos();
            let r2 = (ty * ty + (inner * 1.2) * (inner * 1.2)).sqrt().max(ty);
            let cx = off + (r2 * r2 - ty * ty).sqrt() * -1.0 + inner * 0.0;
            let a = (ty / r2).asin();
            let a0 = -a;
            let a1 = a;
            p.extend((1..64).map(|i| {
                let an = a0 + (a1 - a0) * i as f32 / 64.0;
                [cx + an.cos() * r2, an.sin() * r2]
            }));
            vec![squish(p)]
        }
        ShapeKind::Teardrop => {
            let mut p = arc(0.0, 0.25, 0.75, -PI * 0.2, PI * 1.2, 64);
            p.push([0.0, -1.0]);
            vec![squish(p)]
        }
        ShapeKind::Line => return (vec![vec![[-hw, 0.0], [hw, 0.0]]], false),
        ShapeKind::Path => return (vec![flatten_path(&sh.path, sh.closed)], sh.closed),
    };
    (polys, true)
}

fn bounds(polys: &[Poly], pad: f32) -> Option<([f32; 2], [f32; 2])> {
    let (mut a, mut b) = ([f32::MAX; 2], [f32::MIN; 2]);
    for q in polys.iter().flatten() {
        a = [a[0].min(q[0]), a[1].min(q[1])];
        b = [b[0].max(q[0]), b[1].max(q[1])];
    }
    (a[0] <= b[0]).then_some(([a[0] - pad, a[1] - pad], [b[0] + pad, b[1] + pad]))
}

fn build_path(polys: &[Poly], closed: bool) -> Option<tiny_skia::Path> {
    let mut b = PathBuilder::new();
    for poly in polys {
        let Some(s) = poly.first() else { continue };
        b.move_to(s[0], s[1]);
        for q in &poly[1..] {
            b.line_to(q[0], q[1]);
        }
        if closed {
            b.close();
        }
    }
    b.finish()
}

fn paint_of(c: [f32; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
        (c[3].clamp(0.0, 1.0) * 255.0) as u8,
    );
    p.anti_alias = true;
    p
}

fn pixmap_to_frame(pm: &Pixmap) -> Frame {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let px = pm.data().chunks_exact(4).map(|b| [b[0] as f32 / 255.0, b[1] as f32 / 255.0, b[2] as f32 / 255.0, b[3] as f32 / 255.0]).collect();
    Frame { w, h, px }
}

fn fill_color_at(l: &Layer, t: f64, bb: ([f32; 2], [f32; 2]), x: f32, y: f32) -> [f32; 4] {
    let (a, b) = (l.fill.a.at(t), l.fill.b.at(t));
    let (cx, cy) = ((bb.0[0] + bb.1[0]) / 2.0, (bb.0[1] + bb.1[1]) / 2.0);
    let (w, h) = ((bb.1[0] - bb.0[0]).max(1.0), (bb.1[1] - bb.0[1]).max(1.0));
    match l.fill.kind {
        FillKind::Linear => {
            let r = l.fill.angle.at(t).to_radians();
            let (dx, dy) = (r.cos(), r.sin());
            let ext = (w * dx.abs() + h * dy.abs()).max(1.0);
            a.lerp(&b, (((x - cx) * dx + (y - cy) * dy) / ext + 0.5).clamp(0.0, 1.0))
        }
        FillKind::Radial => a.lerp(&b, (((x - cx) / (w / 2.0)).hypot((y - cy) / (h / 2.0))).clamp(0.0, 1.0)),
        _ => a,
    }
}

/// Fills polygons, colors them with the layer's fill and strokes them.
#[allow(clippy::too_many_arguments)]
fn draw(l: &Layer, polys: &[Poly], closed: bool, t: f64, m: &Mods, res: f32, fill_on: bool, extra_pad: f32) -> Option<Raster> {
    let bw = l.border.as_ref().map_or(0.0, |b| b.width.at(t));
    let trimmed = m.trim != [0.0, 1.0];
    let shaped_stroke = m.stroke_color.is_some() || m.taper != [1.0, 1.0] || trimmed;
    let stroke_w = if bw > 0.0 { bw } else if shaped_stroke || !fill_on { 6.0 } else { 0.0 };
    let bb = bounds(polys, stroke_w + 2.0 + extra_pad)?;
    let (w, h) = (bb.1[0] - bb.0[0], bb.1[1] - bb.0[1]);
    let ss = res.min(MAX_RASTER / w.max(1.0)).min(MAX_RASTER / h.max(1.0)).max(0.01);
    let (pw, ph) = (((w * ss).ceil() as u32).max(1), ((h * ss).ceil() as u32).max(1));
    let tr = Transform::from_translate(-bb.0[0], -bb.0[1]).post_scale(ss, ss);
    let mut out = Frame::new(pw as usize, ph as usize);

    if fill_on && !trimmed && l.fill.kind != FillKind::None {
        if let (Some(path), Some(mut pm)) = (build_path(polys, true), Pixmap::new(pw, ph)) {
            pm.fill_path(&path, &paint_of([1.0; 4]), FillRule::Winding, tr, None);
            let cov = pixmap_to_frame(&pm);
            let inner = (bounds(polys, 0.0).unwrap_or(bb), bb.0);
            let solid = l.fill.kind == FillKind::Solid;
            let s0 = l.fill.a.at(t);
            let (b0, ss2) = (bb.0, ss);
            out.shade(|x, y, _| {
                let a = cov.get(x as i32, y as i32)[3];
                if a <= 0.0 {
                    return [0.0; 4];
                }
                let c = if solid { s0 } else { fill_color_at(l, t, inner.0, x / ss2 + b0[0], y / ss2 + b0[1]) };
                frame::premul([c[0], c[1], c[2]], c[3] * a)
            });
        }
    }
    if stroke_w > 0.0 {
        let base = l.border.as_ref().map_or(l.fill.a.at(t), |b| b.color.at(t));
        let mut pm = Pixmap::new(pw, ph)?;
        if shaped_stroke {
            draw_shaped_stroke(&mut pm, polys, closed, stroke_w, base, m, tr);
        } else if let Some(path) = build_path(polys, closed) {
            let st = Stroke { width: stroke_w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
            pm.stroke_path(&path, &paint_of(base), &st, tr, None);
        }
        out.add_over(&pixmap_to_frame(&pm));
    }
    Some(Raster { frame: out, ss, min: bb.0 })
}

/// Stroke drawn piece by piece, for trim, color ramp and taper.
fn draw_shaped_stroke(pm: &mut Pixmap, polys: &[Poly], closed: bool, width: f32, base: [f32; 4], m: &Mods, tr: Transform) {
    for poly in polys {
        let mut pts = poly.clone();
        if closed && pts.len() > 2 {
            pts.push(pts[0]);
        }
        // Subdivide long edges so color and width change smoothly.
        let total: f32 = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
        if total <= 0.0 {
            continue;
        }
        let step = (total / 240.0).max(1.0);
        let mut dist = 0.0f32;
        for w in pts.windows(2) {
            let l = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
            let n = ((l / step).ceil() as usize).max(1);
            for i in 0..n {
                let (a, b) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                let (u0, u1) = ((dist + l * a) / total, (dist + l * b) / total);
                if u1 < m.trim[0].min(m.trim[1]) || u0 > m.trim[1].max(m.trim[0]) {
                    continue;
                }
                let lerp = |k: usize, f: f32| w[0][k] + (w[1][k] - w[0][k]) * f;
                let mut pb = PathBuilder::new();
                pb.move_to(lerp(0, a), lerp(1, a));
                pb.line_to(lerp(0, b), lerp(1, b));
                let Some(path) = pb.finish() else { continue };
                let c = match (m.stroke_color, m.stroke_end_color) {
                    (Some(s), Some(e)) => s.lerp(&e, u0),
                    (Some(s), None) => s,
                    _ => base,
                };
                let wd = width * (m.taper[0] + (m.taper[1] - m.taper[0]) * u0);
                if wd <= 0.01 {
                    continue;
                }
                let st = Stroke { width: wd, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
                pm.stroke_path(&path, &paint_of(c), &st, tr, None);
            }
            dist += l;
        }
    }
}

pub fn raster_shape(l: &Layer, sh: &ShapeData, t: f64, m: &Mods, res: f32) -> Option<(Raster, Vec<[f32; 2]>)> {
    let (polys, closed) = shape_polys(sh, t);
    let fill_on = closed && !sh.freehand;
    let r = draw(l, &polys, closed, t, m, res, fill_on, 0.0)?;
    let mut path = polys.first().cloned().unwrap_or_default();
    if closed && path.len() > 2 {
        path.push(path[0]);
    }
    Some((r, path))
}

const SCRAMBLE: &[char] = &['A', 'K', 'Z', 'Q', 'X', '7', '#', '%', 'M', 'R', '3', '&', 'Ş', 'Ğ', '@', '9'];

/// Lays text out as glyph outlines. Output polygons are grouped by opacity.
pub fn raster_text(l: &Layer, tx: &TextData, t: f64, m: &Mods, res: f32, assets: &Assets) -> Option<Raster> {
    let font = assets.font(&tx.font)?;
    let size = tx.size.at(t).max(1.0);
    let upem = font.units_per_em()?;
    let k = size / upem;
    let source = m.text.as_deref().unwrap_or(&tx.text);
    let tracking = tx.tracking.at(t) + m.tracking;
    let leading = tx.leading.at(t) + m.leading + (font.ascent_unscaled() - font.descent_unscaled() + font.line_gap_unscaled()) * k;
    let total: usize = source.chars().filter(|c| !c.is_whitespace()).count().max(1);
    // Scramble's fractional part is the amount; the integer part reseeds over time.
    let (sc_amt, sc_seed) = (m.scramble.fract(), m.scramble.floor() as u32);
    let vis_chars = m.text_visible.clamp(0.0, 1.0) * total as f32;
    let soft = (m.text_soft.max(0.0) * total as f32).max(1e-3);

    struct G { polys: Vec<Poly>, alpha: f32 }
    let mut glyphs: Vec<G> = vec![];
    let lines: Vec<&str> = source.split('\n').collect();
    let mut idx = 0usize;
    for (ln, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().enumerate().map(|(i, c)| {
            if sc_amt > 0.0 && !c.is_whitespace() && frame::hash2(i as i32, ln as i32, sc_seed + 1) < sc_amt {
                SCRAMBLE[(frame::hash2(i as i32, ln as i32, sc_seed + 2) * SCRAMBLE.len() as f32) as usize % SCRAMBLE.len()]
            } else {
                c
            }
        }).collect();
        let mut width = 0.0;
        let ids: Vec<_> = chars.iter().map(|c| font.glyph_id(*c)).collect();
        for (i, id) in ids.iter().enumerate() {
            width += font.h_advance_unscaled(*id) * k + tracking;
            if i + 1 < ids.len() {
                width += font.kern_unscaled(*id, ids[i + 1]) * k;
            }
        }
        let width = width - tracking;
        let mut pen = match tx.align {
            Align::Left => 0.0,
            Align::Center => -width / 2.0,
            Align::Right => -width,
        };
        let block_h = leading * lines.len() as f32;
        let base_y = ln as f32 * leading - block_h / 2.0 + font.ascent_unscaled() * k;
        for (i, id) in ids.iter().enumerate() {
            let adv = font.h_advance_unscaled(*id) * k;
            if let (Some(out), false) = (font.outline(*id), chars[i].is_whitespace()) {
                let mut alpha = frame::smooth(idx as f32, idx as f32 + soft, vis_chars);
                if soft <= 1e-3 && (idx as f32) < vis_chars.floor() {
                    alpha = 1.0;
                } else if soft <= 1e-3 && (idx as f32) >= vis_chars {
                    alpha = 0.0;
                }
                let (mut dx, mut dy, mut rot, mut sc) = (0.0, 0.0, 0.0f32, 1.0);
                if let Some(g) = &m.glyph {
                    // Each character runs its transition over `spread` of the timeline.
                    let at = idx as f32 / total as f32;
                    let q = ((g.progress * (1.0 + g.spread) - at * 1.0) / g.spread.max(1e-3)).clamp(0.0, 1.0);
                    let e = q * q * (3.0 - 2.0 * q);
                    let rest = 1.0 - e;
                    dx = g.dx * rest;
                    dy = g.dy * rest;
                    rot = (g.rot * rest).to_radians();
                    sc = 1.0 + (g.scale - 1.0) * rest;
                    alpha *= g.opacity + (1.0 - g.opacity) * e;
                }
                let (s, c) = rot.sin_cos();
                let cx = adv / 2.0;
                let place = |q: ab_glyph::Point| -> [f32; 2] {
                    let (x, y) = ((q.x * k - cx) * sc, -q.y * k * sc);
                    [pen + cx + dx + x * c - y * s, base_y + dy + x * s + y * c]
                };
                glyphs.push(G { polys: flatten_outline(&out.curves, &place), alpha });
                idx += 1;
            }
            pen += adv + tracking;
            if i + 1 < ids.len() {
                pen += font.kern_unscaled(*id, ids[i + 1]) * k;
            }
        }
    }
    let all: Vec<Poly> = glyphs.iter().flat_map(|g| g.polys.iter().cloned()).collect();
    let bb = bounds(&all, 4.0 + l.border.as_ref().map_or(0.0, |b| b.width.at(t)))?;
    // Partly transparent glyphs need their own pass; opaque ones share one fill.
    let opaque: Vec<Poly> = glyphs.iter().filter(|g| g.alpha >= 0.999).flat_map(|g| g.polys.iter().cloned()).collect();
    let mut base = draw_text_group(l, &opaque, bb, t, res)?;
    for g in glyphs.iter().filter(|g| g.alpha > 0.0 && g.alpha < 0.999) {
        if let Some(part) = draw_text_group(l, &g.polys, bb, t, res) {
            base.frame.blend_from(&part.frame, Blend::Normal, g.alpha);
        }
    }
    Some(base)
}

fn draw_text_group(l: &Layer, polys: &[Poly], bb: ([f32; 2], [f32; 2]), t: f64, res: f32) -> Option<Raster> {
    let (w, h) = (bb.1[0] - bb.0[0], bb.1[1] - bb.0[1]);
    let ss = res.min(MAX_RASTER / w.max(1.0)).min(MAX_RASTER / h.max(1.0)).max(0.01);
    let (pw, ph) = (((w * ss).ceil() as u32).max(1), ((h * ss).ceil() as u32).max(1));
    let tr = Transform::from_translate(-bb.0[0], -bb.0[1]).post_scale(ss, ss);
    let mut out = Frame::new(pw as usize, ph as usize);
    if polys.is_empty() {
        return Some(Raster { frame: out, ss, min: bb.0 });
    }
    let path = build_path(polys, true)?;
    if let Some(b) = &l.border {
        let bw = b.width.at(t);
        if bw > 0.0 {
            let mut pm = Pixmap::new(pw, ph)?;
            let st = Stroke { width: bw * 2.0, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
            pm.stroke_path(&path, &paint_of(b.color.at(t)), &st, tr, None);
            out = pixmap_to_frame(&pm);
        }
    }
    if l.fill.kind != FillKind::None {
        let mut pm = Pixmap::new(pw, ph)?;
        pm.fill_path(&path, &paint_of([1.0; 4]), FillRule::Winding, tr, None);
        let cov = pixmap_to_frame(&pm);
        let tight = bounds(polys, 0.0).unwrap_or(bb);
        let (b0, sq) = (bb.0, ss);
        let mut fill = Frame::new(pw as usize, ph as usize);
        fill.shade(|x, y, _| {
            let a = cov.get(x as i32, y as i32)[3];
            if a <= 0.0 {
                return [0.0; 4];
            }
            let c = fill_color_at(l, t, tight, x / sq + b0[0], y / sq + b0[1]);
            frame::premul([c[0], c[1], c[2]], c[3] * a)
        });
        out.add_over(&fill);
    }
    Some(Raster { frame: out, ss, min: bb.0 })
}

/// Converts outline curves to closed polygons.
fn flatten_outline(curves: &[OutlineCurve], place: &impl Fn(ab_glyph::Point) -> [f32; 2]) -> Vec<Poly> {
    let mut out: Vec<Poly> = vec![];
    let mut cur: Poly = vec![];
    let mut last: Option<ab_glyph::Point> = None;
    let mut start_new = |cur: &mut Poly, a: ab_glyph::Point, last: &Option<ab_glyph::Point>| {
        let continuous = last.is_some_and(|l| (l.x - a.x).abs() < 1e-3 && (l.y - a.y).abs() < 1e-3);
        if !continuous {
            if cur.len() > 2 {
                out.push(std::mem::take(cur));
            } else {
                cur.clear();
            }
            cur.push(place(a));
        }
    };
    for c in curves {
        match c {
            OutlineCurve::Line(a, b) => {
                start_new(&mut cur, *a, &last);
                cur.push(place(*b));
                last = Some(*b);
            }
            OutlineCurve::Quad(a, q, b) => {
                start_new(&mut cur, *a, &last);
                for i in 1..=8 {
                    let t = i as f32 / 8.0;
                    let m = 1.0 - t;
                    cur.push(place(ab_glyph::point(m * m * a.x + 2.0 * m * t * q.x + t * t * b.x, m * m * a.y + 2.0 * m * t * q.y + t * t * b.y)));
                }
                last = Some(*b);
            }
            OutlineCurve::Cubic(a, q, r, b) => {
                start_new(&mut cur, *a, &last);
                for i in 1..=10 {
                    let t = i as f32 / 10.0;
                    let m = 1.0 - t;
                    let f = |p0: f32, p1: f32, p2: f32, p3: f32| m * m * m * p0 + 3.0 * m * m * t * p1 + 3.0 * m * t * t * p2 + t * t * t * p3;
                    cur.push(place(ab_glyph::point(f(a.x, q.x, r.x, b.x), f(a.y, q.y, r.y, b.y))));
                }
                last = Some(*b);
            }
        }
    }
    if cur.len() > 2 {
        out.push(cur);
    }
    out
}
