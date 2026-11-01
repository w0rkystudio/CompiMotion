//! The canvas: shows the render and handles selection, transform handles and drawing tools.
use crate::app::*;
use crate::media::Assets;
use crate::model::*;
use crate::render::{self, Mat3, apply};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use std::sync::Arc;

/// Local-space box of a layer, as (min, max).
pub fn local_bounds(ed: &mut Editor, id: u64, t: f64, assets: &Arc<Assets>) -> Option<([f32; 2], [f32; 2])> {
    let (cw, ch) = (ed.p.w as f32, ed.p.h as f32);
    let l = ed.p.find(id)?;
    let lt = l.local(t);
    let centered = |w: f32, h: f32| Some(([-w / 2.0, -h / 2.0], [w / 2.0, h / 2.0]));
    match &l.content {
        Content::Shape(s) => {
            let (polys, _) = crate::vector::shape_polys(s, lt);
            let (mut a, mut b) = ([f32::MAX; 2], [f32::MIN; 2]);
            for q in polys.iter().flatten() {
                a = [a[0].min(q[0]), a[1].min(q[1])];
                b = [b[0].max(q[0]), b[1].max(q[1])];
            }
            (a[0] <= b[0]).then_some((a, b))
        }
        Content::Text(tx) => {
            let r = crate::vector::raster_text(l, tx, lt, &crate::effects::Mods::default(), 0.25, assets)?;
            Some((r.min, [r.min[0] + r.frame.w as f32 / r.ss, r.min[1] + r.frame.h as f32 / r.ss]))
        }
        Content::Image { path } | Content::Video { path } => {
            let path = path.clone();
            let is_video = matches!(l.content, Content::Video { .. });
            let (w, h) = match ed.media_dims.get(&path) {
                Some(d) => *d,
                None => {
                    let d = if is_video {
                        assets.video_info(&path).map(|i| (i.0 as f32, i.1 as f32))
                    } else {
                        assets.image(&path).map(|f| (f.w as f32, f.h as f32))
                    }?;
                    ed.media_dims.insert(path, d);
                    d
                }
            };
            centered(w, h)
        }
        Content::Group { .. } => centered(cw, ch),
        Content::Null | Content::Camera { .. } => centered(40.0, 40.0),
        Content::Audio { .. } => None,
    }
}

fn flat(v: &[Layer], out: &mut Vec<u64>) {
    for l in v {
        out.push(l.id);
    }
}

fn matrix_of(ed: &Editor, id: u64, t: f64) -> Option<Mat3> {
    let l = ed.p.find(id)?;
    let tiny = crate::frame::Frame::new(1, 1);
    let mods = render::run_mods(l, l.local(t), ed.p.fps, 1.0, &tiny);
    let cam = render::find_camera(&ed.p, t);
    let all: Vec<&Layer> = ed.p.layers.iter().collect();
    Some(render::world_matrix(l, &all, t, [ed.p.w as f32, ed.p.h as f32], cam.as_ref(), &mods))
}

fn hit_layer(ed: &mut Editor, t: f64, q: [f32; 2], assets: &Arc<Assets>) -> Option<u64> {
    let mut ids = vec![];
    flat(&ed.p.layers, &mut ids);
    for id in ids.into_iter().rev() {
        let Some(l) = ed.p.find(id) else { continue };
        if !l.active(t) || l.locked || matches!(l.content, Content::Audio { .. } | Content::Camera { .. } | Content::Null) {
            continue;
        }
        let Some(m) = matrix_of(ed, id, t) else { continue };
        let Some(inv) = render::invert(&m) else { continue };
        let Some((a, b)) = local_bounds(ed, id, t, assets) else { continue };
        let u = apply(&inv, q[0], q[1]);
        // Thin shapes such as lines still need a usable click target.
        let pad = 6.0;
        if u[0] >= a[0] - pad && u[0] <= b[0] + pad && u[1] >= a[1] - pad && u[1] <= b[1] + pad {
            return Some(id);
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq)]
enum Grip {
    Body,
    Corner,
    Rotate,
}

pub fn show(ed: &mut Editor, ui: &mut egui::Ui, assets: &Arc<Assets>) {
    let avail = ui.available_rect_before_wrap();
    ed.preview_rect = avail;
    let (cw, ch) = (ed.p.w as f32, ed.p.h as f32);
    let zoom = ((avail.width() - 32.0) / cw).min((avail.height() - 32.0) / ch).max(0.01);
    let size = vec2(cw * zoom, ch * zoom);
    let rect = Rect::from_center_size(avail.center(), size);
    let painter = ui.painter_at(avail);

    // Checkerboard shows through transparent project backgrounds.
    painter.rect_filled(rect, 0.0, Color32::from_gray(40));
    let cell = 14.0;
    let (mut y, mut row) = (rect.top(), 0);
    if ed.p.bg[3] < 1.0 {
        while y < rect.bottom() {
            let mut x = rect.left() + if row % 2 == 0 { 0.0 } else { cell };
            while x < rect.right() {
                let r = Rect::from_min_size(pos2(x, y), vec2(cell, cell)).intersect(rect);
                painter.rect_filled(r, 0.0, Color32::from_gray(58));
                x += cell * 2.0;
            }
            y += cell;
            row += 1;
        }
    }
    if let Some(tex) = &ed.tex {
        painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    if ed.show_grid {
        let c = Color32::from_white_alpha(50);
        for k in 1..3 {
            let (fx, fy) = (rect.left() + rect.width() * k as f32 / 3.0, rect.top() + rect.height() * k as f32 / 3.0);
            painter.line_segment([pos2(fx, rect.top()), pos2(fx, rect.bottom())], Stroke::new(1.0, c));
            painter.line_segment([pos2(rect.left(), fy), pos2(rect.right(), fy)], Stroke::new(1.0, c));
        }
    }
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_gray(70)), StrokeKind::Outside);

    // Messages sit over the canvas, where they cannot push toolbar buttons around.
    if let Some(s) = &ed.status {
        let at = avail.left_bottom() + vec2(10.0, -8.0);
        let galley = painter.layout(s.clone(), egui::FontId::proportional(13.0), Color32::WHITE, avail.width() - 20.0);
        let r = Rect::from_min_size(at - vec2(0.0, galley.size().y), galley.size()).expand(5.0);
        painter.rect_filled(r, 4.0, Color32::from_black_alpha(200));
        painter.galley(r.min + vec2(5.0, 5.0), galley, Color32::WHITE);
    }

    let to_screen = |q: [f32; 2]| rect.min + vec2(q[0] * zoom, q[1] * zoom);
    let to_proj = |s: Pos2| [(s.x - rect.min.x) / zoom, (s.y - rect.min.y) / zoom];
    // Inset so the panels' resize handles, which straddle these edges, stay reachable.
    let resp = ui.interact(avail.shrink(8.0), ui.scope_id().with("canvas"), Sense::click_and_drag());
    let t = ed.t;

    match ed.tool {
        Tool::Pen => pen_tool(ed, ui, &resp, &painter, &to_screen, &to_proj),
        Tool::Freehand => freehand_tool(ed, &resp, &painter, &to_screen, &to_proj),
        Tool::Select => {}
    }
    if ed.tool != Tool::Select {
        return;
    }

    // Other selected layers get a plain outline; handles belong to the primary one.
    for other in ed.sel.clone() {
        if Some(other) == ed.selected() || !ed.p.find(other).is_some_and(|l| l.active(t)) {
            continue;
        }
        if let (Some(m), Some((a, b))) = (matrix_of(ed, other, t), local_bounds(ed, other, t, assets)) {
            let corners = [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]].map(|c| to_screen(apply(&m, c[0], c[1])));
            painter.add(egui::Shape::closed_line(corners.to_vec(), Stroke::new(1.0, ACCENT)));
        }
    }

    let shift = ui.input(|i| i.modifiers.shift);
    // Selection outline and handles.
    let mut grips = None;
    if let Some(id) = ed.selected() {
        if ed.p.find(id).is_some_and(|l| l.active(t)) {
            if let (Some(m), Some((a, b))) = (matrix_of(ed, id, t), local_bounds(ed, id, t, assets)) {
                let corners = [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]].map(|c| to_screen(apply(&m, c[0], c[1])));
                painter.add(egui::Shape::closed_line(corners.to_vec(), Stroke::new(1.5, ACCENT)));
                let top = pos2((corners[0].x + corners[1].x) / 2.0, (corners[0].y + corners[1].y) / 2.0);
                let c = to_screen(apply(&m, (a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0));
                let dir = (top - c).normalized();
                let rot = top + dir * 28.0;
                painter.line_segment([top, rot], Stroke::new(1.5, ACCENT));
                painter.circle_filled(rot, 6.0, ACCENT2);
                for q in corners {
                    painter.rect_filled(Rect::from_center_size(q, vec2(9.0, 9.0)), 1.0, Color32::WHITE);
                }
                painter.circle_stroke(to_screen(apply(&m, ed.p.find(id).map_or(0.0, |l| l.tf.anchor.at(l.local(t))[0]), ed.p.find(id).map_or(0.0, |l| l.tf.anchor.at(l.local(t))[1]))), 5.0, Stroke::new(1.5, ACCENT));
                grips = Some((corners, rot, c));
            }
        }
    }

    let hover = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
    let grip_id = ui.scope_id().with("grip");
    if resp.drag_started() {
        let press = resp.interact_pointer_pos().unwrap_or_default();
        let mut g = None;
        if let Some((corners, rot, _)) = grips {
            if press.distance(rot) < 12.0 {
                g = Some(Grip::Rotate);
            } else if corners.iter().any(|c| press.distance(*c) < 11.0) {
                g = Some(Grip::Corner);
            }
        }
        if g.is_none() {
            let pick = hit_layer(ed, t, to_proj(press), assets);
            if let Some(h) = pick {
                if !ed.sel.contains(&h) {
                    if shift {
                        ed.sel.push(h);
                    } else {
                        ed.sel = vec![h];
                    }
                }
                g = Some(Grip::Body);
            }
        }
        ui.data_mut(|d| d.insert_temp(grip_id, g.map(|x| x as u8 + 1).unwrap_or(0)));
    }
    if resp.dragged() {
        let code: u8 = ui.data(|d| d.get_temp(grip_id)).unwrap_or(0);
        if let (Some(id), Some(now)) = (ed.selected(), hover) {
            let delta = resp.drag_delta() / zoom;
            let center = grips.map(|g| g.2);
            let prev = now - resp.drag_delta();
            if ed.p.find(id).is_some_and(|l| !l.locked) {
                if let Some(l) = ed.p.find_mut(id) {
                    let lt = l.local(t);
                    match code {
                        1 => {
                            let q = l.tf.pos.at(lt);
                            l.tf.pos.set(lt, [q[0] + delta.x, q[1] + delta.y]);
                        }
                        2 => {
                            if let Some(c) = center {
                                let k = (now.distance(c) / prev.distance(c).max(1.0)).clamp(0.5, 2.0);
                                let s = l.tf.scale.at(lt);
                                l.tf.scale.set(lt, [s[0] * k, s[1] * k]);
                            }
                        }
                        3 => {
                            if let Some(c) = center {
                                let (a0, a1) = ((prev - c).angle(), (now - c).angle());
                                let r = l.tf.rot.at(lt);
                                l.tf.rot.set(lt, r + (a1 - a0).to_degrees());
                            }
                        }
                        _ => {}
                    }
                    if code == 1 {
                        for other in ed.top_selection() {
                            if other == id {
                                continue;
                            }
                            if let Some(o) = ed.p.find_mut(other).filter(|o| !o.locked) {
                                let lt = o.local(t);
                                let q = o.tf.pos.at(lt);
                                o.tf.pos.set(lt, [q[0] + delta.x, q[1] + delta.y]);
                            }
                        }
                    }
                    if code != 0 {
                        ed.touch();
                    }
                }
            }
        }
    }
    if resp.clicked() {
        if let Some(press) = resp.interact_pointer_pos() {
            let h = hit_layer(ed, t, to_proj(press), assets);
            if shift {
                // Shift+click toggles one layer and leaves the rest of the selection alone.
                if let Some(h) = h {
                    match ed.sel.iter().position(|s| *s == h) {
                        Some(i) => {
                            ed.sel.remove(i);
                        }
                        None => ed.sel.push(h),
                    }
                }
            } else {
                ed.sel = h.into_iter().collect();
                ed.sel_keys.clear();
            }
        }
    }
    path_node_handles(ed, ui, t, &to_screen, zoom);
}

/// Draggable nodes for the selected bezier path.
fn path_node_handles(ed: &mut Editor, ui: &mut egui::Ui, t: f64, to_screen: &impl Fn([f32; 2]) -> Pos2, zoom: f32) {
    let Some(id) = ed.selected() else { return };
    let Some(m) = matrix_of(ed, id, t) else { return };
    let Some(inv) = render::invert(&m) else { return };
    let Some(l) = ed.p.find(id) else { return };
    let Content::Shape(sh) = &l.content else { return };
    if sh.kind != ShapeKind::Path || sh.freehand || !l.active(t) {
        return;
    }
    let nodes: Vec<PathPt> = sh.path.clone();
    let mut changed = None;
    let painter = ui.painter().clone();
    for (i, n) in nodes.iter().enumerate() {
        let scr = to_screen(apply(&m, n.p[0], n.p[1]));
        let r = ui.interact(Rect::from_center_size(scr, vec2(14.0, 14.0)), ui.scope_id().with(("node", id, i)), Sense::drag());
        painter.rect_filled(Rect::from_center_size(scr, vec2(9.0, 9.0)), 2.0, if r.hovered() { ACCENT2 } else { Color32::WHITE });
        if r.dragged() {
            let d = r.drag_delta() / zoom;
            let o = apply(&inv, 0.0, 0.0);
            let e = apply(&inv, d.x, d.y);
            changed = Some((i, [e[0] - o[0], e[1] - o[1]]));
        }
    }
    if let Some((i, d)) = changed {
        if let Some(l) = ed.p.find_mut(id) {
            if let Content::Shape(sh) = &mut l.content {
                sh.path[i].p[0] += d[0];
                sh.path[i].p[1] += d[1];
            }
        }
        ed.touch();
    }
}

fn finish_path(ed: &mut Editor, pts: Vec<PathPt>, closed: bool, freehand: bool) {
    if pts.len() < 2 {
        return;
    }
    // Path nodes are stored relative to the layer position, at the project center.
    let (cx, cy) = (ed.p.w as f32 / 2.0, ed.p.h as f32 / 2.0);
    let path = pts.into_iter().map(|q| PathPt { p: [q.p[0] - cx, q.p[1] - cy], ..q }).collect();
    let sh = ShapeData { kind: ShapeKind::Path, size: Track::new([0.0, 0.0]), radius: Track::new(0.0), sides: Track::new(5.0), inner: Track::new(0.5), path, closed, freehand };
    let id = ed.add_layer(if freehand { "Çizim" } else { "Yol" }, Content::Shape(sh));
    let width = (ed.p.h as f32 * 0.008).max(2.0);
    if let Some(l) = ed.p.find_mut(id) {
        l.border = Some(Border { color: Track::new([1.0, 1.0, 1.0, 1.0]), width: Track::new(width) });
        if !closed || freehand {
            l.fill.kind = FillKind::None;
        }
    }
}

fn pen_tool(ed: &mut Editor, ui: &mut egui::Ui, resp: &egui::Response, painter: &egui::Painter, to_screen: &impl Fn([f32; 2]) -> Pos2, to_proj: &impl Fn(Pos2) -> [f32; 2]) {
    if resp.drag_started() {
        if let Some(q) = resp.interact_pointer_pos() {
            ed.pen.push(PathPt { p: to_proj(q), h_in: [0.0, 0.0], h_out: [0.0, 0.0] });
        }
    }
    if resp.clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            let p = to_proj(q);
            // Clicking the first node closes the path.
            if ed.pen.len() > 2 && (to_screen(ed.pen[0].p)).distance(q) < 10.0 {
                let pts = std::mem::take(&mut ed.pen);
                finish_path(ed, pts, true, false);
                ed.tool = Tool::Select;
                return;
            }
            ed.pen.push(PathPt { p, h_in: [0.0, 0.0], h_out: [0.0, 0.0] });
        }
    }
    if resp.dragged() {
        // Dragging pulls smooth handles out of the node just placed.
        if let (Some(last), Some(q)) = (ed.pen.last_mut(), resp.interact_pointer_pos()) {
            let c = to_proj(q);
            last.h_out = [c[0] - last.p[0], c[1] - last.p[1]];
            last.h_in = [-last.h_out[0], -last.h_out[1]];
        }
    }
    let finish = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if finish || resp.double_clicked() {
        let pts = std::mem::take(&mut ed.pen);
        finish_path(ed, pts, false, false);
        ed.tool = Tool::Select;
        return;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        ed.pen.clear();
        ed.tool = Tool::Select;
        return;
    }
    let line: Vec<Pos2> = crate::vector::flatten_path(&ed.pen, false).into_iter().map(to_screen).collect();
    if line.len() > 1 {
        painter.add(egui::Shape::line(line, Stroke::new(2.0, ACCENT2)));
    }
    for n in &ed.pen {
        painter.rect_filled(Rect::from_center_size(to_screen(n.p), vec2(8.0, 8.0)), 1.0, Color32::WHITE);
    }
    painter.text(resp.rect.left_top() + vec2(10.0, 10.0), egui::Align2::LEFT_TOP, "Kalem: tıkla ve nokta ekle, sürükle ve eğri yap. Enter bitirir, ilk noktaya tıklamak kapatır.", egui::FontId::proportional(13.0), TEXT_DIM);
}

fn freehand_tool(ed: &mut Editor, resp: &egui::Response, painter: &egui::Painter, to_screen: &impl Fn([f32; 2]) -> Pos2, to_proj: &impl Fn(Pos2) -> [f32; 2]) {
    if resp.drag_started() {
        ed.stroke.clear();
    }
    if resp.dragged() {
        if let Some(q) = resp.interact_pointer_pos() {
            let p = to_proj(q);
            if ed.stroke.last().is_none_or(|l| (l[0] - p[0]).hypot(l[1] - p[1]) > 2.0) {
                ed.stroke.push(p);
            }
        }
    }
    if resp.drag_stopped() {
        // Light smoothing removes mouse jitter while keeping the stroke's ends in place.
        let s = std::mem::take(&mut ed.stroke);
        let smooth: Vec<PathPt> = (0..s.len()).map(|i| {
            let a = s[i.saturating_sub(1)];
            let b = s[i];
            let c = s[(i + 1).min(s.len() - 1)];
            PathPt { p: [(a[0] + 2.0 * b[0] + c[0]) / 4.0, (a[1] + 2.0 * b[1] + c[1]) / 4.0], h_in: [0.0, 0.0], h_out: [0.0, 0.0] }
        }).collect();
        finish_path(ed, smooth, false, true);
    }
    let line: Vec<Pos2> = ed.stroke.iter().map(|q| to_screen(*q)).collect();
    if line.len() > 1 {
        painter.add(egui::Shape::line(line, Stroke::new(2.0, Color32::WHITE)));
    }
    painter.text(resp.rect.left_top() + vec2(10.0, 10.0), egui::Align2::LEFT_TOP, "Serbest çizim: sürükleyerek çiz. V ile seçime dön.", egui::FontId::proportional(13.0), TEXT_DIM);
}
