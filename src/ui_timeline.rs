//! Timeline: transport, layer rows with clips, keyframes and the playhead.
use crate::app::*;
use crate::media::Assets;
use crate::model::*;
use eframe::egui;
use egui::{Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2};
use std::sync::Arc;

const HEADER_W: f32 = 210.0;
const ROW_H: f32 = 30.0;
const RULER_H: f32 = 24.0;
/// How far, in screen pixels, a Ctrl-dragged edge stays held on either side of a snap point.
const SNAP_PX: f32 = 12.0;

/// The nearest point within `radius` of `raw`.
fn snap(raw: f64, points: &[f64], radius: f64) -> Option<f64> {
    points.iter().copied().filter(|q| (q - raw).abs() <= radius).min_by(|a, b| (a - raw).abs().total_cmp(&(b - raw).abs()))
}

fn fmt_time(t: f64, fps: f32) -> String {
    let s = t.max(0.0);
    let m = (s / 60.0) as u32;
    let f = (s.fract() * fps as f64) as u32;
    format!("{:02}:{:02}:{:02}", m, s as u32 % 60, f)
}

enum Row {
    Layer { id: u64, depth: usize, name: String, tag: u8, start: f64, dur: f64, is_group: bool, hidden: bool, locked: bool, kind: &'static str },
    Prop { id: u64, name: String, times: Vec<f64>, start: f64 },
}

fn prop_label(name: &str) -> String {
    // Effect tracks are stored as "effect-id#index/param".
    if let Some((head, key)) = name.split_once('/') {
        let id = head.split('#').next().unwrap_or(head);
        if let Some(d) = crate::effects::find(id) {
            let lbl = d.params.iter().find(|q| q.key == key).map_or(key, |q| q.label);
            return format!("{}: {}", d.name, lbl);
        }
    }
    name.to_string()
}

fn kind_of(c: &Content) -> &'static str {
    match c {
        Content::Shape(_) => "Şekil",
        Content::Text(_) => "Metin",
        Content::Image { .. } => "Görüntü",
        Content::Video { .. } => "Video",
        Content::Audio { .. } => "Ses",
        Content::Group { .. } => "Grup",
        Content::Null => "Boş",
        Content::Camera { .. } => "Kamera",
    }
}

fn build_rows(layers: &mut [Layer], depth: usize, base: f64, ex: &std::collections::HashSet<u64>, out: &mut Vec<Row>) {
    for l in layers.iter_mut().rev() {
        let start = base + l.start;
        out.push(Row::Layer { id: l.id, depth, name: l.name.clone(), tag: l.color_tag, start, dur: l.dur, is_group: matches!(l.content, Content::Group { .. }), hidden: !l.visible, locked: l.locked, kind: kind_of(&l.content) });
        if ex.contains(&l.id) {
            let id = l.id;
            let mut props = vec![];
            l.visit_tracks(&mut |name, tr| {
                let times = tr.times();
                if !times.is_empty() {
                    props.push(Row::Prop { id, name: name.to_string(), times, start });
                }
            });
            out.extend(props);
            if let Content::Group { children } = &mut l.content {
                build_rows(children, depth + 1, start, ex, out);
            }
        }
    }
}

fn diamond(p: &egui::Painter, c: Pos2, r: f32, fill: Color32) {
    p.add(egui::Shape::convex_polygon(vec![pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - r, c.y)], fill, Stroke::new(1.0, Color32::from_black_alpha(160))));
}

pub fn show(ed: &mut Editor, ui: &mut egui::Ui, _assets: &Arc<Assets>) {
    // Transport bar
    egui::Frame::new().fill(BG2).inner_margin(6.0).show(ui, |ui| {
        // Wraps on narrow windows, so buttons never land on top of each other.
        ui.horizontal_wrapped(|ui| {
            if ui.button("⏮").on_hover_text("Başa dön (Home)").clicked() {
                ed.t = 0.0;
            }
            if ui.button("◀").on_hover_text("Önceki kare (←)").clicked() {
                ed.step_frame(-1);
            }
            let label = if ed.playing { "⏸ Duraklat" } else { "▶ Oynat" };
            if ui.add(egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(ACCENT)).on_hover_text("Boşluk").clicked() {
                ed.playing = !ed.playing;
            }
            if ui.button("▶|").on_hover_text("Sonraki kare (→)").clicked() {
                ed.step_frame(1);
            }
            ui.label(RichText::new(format!("{} / {}", fmt_time(ed.t, ed.p.fps), fmt_time(ed.p.dur, ed.p.fps))).monospace());
            ui.separator();
            if ui.button("Böl").on_hover_text("Katmanı oynatma kafasında böl (S)").clicked() {
                ed.split_selected();
            }
            if ui.button("Ortadan Böl").on_hover_text("Seçili klipleri kendi ortasından böl (Ctrl+Shift+D)").clicked() {
                ed.split_middle_selected();
            }
            if ui.button("Çoğalt").on_hover_text("Ctrl+D").clicked() {
                ed.duplicate_selected();
            }
            if ui.button("Sil").on_hover_text("Delete").clicked() {
                ed.delete_selected();
            }
            if ui.button("Grupla").on_hover_text("Ctrl+G. Shift+tık ile birden çok katman seç").clicked() {
                ed.group_selected();
            }
            if ui.button("Gruptan Çıkar").on_hover_text("Seçili katmanları bulundukları gruptan bir üst seviyeye al").clicked() {
                ed.remove_selected_from_group();
            }
            if ui.button("Grubu Çöz").clicked() {
                ed.ungroup_selected();
            }
            if ui.button("▲").on_hover_text("Öne al").clicked() {
                ed.move_selected(1);
            }
            if ui.button("▼").on_hover_text("Arkaya al").clicked() {
                ed.move_selected(-1);
            }
            ui.label("Yakınlaştır");
            ui.add(egui::Slider::new(&mut ed.tl_zoom, 8.0..=600.0).logarithmic(true).show_value(false));
        });
    });

    let area = ui.available_rect_before_wrap();
    // Claim the space, or the panel would shrink to its minimum around custom painting.
    ui.allocate_rect(area, Sense::hover());
    ed.timeline_rect = area;
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, BG1);
    let timeline_left = area.left() + HEADER_W;
    let x_of = |t: f64, ed_scroll: f32, z: f32| timeline_left + (t as f32) * z - ed_scroll;
    let (z, scroll) = (ed.tl_zoom, ed.tl_scroll);
    let t_of = |x: f32| ((x - timeline_left + scroll) / z) as f64;

    // Wheel scrolls rows; Shift+wheel pans; Ctrl+wheel zooms around the pointer.
    if ui.rect_contains_pointer(area) {
        let (d, ctrl, shift) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.ctrl, i.modifiers.shift));
        if ctrl {
            ed.tl_zoom = (ed.tl_zoom * (1.0 + d.y * 0.002)).clamp(8.0, 600.0);
        } else if shift {
            ed.tl_scroll = (ed.tl_scroll - d.y - d.x).max(0.0);
        } else {
            ed.tl_scroll = (ed.tl_scroll - d.x).max(0.0);
            ed.tl_vscroll = (ed.tl_vscroll - d.y).max(0.0);
        }
    }

    let expanded = ed.expanded.clone();
    let mut rows = vec![];
    build_rows(&mut ed.p.layers, 0, 0.0, &expanded, &mut rows);
    let content_h = rows.len() as f32 * ROW_H;
    ed.tl_vscroll = ed.tl_vscroll.min((content_h - (area.height() - RULER_H)).max(0.0));
    let vs = ed.tl_vscroll;

    // Ruler
    let ruler = Rect::from_min_max(pos2(timeline_left, area.top()), pos2(area.right(), area.top() + RULER_H));
    painter.rect_filled(ruler, 0.0, BG2);
    let step = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0].into_iter().find(|s| (*s as f32) * z >= 60.0).unwrap_or(600.0);
    let mut tick = (scroll / z / step as f32).floor() as f64 * step;
    while x_of(tick, scroll, z) < area.right() {
        let x = x_of(tick, scroll, z);
        if x >= timeline_left {
            painter.line_segment([pos2(x, ruler.bottom() - 6.0), pos2(x, ruler.bottom())], Stroke::new(1.0, TEXT_DIM));
            painter.line_segment([pos2(x, ruler.bottom()), pos2(x, area.bottom())], Stroke::new(1.0, Color32::from_white_alpha(8)));
            painter.text(pos2(x + 3.0, ruler.top() + 3.0), egui::Align2::LEFT_TOP, format!("{tick:.2}").trim_end_matches('0').trim_end_matches('.').to_string() + "s", FontId::proportional(11.0), TEXT_DIM);
        }
        tick += step;
    }
    let r_resp = ui.interact(ruler, ui.scope_id().with("ruler"), Sense::click_and_drag());
    if r_resp.dragged() || r_resp.clicked() {
        if let Some(q) = r_resp.interact_pointer_pos() {
            let snapped = ed.snap_time(t_of(q.x));
            ed.set_time(snapped);
            ed.playing = false;
        }
    }

    // Rows
    let rows_clip = Rect::from_min_max(pos2(area.left(), area.top() + RULER_H), area.right_bottom());
    let row_painter = painter.with_clip_rect(rows_clip);
    let mut action: Vec<Action> = vec![];
    let ctrl = ui.input(|i| i.modifiers.ctrl);
    // Shift adds to the selection; Ctrl+click keeps working the same way.
    let add_sel = ctrl || ui.input(|i| i.modifiers.shift);
    let sel = ed.sel.clone();
    let sel_keys = ed.sel_keys.clone();
    let groups: Vec<(u64, String)> = rows.iter().filter_map(|r| if let Row::Layer { id, name, is_group: true, .. } = r { Some((*id, name.clone())) } else { None }).collect();
    let t_now = ed.t;
    let has_clip = ed.key_clip.is_some();
    // What a Ctrl-drag can stick to: every clip's ends, the playhead and the timeline start.
    let mut snap_points: Vec<(u64, f64)> = vec![(0, 0.0), (0, t_now)];
    for row in &rows {
        if let Row::Layer { id, start, dur, .. } = row {
            snap_points.push((*id, *start));
            snap_points.push((*id, *start + *dur));
        }
    }
    let mut snap_line: Option<f64> = None;
    for (i, row) in rows.iter().enumerate() {
        let y = area.top() + RULER_H + i as f32 * ROW_H - vs;
        if y + ROW_H < rows_clip.top() || y > rows_clip.bottom() {
            continue;
        }
        let head = Rect::from_min_size(pos2(area.left(), y), vec2(HEADER_W, ROW_H));
        let body = Rect::from_min_max(pos2(timeline_left, y), pos2(area.right(), y + ROW_H));
        match row {
            Row::Layer { id, depth, name, tag, start, dur, is_group, hidden, locked, kind } => {
                let id = *id;
                let selected = sel.contains(&id);
                row_painter.rect_filled(head, 0.0, if selected { BG3 } else { BG2 });
                row_painter.line_segment([head.left_bottom(), pos2(area.right(), head.bottom())], Stroke::new(1.0, BG0));
                let col = TAGS[*tag as usize % TAGS.len()];
                let mut x = head.left() + 6.0 + *depth as f32 * 12.0;
                row_painter.rect_filled(Rect::from_min_size(pos2(x, y + 6.0), vec2(4.0, ROW_H - 12.0)), 2.0, col);
                x += 8.0;
                // Header buttons are placed first so they take the click over the row.
                let arrow = Rect::from_min_size(pos2(x, y + 5.0), vec2(16.0, 20.0));
                let a_r = ui.interact(arrow.intersect(rows_clip), ui.scope_id().with(("exp", id)), Sense::click());
                row_painter.text(arrow.center(), egui::Align2::CENTER_CENTER, if expanded.contains(&id) { "▾" } else { "▸" }, FontId::proportional(13.0), TEXT_DIM);
                if a_r.clicked() {
                    action.push(Action::ToggleExpand(id));
                }
                x += 18.0;
                let eye = Rect::from_min_size(pos2(head.right() - 44.0, y + 5.0), vec2(20.0, 20.0));
                let lock = Rect::from_min_size(pos2(head.right() - 22.0, y + 5.0), vec2(20.0, 20.0));
                let e_r = ui.interact(eye.intersect(rows_clip), ui.scope_id().with(("eye", id)), Sense::click()).on_hover_text("Göster/gizle");
                let l_r = ui.interact(lock.intersect(rows_clip), ui.scope_id().with(("lock", id)), Sense::click()).on_hover_text("Kilitle");
                row_painter.text(eye.center(), egui::Align2::CENTER_CENTER, if *hidden { "○" } else { "●" }, FontId::proportional(12.0), if *hidden { TEXT_DIM } else { Color32::WHITE });
                row_painter.text(lock.center(), egui::Align2::CENTER_CENTER, if *locked { "■" } else { "□" }, FontId::proportional(12.0), if *locked { ACCENT2 } else { TEXT_DIM });
                if e_r.clicked() {
                    action.push(Action::ToggleHide(id));
                }
                if l_r.clicked() {
                    action.push(Action::ToggleLock(id));
                }
                let text_rect = Rect::from_min_max(pos2(x, y), pos2(eye.left() - 2.0, y + ROW_H));
                let p2 = row_painter.with_clip_rect(text_rect.intersect(rows_clip));
                p2.text(pos2(x, y + 4.0), egui::Align2::LEFT_TOP, name, FontId::proportional(13.0), Color32::WHITE);
                p2.text(pos2(x, y + 17.0), egui::Align2::LEFT_TOP, *kind, FontId::proportional(10.0), TEXT_DIM);
                let h_r = ui.interact(text_rect.intersect(rows_clip), ui.scope_id().with(("head", id)), Sense::click_and_drag());
                if h_r.clicked() {
                    action.push(Action::Select(id, add_sel));
                }
                if h_r.drag_started() {
                    ed.drag_row = Some(id);
                }
                h_r.context_menu(|ui| layer_menu(ui, id, has_clip, *depth, &groups, &mut action));

                // Clip body
                let (x0, x1) = (x_of(*start, scroll, z), x_of(*start + *dur, scroll, z));
                let clip = Rect::from_min_max(pos2(x0, y + 3.0), pos2(x1.max(x0 + 2.0), y + ROW_H - 3.0));
                let vis = clip.intersect(body).intersect(rows_clip);
                let body_clip = row_painter.with_clip_rect(body.intersect(rows_clip));
                body_clip.rect_filled(clip, 4.0, if selected { col } else { col.gamma_multiply(0.55) });
                if *hidden {
                    body_clip.rect_filled(clip, 4.0, Color32::from_black_alpha(140));
                }
                let lbl = if *is_group { format!("▣ {name}") } else { name.clone() };
                body_clip.with_clip_rect(vis).text(pos2(clip.left().max(timeline_left) + 6.0, y + ROW_H / 2.0), egui::Align2::LEFT_CENTER, lbl, FontId::proportional(12.0), Color32::WHITE);
                if vis.width() > 0.0 && vis.height() > 0.0 && !*locked {
                    let edge = 6.0;
                    let left = Rect::from_min_max(clip.left_top(), pos2(clip.left() + edge, clip.bottom())).intersect(vis);
                    let right = Rect::from_min_max(pos2(clip.right() - edge, clip.top()), clip.right_bottom()).intersect(vis);
                    let mid = ui.interact(vis, ui.scope_id().with(("clip", id)), Sense::click_and_drag());
                    let l_e = ui.interact(left, ui.scope_id().with(("cl", id)), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                    let r_e = ui.interact(right, ui.scope_id().with(("cr", id)), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                    if mid.clicked() {
                        action.push(Action::Select(id, add_sel));
                    }
                    // Dragging a clip that is already selected carries the whole selection.
                    if (mid.drag_started() || l_e.drag_started() || r_e.drag_started()) && !selected {
                        action.push(Action::Select(id, add_sel));
                    }
                    mid.context_menu(|ui| layer_menu(ui, id, has_clip, *depth, &groups, &mut action));
                    // The unsnapped position follows the mouse, so a held edge lets go
                    // once the pointer has really travelled past the sticky zone.
                    if mid.drag_started() {
                        ed.clip_drag_raw = *start;
                    } else if l_e.drag_started() {
                        ed.clip_drag_raw = *start;
                    } else if r_e.drag_started() {
                        ed.clip_drag_raw = *start + *dur;
                    }
                    let radius = (SNAP_PX / z) as f64;
                    // Selected clips travel along, so they are not something to stick to.
                    let others: Vec<f64> = snap_points.iter().filter(|q| q.0 != id && !sel.contains(&q.0)).map(|q| q.1).collect();
                    if mid.dragged() {
                        ed.clip_drag_raw += (mid.drag_delta().x / z) as f64;
                        let (mut to, mut line) = (ed.clip_drag_raw, None);
                        if ctrl {
                            // Whichever end of the clip is closer to something sticks.
                            let by_start = snap(ed.clip_drag_raw, &others, radius);
                            let by_end = snap(ed.clip_drag_raw + *dur, &others, radius);
                            match (by_start, by_end) {
                                (Some(a), Some(b)) if (b - *dur - ed.clip_drag_raw).abs() < (a - ed.clip_drag_raw).abs() => {
                                    to = b - *dur;
                                    line = Some(b);
                                }
                                (Some(a), _) => {
                                    to = a;
                                    line = Some(a);
                                }
                                (None, Some(b)) => {
                                    to = b - *dur;
                                    line = Some(b);
                                }
                                _ => {}
                            }
                        }
                        snap_line = line.or(snap_line);
                        if to != *start {
                            action.push(Action::MoveClip(id, to - *start));
                        }
                    }
                    for (resp, edge, right_side) in [(&l_e, *start, false), (&r_e, *start + *dur, true)] {
                        if !resp.dragged() {
                            continue;
                        }
                        ed.clip_drag_raw += (resp.drag_delta().x / z) as f64;
                        let held = if ctrl { snap(ed.clip_drag_raw, &others, radius) } else { None };
                        snap_line = held.or(snap_line);
                        let to = held.unwrap_or(ed.clip_drag_raw);
                        if to != edge {
                            action.push(if right_side { Action::TrimRight(id, to - edge) } else { Action::TrimLeft(id, to - edge) });
                        }
                    }
                }
                // Each keyframe time shown on the collapsed clip.
                if !expanded.contains(&id) {
                    if let Some(l) = ed.p.find_mut(id) {
                        let mut ts = vec![];
                        l.visit_tracks(&mut |_, tr| ts.extend(tr.times()));
                        for k in ts {
                            let kx = x_of(*start + k, scroll, z);
                            if kx >= timeline_left {
                                diamond(&body_clip, pos2(kx, y + ROW_H - 7.0), 3.5, Color32::from_rgb(255, 225, 120));
                            }
                        }
                    }
                }
            }
            Row::Prop { id, name, times, start } => {
                let id = *id;
                row_painter.rect_filled(head, 0.0, BG1);
                row_painter.line_segment([head.left_bottom(), pos2(area.right(), head.bottom())], Stroke::new(1.0, BG0));
                let p2 = row_painter.with_clip_rect(head.intersect(rows_clip));
                p2.text(pos2(head.left() + 38.0, y + ROW_H / 2.0), egui::Align2::LEFT_CENTER, prop_label(name), FontId::proportional(12.0), TEXT_DIM);
                let p_r = ui.interact(head.intersect(rows_clip), ui.scope_id().with(("phead", id, name)), Sense::click());
                p_r.context_menu(|ui| {
                    if ui.button("Bu özelliğin keyframe'lerini kopyala").clicked() {
                        action.push(Action::CopyTrack(id, name.clone()));
                        ui.close();
                    }
                    if ui.add_enabled(has_clip, egui::Button::new("Buraya yapıştır")).clicked() {
                        action.push(Action::PasteToTrack(id, name.clone()));
                        ui.close();
                    }
                });
                let body_clip = row_painter.with_clip_rect(body.intersect(rows_clip));
                for (k, kt) in times.iter().enumerate() {
                    let c = pos2(x_of(*start + kt, scroll, z), y + ROW_H / 2.0);
                    let is_sel = sel_keys.iter().any(|s| s.0 == id && s.1 == *name && s.2 == k);
                    diamond(&body_clip, c, 6.0, if is_sel { ACCENT2 } else { Color32::from_rgb(255, 225, 120) });
                    if c.x < timeline_left {
                        continue;
                    }
                    let r = Rect::from_center_size(c, vec2(14.0, 14.0)).intersect(rows_clip);
                    if r.height() <= 0.0 {
                        continue;
                    }
                    let resp = ui.interact(r, ui.scope_id().with(("key", id, name, k)), Sense::click_and_drag());
                    if resp.clicked() || (resp.drag_started() && !is_sel) {
                        action.push(Action::SelectKey(id, name.clone(), k, add_sel));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Kopyala").clicked() {
                            action.push(Action::CopyKey(id, name.clone(), k));
                            ui.close();
                        }
                        if ui.button("Sil").clicked() {
                            action.push(Action::RemoveKey(id, name.clone(), k));
                            ui.close();
                        }
                    });
                    if resp.dragged() {
                        if let Some(q) = resp.interact_pointer_pos() {
                            action.push(Action::MoveKey(id, name.clone(), k, t_of(q.x) - *start));
                        }
                    }
                }
            }
        }
    }

    // Row reordering by dragging a header.
    if let Some(drag_id) = ed.drag_row {
        if let Some(q) = ui.input(|i| i.pointer.interact_pos()) {
            let idx = ((q.y - area.top() - RULER_H + vs) / ROW_H).floor();
            let ly = area.top() + RULER_H + idx.max(0.0) * ROW_H - vs;
            row_painter.line_segment([pos2(area.left(), ly), pos2(timeline_left, ly)], Stroke::new(2.0, ACCENT2));
            if ui.input(|i| i.pointer.any_released()) {
                if let Some(Row::Layer { id: target, .. }) = rows.get(idx.max(0.0) as usize) {
                    action.push(Action::Reorder(drag_id, *target));
                }
                ed.drag_row = None;
            }
        } else {
            ed.drag_row = None;
        }
    }

    if let Some(q) = snap_line {
        let x = x_of(q, scroll, z);
        if x >= timeline_left {
            painter.line_segment([pos2(x, area.top()), pos2(x, area.bottom())], Stroke::new(1.5, Color32::from_rgb(255, 225, 120)));
        }
    }

    // Playhead
    let px = x_of(t_now, scroll, z);
    if px >= timeline_left {
        painter.line_segment([pos2(px, area.top()), pos2(px, area.bottom())], Stroke::new(1.5, ACCENT2));
        painter.add(egui::Shape::convex_polygon(vec![pos2(px - 6.0, area.top()), pos2(px + 6.0, area.top()), pos2(px, area.top() + 10.0)], ACCENT2, Stroke::NONE));
    }
    if rows.is_empty() {
        painter.text(rows_clip.center(), egui::Align2::CENTER_CENTER, "Soldan bir katman ekle ya da pencereye dosya sürükle.", FontId::proportional(14.0), TEXT_DIM);
    }

    let fps = ed.p.fps as f64;
    let ctx = ui.ctx().clone();
    for a in action {
        apply(ed, a, fps, &ctx);
    }
}

fn layer_menu(ui: &mut egui::Ui, id: u64, has_clip: bool, depth: usize, groups: &[(u64, String)], action: &mut Vec<Action>) {
    ui.menu_button("Gruba ekle", |ui| {
        for (g, name) in groups.iter().filter(|g| g.0 != id) {
            if ui.button(name).clicked() {
                action.push(Action::AddToGroup(id, *g));
                ui.close();
            }
        }
        if ui.button("+ Yeni grup").clicked() {
            action.push(Action::NewGroup(id));
            ui.close();
        }
    });
    if depth > 0 && ui.button("Gruptan çıkar").clicked() {
        action.push(Action::RemoveFromGroup(id));
        ui.close();
    }
    if ui.button("Ortadan böl").on_hover_text("Ctrl+Shift+D").clicked() {
        action.push(Action::SplitMiddle(id));
        ui.close();
    }
    ui.separator();
    if ui.button("Keyframe'leri kopyala").on_hover_text("Ctrl+C").clicked() {
        action.push(Action::CopyLayerKeys(id));
        ui.close();
    }
    if ui.add_enabled(has_clip, egui::Button::new("Keyframe'leri yapıştır")).on_hover_text("Ctrl+V. Oynatma kafasından başlar").clicked() {
        action.push(Action::PasteKeys(id));
        ui.close();
    }
    ui.separator();
    for (label, a) in [("Çoğalt", Action::Duplicate(id)), ("Oynatma kafasında böl", Action::Split(id)), ("Sil", Action::Delete(id)), ("Keyframe'leri göster", Action::ToggleExpand(id))] {
        if ui.button(label).clicked() {
            action.push(a);
            ui.close();
        }
    }
}

enum Action {
    Select(u64, bool),
    ToggleExpand(u64),
    ToggleHide(u64),
    ToggleLock(u64),
    MoveClip(u64, f64),
    TrimLeft(u64, f64),
    TrimRight(u64, f64),
    SelectKey(u64, String, usize, bool),
    AddToGroup(u64, u64),
    NewGroup(u64),
    RemoveFromGroup(u64),
    SplitMiddle(u64),
    RemoveKey(u64, String, usize),
    MoveKey(u64, String, usize, f64),
    Reorder(u64, u64),
    Duplicate(u64),
    Split(u64),
    Delete(u64),
    CopyKey(u64, String, usize),
    CopyTrack(u64, String),
    CopyLayerKeys(u64),
    PasteKeys(u64),
    PasteToTrack(u64, String),
}

fn with_track(ed: &mut Editor, id: u64, name: &str, f: &mut dyn FnMut(&mut dyn AnyTrack)) {
    if let Some(l) = ed.p.find_mut(id) {
        l.visit_tracks(&mut |n, tr| {
            if n == name {
                f(tr);
            }
        });
    }
}

/// Menu commands act on the selection if the clicked layer is in it, else on that layer alone.
fn select_for_menu(ed: &mut Editor, id: u64) {
    if !ed.sel.contains(&id) {
        ed.sel = vec![id];
    }
}

/// The layers a clip gesture on `id` applies to, skipping locked ones.
fn clip_targets(ed: &Editor, id: u64) -> Vec<u64> {
    let ids = if ed.sel.contains(&id) { ed.top_selection() } else { vec![id] };
    ids.into_iter().filter(|i| ed.p.find(*i).is_some_and(|l| !l.locked)).collect()
}

fn apply(ed: &mut Editor, a: Action, fps: f64, ctx: &egui::Context) {
    match a {
        Action::AddToGroup(id, g) => {
            select_for_menu(ed, id);
            ed.add_selected_to_group(g);
        }
        Action::NewGroup(id) => {
            select_for_menu(ed, id);
            ed.group_selected();
        }
        Action::RemoveFromGroup(id) => {
            select_for_menu(ed, id);
            ed.remove_selected_from_group();
        }
        Action::SplitMiddle(id) => {
            select_for_menu(ed, id);
            ed.split_middle_selected();
        }
        Action::CopyKey(id, name, k) => {
            if ed.sel_keys.len() > 1 && ed.sel_keys.iter().any(|s| s.0 == id && s.1 == name && s.2 == k) {
                ed.copy_selected_keys(ctx);
            } else {
                ed.copy_key(id, &name, k, ctx);
            }
        }
        Action::CopyTrack(id, name) => ed.copy_track(id, &name, ctx),
        Action::CopyLayerKeys(id) => ed.copy_layer_keys(id, ctx),
        Action::PasteKeys(id) => {
            // Pasting from a layer's own menu targets that layer.
            if !ed.sel.contains(&id) {
                ed.sel = vec![id];
            }
            ed.paste_keys(None);
        }
        Action::PasteToTrack(id, name) => ed.paste_keys_to_track(id, &name),
        Action::Select(id, add) => {
            if add {
                if let Some(i) = ed.sel.iter().position(|s| *s == id) {
                    ed.sel.remove(i);
                } else {
                    ed.sel.push(id);
                }
            } else {
                ed.sel = vec![id];
                ed.sel_keys.clear();
            }
        }
        Action::ToggleExpand(id) => {
            if !ed.expanded.remove(&id) {
                ed.expanded.insert(id);
            }
        }
        Action::ToggleHide(id) => {
            if let Some(l) = ed.p.find_mut(id) {
                l.visible = !l.visible;
            }
            ed.touch();
        }
        Action::ToggleLock(id) => {
            if let Some(l) = ed.p.find_mut(id) {
                l.locked = !l.locked;
            }
            ed.touch();
        }
        Action::MoveClip(id, d) => {
            let ids = clip_targets(ed, id);
            // The earliest clip stops at 0, which keeps the selection's spacing intact.
            let earliest = ids.iter().filter_map(|i| ed.p.find(*i).map(|l| l.start)).fold(f64::MAX, f64::min);
            let d = d.max(-earliest);
            for i in ids {
                if let Some(l) = ed.p.find_mut(i) {
                    l.start += d;
                }
            }
            ed.touch();
        }
        Action::TrimLeft(id, d) => {
            for i in clip_targets(ed, id) {
                if let Some(l) = ed.p.find_mut(i) {
                    // Keeps content in place on screen: the source and keys shift with the cut.
                    let d = d.max(-l.start).min(l.dur - 1.0 / fps);
                    let d = d.max(-l.in_point / l.speed.max(0.01) as f64);
                    l.start += d;
                    l.dur -= d;
                    l.in_point += d * l.speed as f64;
                    l.visit_tracks(&mut |_, tr| tr.shift(-d));
                }
            }
            ed.touch();
        }
        Action::TrimRight(id, d) => {
            for i in clip_targets(ed, id) {
                if let Some(l) = ed.p.find_mut(i) {
                    l.dur = (l.dur + d).max(1.0 / fps);
                }
            }
            ed.touch();
        }
        Action::SelectKey(id, name, k, add) => {
            let key = (id, name, k);
            if !add {
                ed.sel = vec![id];
                ed.sel_keys = vec![key];
            } else if let Some(i) = ed.sel_keys.iter().position(|s| *s == key) {
                ed.sel_keys.remove(i);
            } else {
                if !ed.sel.contains(&id) {
                    ed.sel.push(id);
                }
                ed.sel_keys.push(key);
            }
        }
        Action::RemoveKey(id, name, k) => {
            if ed.sel_keys.iter().any(|s| s.0 == id && s.1 == name && s.2 == k) {
                ed.delete_selected_keys();
            } else {
                with_track(ed, id, &name, &mut |tr| tr.remove_key(k));
                ed.sel_keys.clear();
                ed.touch();
            }
        }
        Action::MoveKey(id, name, k, t) => {
            ed.move_selected_keys(id, &name, k, (t * fps).round() / fps);
        }
        Action::Reorder(from, to) => {
            let ids = if ed.sel.contains(&from) { ed.top_selection() } else { vec![from] };
            if ids.contains(&to) {
                return;
            }
            let is_group = ed.p.find(to).is_some_and(|l| matches!(l.content, Content::Group { .. }));
            let to_parent = crate::ops::parent_of(&ed.p.layers, to);
            if is_group {
                // Dropping on a group row puts the layers into that group.
                if ed.move_layers(&ids, crate::ops::Dest::Group(to)) == 0 {
                    ed.status = Some("Bu seçim bu gruba eklenemez.".into());
                }
            } else {
                for i in ids {
                    if crate::ops::parent_of(&ed.p.layers, i) == to_parent {
                        reorder(&mut ed.p.layers, i, to);
                        ed.touch();
                    } else {
                        ed.move_layers(&[i], crate::ops::Dest::Above(to));
                    }
                }
            }
        }
        Action::Duplicate(id) => {
            ed.sel = vec![id];
            ed.duplicate_selected();
        }
        Action::Split(id) => {
            ed.sel = vec![id];
            ed.split_selected();
        }
        Action::Delete(id) => {
            ed.sel = vec![id];
            ed.delete_selected();
        }
    }
}

/// Moves `from` to `to`'s place when both live in the same stack.
fn reorder(v: &mut Vec<Layer>, from: u64, to: u64) -> bool {
    let (a, b) = (v.iter().position(|l| l.id == from), v.iter().position(|l| l.id == to));
    if let (Some(a), Some(b)) = (a, b) {
        let l = v.remove(a);
        v.insert(b, l);
        return true;
    }
    v.iter_mut().any(|l| if let Content::Group { children } = &mut l.content { reorder(children, from, to) } else { false })
}
