//! The compositor: turns a project and a time into a frame.
use crate::effects::{self, Ctx, Mods, Run, V};
use crate::frame::{self, Frame, Px};
use crate::media::Assets;
use crate::model::*;
use crate::vector;
use std::sync::Arc;

pub type Mat3 = [[f32; 3]; 3];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            o[r][c] = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}
pub fn apply(m: &Mat3, x: f32, y: f32) -> [f32; 2] {
    let w = m[2][0] * x + m[2][1] * y + m[2][2];
    let w = if w.abs() < 1e-6 { 1e-6 } else { w };
    [(m[0][0] * x + m[0][1] * y + m[0][2]) / w, (m[1][0] * x + m[1][1] * y + m[1][2]) / w]
}
pub fn invert(m: &Mat3) -> Option<Mat3> {
    let c = |a: usize, b: usize, d: usize, e: usize| m[a][b] * m[d][e] - m[a][e] * m[d][b];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    if det.abs() < 1e-12 {
        return None;
    }
    let k = 1.0 / det;
    Some([
        [c(1, 1, 2, 2) * k, -c(0, 1, 2, 2) * k, c(0, 1, 1, 2) * k],
        [-c(1, 0, 2, 2) * k, c(0, 0, 2, 2) * k, -c(0, 0, 1, 2) * k],
        [c(1, 0, 2, 1) * k, -c(0, 0, 2, 1) * k, c(0, 0, 1, 1) * k],
    ])
}
fn translate(x: f32, y: f32) -> Mat3 {
    [[1.0, 0.0, x], [0.0, 1.0, y], [0.0, 0.0, 1.0]]
}

const PERSPECTIVE: f32 = 1200.0;

/// Effects that read what lies beneath the layer.
const NEEDS_BG: &[&str] = &["copy-background", "magnify-background", "glass", "soft-glass", "displacement-map", "polar-displacement-map", "bump-map"];

/// Local-to-parent matrix of one layer, 2D plus optional perspective tilt.
fn local_matrix(l: &Layer, t: f64, m: &Mods, comp: [f32; 2], cam: Option<&CamView>) -> Mat3 {
    let tf = &l.tf;
    let pos = tf.pos.at(t);
    let sc = tf.scale.at(t);
    let an = tf.anchor.at(t);
    let sk = tf.skew.at(t);
    let (sn, cs) = (tf.rot.at(t) + m.rot).to_radians().sin_cos();
    let (sx, sy) = (sc[0] * m.sx, sc[1] * m.sy);
    let (tx, ty) = (sk[0].to_radians().tan(), sk[1].to_radians().tan());
    // Rotation * skew * scale, about the anchor.
    let a = [[cs * sx + -sn * ty * sx, (cs * tx - sn) * sy], [sn * sx + cs * ty * sx, (sn * tx + cs) * sy]];
    let linear: Mat3 = [
        [a[0][0], a[0][1], -(a[0][0] * an[0] + a[0][1] * an[1])],
        [a[1][0], a[1][1], -(a[1][0] * an[0] + a[1][1] * an[1])],
        [0.0, 0.0, 1.0],
    ];
    let (rx, ry) = (tf.rot_x.at(t).to_radians(), tf.rot_y.at(t).to_radians());
    let z = tf.z.at(t);
    let tilt = rx != 0.0 || ry != 0.0 || z != 0.0 || (l.three_d && cam.is_some());
    let (px, py) = (pos[0] + m.dx, pos[1] + m.dy);
    if !tilt {
        return mul(&translate(px, py), &linear);
    }
    // 3D: columns are the images of local x, y and origin; divide by depth for perspective.
    let (s1, c1) = rx.sin_cos();
    let (s2, c2) = ry.sin_cos();
    let rot3 = |x: f32, y: f32, zz: f32| -> [f32; 3] {
        let (y1, z1) = (y * c1 - zz * s1, y * s1 + zz * c1);
        [x * c2 + z1 * s2, y1, -x * s2 + z1 * c2]
    };
    let ex = rot3(linear[0][0], linear[1][0], 0.0);
    let ey = rot3(linear[0][1], linear[1][1], 0.0);
    let o = rot3(linear[0][2], linear[1][2], 0.0);
    let (center, focal) = ([comp[0] / 2.0, comp[1] / 2.0], cam.map_or(PERSPECTIVE, |c| c.focal));
    let mut world = [
        [ex[0], ey[0], o[0] + px - center[0]],
        [ex[1], ey[1], o[1] + py - center[1]],
        [ex[2], ey[2], o[2] + z],
    ];
    if let (Some(c), true) = (cam, l.three_d) {
        world = c.view(&world);
    }
    let w = |k: usize| world[2][k] / focal + if k == 2 { 1.0 } else { 0.0 };
    let (w0, w1, w2) = (w(0), w(1), w(2));
    [
        [world[0][0] + center[0] * w0, world[0][1] + center[0] * w1, world[0][2] + center[0] * w2],
        [world[1][0] + center[1] * w0, world[1][1] + center[1] * w1, world[1][2] + center[1] * w2],
        [w0, w1, w2],
    ]
}

struct CamView {
    pos: [f32; 3],
    rot: [f32; 3],
    focal: f32,
}
impl CamView {
    /// Applies the inverse camera transform to the columns of a world-space map.
    fn view(&self, m: &Mat3) -> Mat3 {
        let (s1, c1) = (-self.rot[0]).sin_cos();
        let (s2, c2) = (-self.rot[1]).sin_cos();
        let (s3, c3) = (-self.rot[2]).sin_cos();
        let r = |v: [f32; 3]| -> [f32; 3] {
            let (x, y) = (v[0] * c3 - v[1] * s3, v[0] * s3 + v[1] * c3);
            let (x2, z2) = (x * c2 + v[2] * s2, -x * s2 + v[2] * c2);
            [x2, y * c1 - z2 * s1, y * s1 + z2 * c1]
        };
        let col = |k: usize, trans: bool| {
            let d = if trans { self.pos } else { [0.0; 3] };
            r([m[0][k] - d[0], m[1][k] - d[1], m[2][k] - d[2]])
        };
        let (a, b, o) = (col(0, false), col(1, false), col(2, true));
        [[a[0], b[0], o[0]], [a[1], b[1], o[1]], [a[2], b[2], o[2]]]
    }
}

/// Runs the pre-raster effects of a layer.
pub fn run_mods(l: &Layer, t: f64, fps: f32, scale: f32, bg: &Frame) -> Mods {
    let mut m = Mods::default();
    for e in l.effects.iter().filter(|e| e.on) {
        let Some(def) = effects::find(&e.def) else { continue };
        if let Run::Mod(f) = def.run {
            let v = V { def, inst: e, t };
            let ctx = Ctx { t: t as f32, dur: l.dur as f32, s: scale, c: [0.0; 2], bg, seed: l.id as u32, fps, path: &[] };
            f(&mut m, &v, &ctx);
        }
    }
    m
}

/// World matrix in project pixels, following the parent chain.
pub fn world_matrix(l: &Layer, all: &[&Layer], comp_t: f64, comp: [f32; 2], cam: Option<&CamViewOpaque>, mods: &Mods) -> Mat3 {
    let cam = cam.map(|c| &c.0);
    let mut m = local_matrix(l, l.local(comp_t), mods, comp, cam);
    let mut cur = l.parent;
    let mut guard = 0;
    while let (Some(id), true) = (cur, guard < 32) {
        guard += 1;
        let Some(p) = all.iter().find(|q| q.id == id) else { break };
        let pm = local_matrix(p, p.local(comp_t), &Mods::default(), comp, None);
        m = mul(&pm, &m);
        cur = p.parent;
    }
    m
}

pub struct CamViewOpaque(CamView);

pub fn find_camera(p: &Project, t: f64) -> Option<CamViewOpaque> {
    let l = p.layers.iter().rev().find(|l| matches!(l.content, Content::Camera { .. }) && l.active(t))?;
    let Content::Camera { fov } = &l.content else { return None };
    let lt = l.local(t);
    let pos = l.tf.pos.at(lt);
    let f = fov.at(lt).clamp(1.0, 170.0).to_radians();
    Some(CamViewOpaque(CamView {
        pos: [pos[0] - p.w as f32 / 2.0, pos[1] - p.h as f32 / 2.0, l.tf.z.at(lt)],
        rot: [l.tf.rot_x.at(lt).to_radians(), l.tf.rot_y.at(lt).to_radians(), l.tf.rot.at(lt).to_radians()],
        focal: (p.h as f32 / 2.0) / (f / 2.0).tan(),
    }))
}

pub struct RenderCtx<'a> {
    pub assets: &'a Assets,
    pub fps: f32,
    pub comp: [f32; 2],
    pub scale: f32,
    pub w: usize,
    pub h: usize,
    pub cam: Option<CamViewOpaque>,
}

/// Raster for a layer's own content, positioned in local coordinates.
struct Source {
    frame: Arc<Frame>,
    ss: f32,
    min: [f32; 2],
}

fn make_source(l: &Layer, t: f64, comp_t: f64, m: &Mods, res: f32, rc: &RenderCtx) -> Option<(Source, Vec<[f32; 2]>)> {
    match &l.content {
        Content::Shape(sh) => vector::raster_shape(l, sh, t, m, res).map(|(s, path)| (Source { frame: Arc::new(s.frame), ss: s.ss, min: s.min }, path)),
        Content::Text(tx) => vector::raster_text(l, tx, t, m, res, rc.assets).map(|s| (Source { frame: Arc::new(s.frame), ss: s.ss, min: s.min }, vec![])),
        Content::Image { path } => {
            let f = rc.assets.image(path)?;
            let min = [-(f.w as f32) / 2.0, -(f.h as f32) / 2.0];
            Some((Source { frame: f, ss: 1.0, min }, vec![]))
        }
        Content::Video { path } => {
            let st = m.time.map_or(l.source_time(comp_t), |q| l.in_point + q * l.speed as f64);
            let f = rc.assets.video_frame(path, st)?;
            let min = [-(f.w as f32) / 2.0, -(f.h as f32) / 2.0];
            Some((Source { frame: f, ss: 1.0, min }, vec![]))
        }
        Content::Group { children } => {
            // A group's raster has project dimensions, with its origin at the frame center.
            let lt = m.time.unwrap_or(t);
            let mut child_rc = RenderCtx { assets: rc.assets, fps: rc.fps, comp: rc.comp, scale: rc.scale, w: rc.w, h: rc.h, cam: None };
            child_rc.cam = None;
            let empty = Frame::new(rc.w, rc.h);
            let f = render_stack(children, lt, &child_rc, empty, None);
            let (cw, ch) = (rc.comp[0], rc.comp[1]);
            Some((Source { frame: Arc::new(f), ss: rc.scale, min: [-cw / 2.0, -ch / 2.0] }, vec![]))
        }
        _ => None,
    }
}

/// Draws the source through `m` (local to output pixels) into a transparent output frame.
fn warp_into(out: &mut Frame, src: &Source, m: &Mat3, weight: f32) {
    let Some(inv) = invert(m) else { return };
    let (ss, min) = (src.ss, src.min);
    let f = &src.frame;
    out.shade(|x, y, p| {
        let w = inv[2][0] * x + inv[2][1] * y + inv[2][2];
        if w <= 1e-5 {
            return p;
        }
        let u = (inv[0][0] * x + inv[0][1] * y + inv[0][2]) / w;
        let v = (inv[1][0] * x + inv[1][1] * y + inv[1][2]) / w;
        let q = f.sample((u - min[0]) * ss, (v - min[1]) * ss);
        if weight >= 1.0 { q } else { std::array::from_fn(|c| p[c] + q[c] * weight) }
    });
}

fn apply_border_generic(fr: &mut Frame, b: &Border, t: f64, s: f32) {
    let w = b.width.at(t) * s;
    if w <= 0.0 {
        return;
    }
    let c = b.color.at(t);
    let mut g = fr.clone();
    g.gauss(w * 0.5, w * 0.5);
    fr.px.iter_mut().zip(g.px.iter()).for_each(|(d, q)| {
        let a = frame::smooth(0.02, 0.12, q[3]) * c[3];
        *d = frame::over(*d, frame::premul([c[0], c[1], c[2]], a));
    });
}

fn apply_shadow(fr: &mut Frame, sh: &Shadow, t: f64, s: f32) {
    let (a, d, c) = (sh.angle.at(t).to_radians(), sh.dist.at(t) * s, sh.color.at(t));
    let (dx, dy) = (a.cos() * d, a.sin() * d);
    let r = sh.blur.at(t) * s;
    let mut g = fr.clone();
    g.gauss(r, r);
    let src = g;
    fr.shade(|x, y, p| {
        let q = src.sample(x - dx, y - dy)[3];
        frame::over(p, frame::premul([c[0], c[1], c[2]], c[3] * q))
    });
}

/// Renders one layer's color into a fresh output-sized frame, or None if nothing draws.
pub fn render_layer(l: &Layer, all: &[&Layer], comp_t: f64, rc: &RenderCtx, under: &Frame) -> Option<(Frame, f32)> {
    let raw_t = l.local(comp_t);
    let tiny = Frame::new(1, 1);
    let mods = run_mods(l, raw_t, rc.fps, rc.scale, &tiny);
    let t = mods.time.unwrap_or(raw_t);
    let m_project = world_matrix(l, all, l.start + t, rc.comp, rc.cam.as_ref(), &mods);
    let to_out: Mat3 = [[rc.scale, 0.0, 0.0], [0.0, rc.scale, 0.0], [0.0, 0.0, 1.0]];
    let m_out = mul(&to_out, &m_project);
    let res = ((m_out[0][0] * m_out[1][1] - m_out[0][1] * m_out[1][0]).abs().sqrt()).clamp(0.05, 8.0);

    let (src, path) = make_source(l, t, comp_t, &mods, res, rc)?;
    let mut out = Frame::new(rc.w, rc.h);
    if let (Some((n, delay, fall)), false) = (mods.echo, l.motion_blur) {
        // Older echoes go underneath, each read from the layer's own keyframes.
        for i in (1..=n).rev() {
            let et = t - (i as f64) * delay as f64;
            if et < 0.0 {
                continue;
            }
            let Some((es, _)) = make_source(l, et, l.start + et, &mods, res, rc) else { continue };
            let em = mul(&to_out, &world_matrix(l, all, l.start + et, rc.comp, rc.cam.as_ref(), &mods));
            let k = (1.0 - fall).powi(i as i32) * l.opacity.at(et).clamp(0.0, 1.0);
            let mut tmp = Frame::new(rc.w, rc.h);
            warp_into(&mut tmp, &es, &em, 1.0);
            out.blend_from(&tmp, Blend::Normal, k);
        }
        let mut top = Frame::new(rc.w, rc.h);
        warp_into(&mut top, &src, &m_out, 1.0);
        out.add_over(&top);
    } else if l.motion_blur {
        // Shutter of half a frame, sampled across the transform.
        let n = 8;
        let shutter = 0.5 / rc.fps.max(1.0) as f64;
        for i in 0..n {
            let dt = shutter * (i as f64 / (n - 1) as f64 - 0.5);
            let mt = mul(&to_out, &world_matrix(l, all, l.start + t + dt, rc.comp, rc.cam.as_ref(), &mods));
            warp_into(&mut out, &src, &mt, 1.0 / n as f32);
        }
    } else {
        warp_into(&mut out, &src, &m_out, 1.0);
    }

    let content_has_own_border = matches!(l.content, Content::Shape(_) | Content::Text(_));
    if let (Some(b), false) = (&l.border, content_has_own_border) {
        apply_border_generic(&mut out, b, t, rc.scale);
    }
    if let Some(sh) = &l.shadow {
        apply_shadow(&mut out, sh, t, rc.scale);
    }
    let center = apply(&m_out, l.tf.anchor.at(t)[0], l.tf.anchor.at(t)[1]);
    let path_out: Vec<[f32; 2]> = path.iter().map(|q| apply(&m_out, q[0], q[1])).collect();
    for e in l.effects.iter().filter(|e| e.on) {
        let Some(def) = effects::find(&e.def) else { continue };
        if let Run::Px(run) = def.run {
            let v = V { def, inst: e, t };
            let bg = if NEEDS_BG.contains(&def.id) { under } else { &tiny };
            let ctx = Ctx { t: t as f32, dur: l.dur as f32, s: rc.scale, c: center, bg, seed: l.id as u32, fps: rc.fps, path: &path_out };
            run(&mut out, &v, &ctx);
        }
    }
    let op = l.opacity.at(t).clamp(0.0, 1.0) * mods.opacity.clamp(0.0, 1.0);
    Some((out, op))
}

/// Composites a stack bottom-to-top onto `base`. A mask layer cuts every layer beneath it
/// back to the previous mask, without touching `base`.
pub fn render_stack(layers: &[Layer], t: f64, rc: &RenderCtx, base: Frame, flatten_bg: Option<Px>) -> Frame {
    let all: Vec<&Layer> = layers.iter().collect();
    let has_mask = layers.iter().any(|l| l.mask != MaskMode::None && l.active(t));
    let mut out = base;
    let mut seg = if has_mask { Some(Frame::new(rc.w, rc.h)) } else { None };
    for l in layers.iter().filter(|l| l.active(t)) {
        if matches!(l.content, Content::Audio { .. } | Content::Null | Content::Camera { .. }) {
            continue;
        }
        // Background-reading effects see everything already beneath this layer.
        let under = match &seg {
            Some(s) => {
                let mut u = out.clone();
                u.add_over(s);
                u
            }
            None => out.clone_if_needed(l),
        };
        let _ = flatten_bg;
        let Some((fr, op)) = render_layer(l, &all, t, rc, &under) else { continue };
        if l.mask != MaskMode::None {
            if let Some(mut s) = seg.take() {
                apply_mask(&mut s, &fr, l.mask, op);
                out.add_over(&s);
            }
            seg = Some(Frame::new(rc.w, rc.h));
            continue;
        }
        match &mut seg {
            Some(s) => s.blend_from(&fr, l.blend, op),
            None => out.blend_from(&fr, l.blend, op),
        }
    }
    if let Some(s) = seg {
        out.add_over(&s);
    }
    out
}

impl Frame {
    /// Only a few effects read the backdrop, so the copy is skipped for other layers.
    fn clone_if_needed(&self, l: &Layer) -> Frame {
        if l.effects.iter().any(|e| e.on && NEEDS_BG.contains(&e.def.as_str())) { self.clone() } else { Frame::new(1, 1) }
    }
}

fn apply_mask(s: &mut Frame, mask: &Frame, mode: MaskMode, op: f32) {
    use rayon::prelude::*;
    s.px.par_iter_mut().zip(mask.px.par_iter()).for_each(|(d, q)| {
        let a = q[3] * op;
        let k = match mode {
            MaskMode::Mask => a,
            MaskMode::InverseMask => 1.0 - a,
            MaskMode::LumaMask => frame::luma(frame::unpremul(*q)) * a,
            MaskMode::None => 1.0,
        };
        *d = d.map(|c| c * k);
    });
}

pub fn render_project(p: &Project, t: f64, scale: f32, assets: &Assets) -> Frame {
    let (w, h) = (((p.w as f32 * scale).round() as usize).max(1), ((p.h as f32 * scale).round() as usize).max(1));
    // Output pixels per project pixel, using the rounded size.
    let rc = RenderCtx { assets, fps: p.fps, comp: [p.w as f32, p.h as f32], scale: w as f32 / p.w as f32, w, h, cam: find_camera(p, t) };
    let bg = frame::premul([p.bg[0], p.bg[1], p.bg[2]], p.bg[3]);
    render_stack(&p.layers, t, &rc, Frame::solid(w, h, bg), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::Assets;

    fn proj() -> Project {
        Project::new("t", 200, 100, 30.0, [0.0, 0.0, 0.0, 1.0])
    }
    fn rect(id: u64, c: [f32; 4], size: [f32; 2], pos: [f32; 2]) -> Layer {
        let sh = ShapeData { kind: ShapeKind::Rect, size: Track::new(size), radius: Track::new(0.0), sides: Track::new(5.0), inner: Track::new(0.5), path: vec![], closed: true, freehand: false };
        let mut l = Layer::new(id, "r", Content::Shape(sh), pos, 10.0);
        l.fill.a = Track::new(c);
        l
    }
    fn at(f: &Frame, x: usize, y: usize) -> Px {
        f.px[y * f.w + x]
    }

    #[test]
    fn rect_lands_where_positioned() {
        let mut p = proj();
        p.layers.push(rect(1, [1.0, 0.0, 0.0, 1.0], [20.0, 20.0], [50.0, 50.0]));
        let f = render_project(&p, 0.0, 1.0, &Assets::default());
        assert!(at(&f, 50, 50)[0] > 0.99);
        assert!(at(&f, 150, 50)[0] < 0.01);
    }

    #[test]
    fn keyframed_position_moves_layer() {
        let mut p = proj();
        let mut l = rect(1, [1.0, 1.0, 1.0, 1.0], [10.0, 10.0], [0.0, 0.0]);
        l.tf.pos.set_key(0.0, [20.0, 50.0]);
        l.tf.pos.set_key(2.0, [180.0, 50.0]);
        l.tf.pos.keys[0].ease = Ease::Linear;
        p.layers.push(l);
        let f = render_project(&p, 1.0, 1.0, &Assets::default());
        assert!(at(&f, 100, 50)[1] > 0.99);
        assert!(at(&f, 20, 50)[1] < 0.01);
    }

    #[test]
    fn mask_cuts_lower_layers_but_not_background() {
        let mut p = proj();
        p.bg = [0.0, 0.0, 1.0, 1.0];
        p.layers.push(rect(1, [1.0, 0.0, 0.0, 1.0], [200.0, 100.0], [100.0, 50.0]));
        let mut m = rect(2, [1.0, 1.0, 1.0, 1.0], [20.0, 20.0], [50.0, 50.0]);
        m.mask = MaskMode::Mask;
        p.layers.push(m);
        let f = render_project(&p, 0.0, 1.0, &Assets::default());
        assert!(at(&f, 50, 50)[0] > 0.99, "maske içi kırmızı kalmalı");
        let out = at(&f, 150, 50);
        assert!(out[0] < 0.01 && out[2] > 0.99, "maske dışı arka plan görünmeli");
    }

    #[test]
    fn parent_transform_carries_child() {
        let mut p = proj();
        let mut parent = Layer::new(1, "n", Content::Null, [100.0, 0.0], 10.0);
        parent.tf.pos = Track::new([100.0, 0.0]);
        let mut c = rect(2, [1.0, 1.0, 1.0, 1.0], [10.0, 10.0], [20.0, 50.0]);
        c.parent = Some(1);
        p.layers.push(parent);
        p.layers.push(c);
        let f = render_project(&p, 0.0, 1.0, &Assets::default());
        assert!(at(&f, 120, 50)[0] > 0.99);
        assert!(at(&f, 20, 50)[0] < 0.01);
    }

    #[test]
    fn multiply_blend_uses_background() {
        let mut p = proj();
        p.bg = [0.5, 0.5, 0.5, 1.0];
        let mut l = rect(1, [0.5, 1.0, 1.0, 1.0], [200.0, 100.0], [100.0, 50.0]);
        l.blend = Blend::Multiply;
        p.layers.push(l);
        let f = render_project(&p, 0.0, 1.0, &Assets::default());
        assert!((at(&f, 100, 50)[0] - 0.25).abs() < 0.02);
    }

    #[test]
    fn group_composites_children() {
        let mut p = proj();
        let child = rect(2, [0.0, 1.0, 0.0, 1.0], [20.0, 20.0], [100.0, 50.0]);
        let g = Layer::new(1, "g", Content::Group { children: vec![child] }, [130.0, 50.0], 10.0);
        p.layers.push(g);
        let f = render_project(&p, 0.0, 1.0, &Assets::default());
        // Group moved 30 px right of its centered rest position.
        assert!(at(&f, 130, 50)[1] > 0.99);
        assert!(at(&f, 100, 50)[1] < 0.01);
    }
}
