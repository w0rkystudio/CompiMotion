//! Left "add" panel, right property tabs, effect browser and the easing curve editor.
use crate::app::*;
use crate::effects::{self, Kind};
use crate::media::Assets;
use crate::model::*;
use eframe::egui;
use egui::{Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use std::sync::Arc;

pub fn add_panel(ed: &mut Editor, ui: &mut egui::Ui, assets: &Arc<Assets>) {
    let wide = |ui: &mut egui::Ui, s: &str| ui.add_sized([ui.available_width(), 26.0], egui::Button::new(s)).clicked();
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label(RichText::new("Araçlar").strong());
        ui.horizontal(|ui| {
            ui.selectable_value(&mut ed.tool, Tool::Select, "Seç").on_hover_text("V");
            ui.selectable_value(&mut ed.tool, Tool::Pen, "Kalem").on_hover_text("P");
            ui.selectable_value(&mut ed.tool, Tool::Freehand, "Çiz").on_hover_text("B");
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Ekle").strong());
        if wide(ui, "Metin") {
            ed.add_text();
        }
        if wide(ui, "Görüntü / Video") {
            if let Some(files) = rfd::FileDialog::new().add_filter("Medya", &["png", "jpg", "jpeg", "webp", "bmp", "gif", "mp4", "mov", "m4v", "avi", "wmv", "mkv", "webm"]).pick_files() {
                for f in files {
                    ed.import_media(&f, assets);
                }
            }
        }
        if wide(ui, "Ses") {
            if let Some(f) = rfd::FileDialog::new().add_filter("Ses", &["mp3", "wav", "aac", "m4a", "wma", "flac"]).pick_file() {
                ed.import_media(&f, assets);
            }
        }
        if wide(ui, "Yazı Tipi (TTF)") || ed.pick_font {
            ed.pick_font = false;
            if let Some(files) = rfd::FileDialog::new().add_filter("Yazı tipi", &["ttf", "otf"]).pick_files() {
                for f in files {
                    ed.add_font(&f, assets);
                }
            }
        }
        if wide(ui, "Boş Katman") {
            ed.add_layer("Boş", Content::Null);
        }
        if wide(ui, "Kamera") {
            ed.add_layer("Kamera", Content::Camera { fov: Track::new(50.0) });
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Şekiller").strong());
        for (k, name) in ShapeKind::ALL {
            if wide(ui, name) {
                ed.add_shape(*k, name);
            }
        }
    });
}

/// Key toggle shown before each animatable row; returns true when clicked.
fn key_button(ui: &mut egui::Ui, has_key_here: bool, animated: bool) -> bool {
    let col = if has_key_here { Color32::from_rgb(255, 225, 120) } else if animated { ACCENT } else { TEXT_DIM };
    let txt = if has_key_here { "◆" } else { "◇" };
    ui.add(egui::Button::new(RichText::new(txt).color(col)).frame(false)).on_hover_text("Bu anda keyframe ekle/kaldır").clicked()
}

fn row_f(ui: &mut egui::Ui, dirty: &mut bool, label: &str, tr: &mut Track<f32>, t: f64, range: std::ops::RangeInclusive<f32>, speed: f32) {
    ui.horizontal(|ui| {
        if key_button(ui, tr.key_index(t).is_some(), !tr.keys.is_empty()) {
            tr.toggle_key(t);
            *dirty = true;
        }
        ui.add_sized([96.0, 18.0], egui::Label::new(label).truncate());
        let mut v = tr.at(t);
        if ui.add(egui::DragValue::new(&mut v).speed(speed).range(range)).changed() {
            tr.set(t, v);
            *dirty = true;
        }
    });
}

fn row_v2(ui: &mut egui::Ui, dirty: &mut bool, label: &str, tr: &mut Track<[f32; 2]>, t: f64, speed: f32) {
    ui.horizontal(|ui| {
        if key_button(ui, tr.key_index(t).is_some(), !tr.keys.is_empty()) {
            tr.toggle_key(t);
            *dirty = true;
        }
        ui.add_sized([96.0, 18.0], egui::Label::new(label).truncate());
        let mut v = tr.at(t);
        let a = ui.add(egui::DragValue::new(&mut v[0]).speed(speed)).changed();
        let b = ui.add(egui::DragValue::new(&mut v[1]).speed(speed)).changed();
        if a || b {
            tr.set(t, v);
            *dirty = true;
        }
    });
}

fn row_color(ui: &mut egui::Ui, dirty: &mut bool, label: &str, tr: &mut Track<[f32; 4]>, t: f64) {
    ui.horizontal(|ui| {
        if key_button(ui, tr.key_index(t).is_some(), !tr.keys.is_empty()) {
            tr.toggle_key(t);
            *dirty = true;
        }
        ui.add_sized([96.0, 18.0], egui::Label::new(label).truncate());
        let mut v = tr.at(t);
        if ui.color_edit_button_rgba_unmultiplied(&mut v).changed() {
            tr.set(t, v);
            *dirty = true;
        }
    });
}

pub fn show(ed: &mut Editor, ui: &mut egui::Ui) {
    let Some(id) = ed.selected() else {
        // Scrolls on short windows; content taller than the panel would overflow it.
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Katman seçilmedi").strong());
                ui.label(RichText::new("Düzenlemek için önizlemede ya da zaman çizelgesinde bir katmana tıkla.").color(TEXT_DIM));
            });
            ui.separator();
            project_settings(ed, ui);
        });
        return;
    };
    ui.horizontal_wrapped(|ui| {
        for (tab, name) in [(Tab::Content, "İçerik"), (Tab::Transform, "Hareket"), (Tab::Color, "Renk"), (Tab::Border, "Kenar/Gölge"), (Tab::Effects, "Efektler"), (Tab::Blend, "Karışım")] {
            ui.selectable_value(&mut ed.tab, tab, name);
        }
    });
    ui.separator();
    let mut dirty = false;
    let mut open_browser = false;
    // Above the scroll area, which takes all the height left below it.
    curve_editor(ed, ui, &mut dirty);
    let parents: Vec<(u64, String)> = ed.p.layers.iter().filter(|l| l.id != id).map(|l| (l.id, l.name.clone())).collect();
    let fonts = std::mem::take(&mut ed.fonts);
    let mut clip = std::mem::take(&mut ed.effect_clip);
    let tab = ed.tab;
    let comp_t = ed.t;
    let mut pick_font = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let Some(l) = ed.p.find_mut(id) else { return };
        let t = l.local(comp_t);
        ui.horizontal(|ui| {
            ui.label("Ad");
            dirty |= ui.text_edit_singleline(&mut l.name).changed();
        });
        ui.add_space(4.0);
        match tab {
            Tab::Content => content_tab(ui, l, t, &fonts, &mut pick_font, &mut dirty),
            Tab::Transform => transform_tab(ui, l, t, &parents, &mut dirty),
            Tab::Color => color_tab(ui, l, t, &mut dirty),
            Tab::Border => border_tab(ui, l, t, &mut dirty),
            Tab::Effects => effects_tab(ui, l, t, &mut clip, &mut open_browser, &mut dirty),
            Tab::Blend => blend_tab(ui, l, t, &mut dirty),
        }
    });
    ed.fonts = fonts;
    ed.effect_clip = clip;
    ed.pick_font |= pick_font;
    if open_browser {
        ed.effect_browser = true;
        ed.effect_search.clear();
    }
    if dirty {
        ed.touch();
    }
}

fn project_settings(ed: &mut Editor, ui: &mut egui::Ui) {
    ui.label(RichText::new("Proje").strong());
    let mut dirty = false;
    ui.horizontal(|ui| {
        ui.label("Arka plan");
        dirty |= ui.color_edit_button_rgba_unmultiplied(&mut ed.p.bg).changed();
    });
    ui.horizontal(|ui| {
        ui.label("Boyut");
        dirty |= ui.add(egui::DragValue::new(&mut ed.p.w).range(16..=7680)).changed();
        dirty |= ui.add(egui::DragValue::new(&mut ed.p.h).range(16..=7680)).changed();
    });
    ui.horizontal(|ui| {
        ui.label("Kare hızı");
        dirty |= ui.add(egui::DragValue::new(&mut ed.p.fps).range(1.0..=120.0)).changed();
    });
    if dirty {
        ed.touch();
    }
}

fn content_tab(ui: &mut egui::Ui, l: &mut Layer, t: f64, fonts: &[String], pick_font: &mut bool, dirty: &mut bool) {
    let start_speed = l.speed;
    match &mut l.content {
        Content::Text(tx) => {
            *dirty |= ui.add(egui::TextEdit::multiline(&mut tx.text).desired_rows(3).desired_width(f32::INFINITY)).changed();
            ui.horizontal(|ui| {
                ui.label("Yazı tipi");
                egui::ComboBox::from_id_salt("font").width(150.0).selected_text(tx.font.clone()).show_ui(ui, |ui| {
                    for f in fonts {
                        if ui.selectable_label(tx.font == *f, f).clicked() {
                            tx.font = f.clone();
                            *dirty = true;
                        }
                    }
                });
                if ui.button("+ TTF").on_hover_text("Bilgisayarından TTF veya OTF dosyası ekle").clicked() {
                    *pick_font = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Hizalama");
                for (a, n) in [(Align::Left, "Sol"), (Align::Center, "Orta"), (Align::Right, "Sağ")] {
                    if ui.selectable_label(tx.align == a, n).clicked() {
                        tx.align = a;
                        *dirty = true;
                    }
                }
            });
            row_f(ui, dirty, "Yazı boyutu", &mut tx.size, t, 1.0..=2000.0, 1.0);
            row_f(ui, dirty, "Harf aralığı", &mut tx.tracking, t, -500.0..=2000.0, 0.5);
            row_f(ui, dirty, "Satır aralığı", &mut tx.leading, t, -500.0..=2000.0, 0.5);
        }
        Content::Shape(s) => {
            if s.kind != ShapeKind::Path {
                ui.horizontal(|ui| {
                    ui.label("Şekil");
                    egui::ComboBox::from_id_salt("kind").selected_text(ShapeKind::ALL.iter().find(|k| k.0 == s.kind).map_or("", |k| k.1)).show_ui(ui, |ui| {
                        for (k, n) in ShapeKind::ALL {
                            if ui.selectable_label(s.kind == *k, *n).clicked() {
                                s.kind = *k;
                                *dirty = true;
                            }
                        }
                    });
                });
                row_v2(ui, dirty, "Boyut", &mut s.size, t, 1.0);
                match s.kind {
                    ShapeKind::RoundRect => row_f(ui, dirty, "Köşe yarıçapı", &mut s.radius, t, 0.0..=4000.0, 1.0),
                    ShapeKind::Polygon => row_f(ui, dirty, "Köşe sayısı", &mut s.sides, t, 3.0..=64.0, 0.05),
                    ShapeKind::Star => {
                        row_f(ui, dirty, "Köşe sayısı", &mut s.sides, t, 3.0..=64.0, 0.05);
                        row_f(ui, dirty, "İç oran", &mut s.inner, t, 0.01..=0.99, 0.005);
                    }
                    ShapeKind::Ring | ShapeKind::Cross | ShapeKind::Crescent => row_f(ui, dirty, "İç oran", &mut s.inner, t, 0.01..=0.99, 0.005),
                    _ => {}
                }
            } else {
                ui.label(RichText::new("Noktaları önizlemede sürükleyerek düzenle.").color(TEXT_DIM));
                *dirty |= ui.checkbox(&mut s.closed, "Kapalı yol").changed();
                ui.label(format!("{} nokta", s.path.len()));
            }
        }
        Content::Video { path } | Content::Image { path } | Content::Audio { path } => {
            ui.label(RichText::new(path.as_str()).color(TEXT_DIM).small());
            let is_time_based = !path.is_empty();
            let _ = is_time_based;
        }
        Content::Group { children } => {
            ui.label(format!("{} alt katman", children.len()));
        }
        Content::Camera { fov } => {
            ui.label(RichText::new("3B işaretli katmanları bu kameradan gösterir.").color(TEXT_DIM));
            row_f(ui, dirty, "Görüş açısı", fov, t, 1.0..=170.0, 0.2);
        }
        Content::Null => {
            ui.label(RichText::new("Görünmez. Başka katmanlar buna üst katman olarak bağlanabilir.").color(TEXT_DIM));
        }
    }
    let has_time = matches!(l.content, Content::Video { .. } | Content::Audio { .. });
    if has_time {
        ui.separator();
        let mut sp = l.speed;
        if ui.add(egui::Slider::new(&mut sp, 0.1..=8.0).logarithmic(true).text("Hız")).changed() {
            // Faster playback means less timeline for the same source.
            l.dur *= (start_speed / sp) as f64;
            l.speed = sp;
            *dirty = true;
        }
        row_f(ui, dirty, "Ses seviyesi", &mut l.volume, t, 0.0..=4.0, 0.01);
        *dirty |= ui.checkbox(&mut l.muted, "Sessiz").changed();
    }
}

fn transform_tab(ui: &mut egui::Ui, l: &mut Layer, t: f64, parents: &[(u64, String)], dirty: &mut bool) {
    row_v2(ui, dirty, "Konum", &mut l.tf.pos, t, 1.0);
    row_v2(ui, dirty, "Ölçek", &mut l.tf.scale, t, 0.005);
    row_f(ui, dirty, "Dönme", &mut l.tf.rot, t, -36000.0..=36000.0, 0.5);
    row_f(ui, dirty, "Opaklık", &mut l.opacity, t, 0.0..=1.0, 0.005);
    row_v2(ui, dirty, "Çapa", &mut l.tf.anchor, t, 1.0);
    row_v2(ui, dirty, "Eğim", &mut l.tf.skew, t, 0.25);
    ui.separator();
    ui.label(RichText::new("3B").strong());
    *dirty |= ui.checkbox(&mut l.three_d, "Kamerayı izle").changed();
    row_f(ui, dirty, "X dönmesi", &mut l.tf.rot_x, t, -36000.0..=36000.0, 0.5);
    row_f(ui, dirty, "Y dönmesi", &mut l.tf.rot_y, t, -36000.0..=36000.0, 0.5);
    row_f(ui, dirty, "Z konumu", &mut l.tf.z, t, -20000.0..=20000.0, 1.0);
    ui.separator();
    *dirty |= ui.checkbox(&mut l.motion_blur, "Hareket bulanıklığı").changed();
    ui.horizontal(|ui| {
        ui.label("Üst katman");
        let name = l.parent.and_then(|p| parents.iter().find(|q| q.0 == p)).map_or("Yok".to_string(), |q| q.1.clone());
        egui::ComboBox::from_id_salt("parent").selected_text(name).show_ui(ui, |ui| {
            if ui.selectable_label(l.parent.is_none(), "Yok").clicked() {
                l.parent = None;
                *dirty = true;
            }
            for (i, n) in parents {
                if ui.selectable_label(l.parent == Some(*i), n).clicked() {
                    l.parent = Some(*i);
                    *dirty = true;
                }
            }
        });
    });
}

fn color_tab(ui: &mut egui::Ui, l: &mut Layer, t: f64, dirty: &mut bool) {
    ui.horizontal(|ui| {
        ui.label("Dolgu");
        for (k, n) in [(FillKind::None, "Yok"), (FillKind::Solid, "Düz"), (FillKind::Linear, "Doğrusal"), (FillKind::Radial, "Dairesel")] {
            if ui.selectable_label(l.fill.kind == k, n).clicked() {
                l.fill.kind = k;
                *dirty = true;
            }
        }
    });
    if matches!(l.content, Content::Image { .. } | Content::Video { .. } | Content::Group { .. }) {
        ui.label(RichText::new("Bu katmanın rengi kaynağından gelir. Renk efektlerini Efektler sekmesinde kullan.").color(TEXT_DIM));
        return;
    }
    if l.fill.kind != FillKind::None {
        row_color(ui, dirty, "Renk 1", &mut l.fill.a, t);
    }
    if matches!(l.fill.kind, FillKind::Linear | FillKind::Radial) {
        row_color(ui, dirty, "Renk 2", &mut l.fill.b, t);
    }
    if l.fill.kind == FillKind::Linear {
        row_f(ui, dirty, "Açı", &mut l.fill.angle, t, -360.0..=360.0, 0.5);
    }
}

fn border_tab(ui: &mut egui::Ui, l: &mut Layer, t: f64, dirty: &mut bool) {
    let mut on = l.border.is_some();
    if ui.checkbox(&mut on, "Kenar çizgisi").changed() {
        l.border = on.then(|| Border { color: Track::new([1.0; 4]), width: Track::new(6.0) });
        *dirty = true;
    }
    if let Some(b) = &mut l.border {
        row_color(ui, dirty, "Renk", &mut b.color, t);
        row_f(ui, dirty, "Kalınlık", &mut b.width, t, 0.0..=1000.0, 0.25);
    }
    ui.separator();
    let mut on = l.shadow.is_some();
    if ui.checkbox(&mut on, "Gölge").changed() {
        l.shadow = on.then(|| Shadow { color: Track::new([0.0, 0.0, 0.0, 0.6]), angle: Track::new(45.0), dist: Track::new(20.0), blur: Track::new(15.0) });
        *dirty = true;
    }
    if let Some(s) = &mut l.shadow {
        row_color(ui, dirty, "Renk", &mut s.color, t);
        row_f(ui, dirty, "Açı", &mut s.angle, t, -360.0..=360.0, 0.5);
        row_f(ui, dirty, "Uzaklık", &mut s.dist, t, 0.0..=2000.0, 0.5);
        row_f(ui, dirty, "Bulanıklık", &mut s.blur, t, 0.0..=500.0, 0.25);
    }
}

fn blend_tab(ui: &mut egui::Ui, l: &mut Layer, t: f64, dirty: &mut bool) {
    row_f(ui, dirty, "Opaklık", &mut l.opacity, t, 0.0..=1.0, 0.005);
    ui.horizontal(|ui| {
        ui.label("Karışım modu");
        let cur = Blend::ALL.iter().find(|b| b.0 == l.blend).map_or("", |b| b.1);
        egui::ComboBox::from_id_salt("blend").selected_text(cur).height(320.0).show_ui(ui, |ui| {
            for (b, n) in Blend::ALL {
                if ui.selectable_label(l.blend == *b, *n).clicked() {
                    l.blend = *b;
                    *dirty = true;
                }
            }
        });
    });
    ui.separator();
    ui.label(RichText::new("Maske").strong());
    ui.label(RichText::new("Maske katmanı görünmez, altındaki katmanları kendi şekline göre keser.").color(TEXT_DIM));
    for (m, n) in [(MaskMode::None, "Yok"), (MaskMode::Mask, "Maske"), (MaskMode::InverseMask, "Ters maske"), (MaskMode::LumaMask, "Parlaklık maskesi")] {
        if ui.radio(l.mask == m, n).clicked() {
            l.mask = m;
            *dirty = true;
        }
    }
}

fn effects_tab(ui: &mut egui::Ui, l: &mut Layer, t: f64, clip: &mut Vec<EffectInst>, open_browser: &mut bool, dirty: &mut bool) {
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new(RichText::new("+ Efekt Ekle").color(Color32::WHITE)).fill(ACCENT)).clicked() {
            *open_browser = true;
        }
        if ui.add_enabled(!l.effects.is_empty(), egui::Button::new("Kopyala")).clicked() {
            *clip = l.effects.clone();
        }
        if ui.add_enabled(!clip.is_empty(), egui::Button::new("Yapıştır")).clicked() {
            l.effects.extend(clip.iter().cloned());
            *dirty = true;
        }
    });
    let mut remove = None;
    let mut swap = None;
    let mut dup = None;
    let count = l.effects.len();
    for (i, e) in l.effects.iter_mut().enumerate() {
        let Some(def) = effects::find(&e.def) else { continue };
        ui.push_id(i, |ui| {
            egui::Frame::new().fill(BG2).corner_radius(6.0).inner_margin(6.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    *dirty |= ui.checkbox(&mut e.on, RichText::new(def.name).strong()).changed();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").on_hover_text("Kaldır").clicked() {
                            remove = Some(i);
                        }
                        if ui.small_button("⧉").on_hover_text("Çoğalt").clicked() {
                            dup = Some(i);
                        }
                        if i + 1 < count && ui.small_button("▼").clicked() {
                            swap = Some((i, i + 1));
                        }
                        if i > 0 && ui.small_button("▲").clicked() {
                            swap = Some((i, i - 1));
                        }
                    });
                });
                for q in def.params {
                    effect_param(ui, e, q, t, dirty);
                }
            });
        });
        ui.add_space(4.0);
    }
    if let Some(i) = remove {
        l.effects.remove(i);
        *dirty = true;
    }
    if let Some(i) = dup {
        let c = l.effects[i].clone();
        l.effects.insert(i + 1, c);
        *dirty = true;
    }
    if let Some((a, b)) = swap {
        l.effects.swap(a, b);
        *dirty = true;
    }
    if l.effects.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Henüz efekt yok. 169 efektin tamamı ücretsiz.").color(TEXT_DIM));
    }
}

fn effect_param(ui: &mut egui::Ui, e: &mut EffectInst, q: &effects::Param, t: f64, dirty: &mut bool) {
    let tr = effects::track_mut(e, q);
    ui.horizontal(|ui| {
        if key_button(ui, tr.key_index(t).is_some(), !tr.keys.is_empty()) {
            tr.toggle_key(t);
            *dirty = true;
        }
        ui.add_sized([104.0, 18.0], egui::Label::new(q.label).truncate());
        let mut v = tr.at(t);
        let mut changed = false;
        match q.kind {
            Kind::F(min, max, _) => {
                let sp = ((max - min) / 400.0).clamp(0.001, 5.0);
                changed = ui.add(egui::DragValue::new(&mut v[0]).speed(sp).range(min..=max)).changed();
            }
            Kind::C(_) => changed = ui.color_edit_button_rgba_unmultiplied(&mut v).changed(),
            Kind::B(_) => {
                let mut b = v[0] > 0.5;
                if ui.checkbox(&mut b, "").changed() {
                    v[0] = b as u8 as f32;
                    changed = true;
                }
            }
            Kind::Ch(opts, _) => {
                let cur = (v[0].round() as usize).min(opts.len() - 1);
                egui::ComboBox::from_id_salt(q.key).selected_text(opts[cur]).show_ui(ui, |ui| {
                    for (i, o) in opts.iter().enumerate() {
                        if ui.selectable_label(cur == i, *o).clicked() {
                            v[0] = i as f32;
                            changed = true;
                        }
                    }
                });
            }
            Kind::P(..) => {
                changed |= ui.add(egui::DragValue::new(&mut v[0]).speed(1.0)).changed();
                changed |= ui.add(egui::DragValue::new(&mut v[1]).speed(1.0)).changed();
            }
        }
        if changed {
            tr.set(t, v);
            *dirty = true;
        }
    });
}

pub fn effect_browser(ed: &mut Editor, ctx: &egui::Context) {
    let mut open = true;
    let mut chosen = None;
    let win = egui::Window::new("Efekt Ekle").open(&mut open).default_size([520.0, 560.0]).collapsible(false).show(ctx, |ui| {
        ui.add(egui::TextEdit::singleline(&mut ed.effect_search).hint_text("Efekt ara...").desired_width(f32::INFINITY));
        let q = ed.effect_search.to_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (cat, title) in effects::Cat::ALL {
                let list: Vec<_> = effects::all().filter(|d| d.cat == *cat && (q.is_empty() || d.name.to_lowercase().contains(&q))).collect();
                if list.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.label(RichText::new(format!("{title} ({})", list.len())).strong().color(ACCENT));
                ui.horizontal_wrapped(|ui| {
                    for d in list {
                        if ui.button(d.name).clicked() {
                            chosen = Some(d.id);
                        }
                    }
                });
            }
        });
    });
    ed.effect_window_rect = win.map(|w| w.response.rect);
    if let Some(id) = chosen {
        if let Some(l) = ed.selected().and_then(|s| ed.p.find_mut(s)) {
            l.effects.push(effects::instance(id));
        }
        ed.tab = Tab::Effects;
        ed.effect_browser = false;
        ed.touch();
    }
    if !open {
        ed.effect_browser = false;
    }
}

/// Easing for the keyframe selected on the timeline: presets plus a draggable bezier.
fn curve_editor(ed: &mut Editor, ui: &mut egui::Ui, dirty: &mut bool) {
    let Some((id, name, k)) = ed.sel_keys.last().cloned() else { return };
    let many = ed.sel_keys.len();
    let Some(l) = ed.p.find_mut(id) else { return };
    let mut cur = None;
    l.visit_tracks(&mut |n, tr| {
        if n == name {
            cur = tr.ease(k);
        }
    });
    let Some(mut ease) = cur else { return };
    let before = ease;
    ui.separator();
    ui.label(RichText::new(if many > 1 { format!("Geçiş Eğrisi ({many} keyframe)") } else { "Geçiş Eğrisi".to_string() }).strong());
    ui.horizontal_wrapped(|ui| {
        for (n, e) in Ease::PRESETS {
            if ui.selectable_label(ease == *e, *n).clicked() {
                ease = *e;
            }
        }
    });
    let side = ui.available_width().min(200.0);
    let (rect, _) = ui.allocate_exact_size(vec2(side, side * 0.75), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    p.rect_filled(rect, 4.0, BG0);
    let inner = Rect::from_min_max(rect.min + vec2(12.0, 22.0), rect.max - vec2(12.0, 22.0));
    let to = |x: f32, y: f32| pos2(inner.left() + x * inner.width(), inner.bottom() - y * inner.height());
    p.line_segment([to(0.0, 0.0), to(1.0, 0.0)], Stroke::new(1.0, BG3));
    p.line_segment([to(0.0, 1.0), to(1.0, 1.0)], Stroke::new(1.0, BG3));
    let pts: Vec<_> = (0..=64).map(|i| {
        let x = i as f32 / 64.0;
        to(x, ease.apply(x))
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0, ACCENT)));
    if let Ease::Bezier(x1, y1, x2, y2) = ease {
        let (mut a, mut b) = ([x1, y1], [x2, y2]);
        for (idx, (q, origin)) in [(&mut a, [0.0, 0.0]), (&mut b, [1.0, 1.0])].into_iter().enumerate() {
            let c = to(q[0], q[1]);
            p.line_segment([to(origin[0], origin[1]), c], Stroke::new(1.0, TEXT_DIM));
            let r = ui.interact(Rect::from_center_size(c, vec2(16.0, 16.0)), ui.scope_id().with(("bez", idx)), Sense::drag());
            p.circle_filled(c, 5.0, if r.hovered() || r.dragged() { ACCENT2 } else { Color32::WHITE });
            if r.dragged() {
                let d = r.drag_delta();
                q[0] = (q[0] + d.x / inner.width()).clamp(0.0, 1.0);
                q[1] = (q[1] - d.y / inner.height()).clamp(-1.0, 2.0);
            }
        }
        ease = Ease::Bezier(a[0], a[1], b[0], b[1]);
    }
    if ease != before {
        ed.set_selected_keys_ease(ease);
        *dirty = true;
    }
}
