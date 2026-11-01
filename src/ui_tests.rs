//! Drives the real UI headlessly: clicks widgets by label and checks the resulting state.
use crate::app::App;
use crate::model::Content;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let d = std::env::var("CM_UI_OUT").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("compimotion_ui"));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Steps until the render thread has delivered a preview.
fn settle(h: &mut Harness<'_, Option<App>>) {
    for _ in 0..40 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn shot(h: &mut Harness<'_, Option<App>>, name: &str) {
    let img = h.render().expect("render");
    img.save(out_dir().join(name)).expect("save");
}

#[test]
fn an_added_font_is_listed_applied_and_actually_drawn() {
    let _iso = isolate();
    let src_dir = std::env::temp_dir().join(format!("cm_ui_font_{}", std::process::id()));
    std::fs::create_dir_all(&src_dir).unwrap();
    let src = src_dir.join("Arkadas Yazisi.ttf");
    std::fs::copy(crate::media::fonts_dir().join("impact.ttf"), &src).expect("impact.ttf");

    let mut h = harness();
    settle(&mut h);
    h.get_by_label("Örnek Proje").click();
    settle(&mut h);
    let assets = h.state().as_ref().unwrap().assets.clone();
    let title = editor_of(&h).p.layers[3].id;
    {
        let ed = h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap();
        ed.sel = vec![title];
        // The same path as dropping the file on the window.
        ed.import_media(&src, &assets);
        assert_eq!(ed.fonts.first().map(String::as_str), Some("Arkadas Yazisi.ttf"), "eklenen en üstte olmalı");
        let Content::Text(t) = &ed.p.find(title).unwrap().content else { panic!() };
        assert_eq!(t.font, "Arkadas Yazisi.ttf");
    }
    settle(&mut h);

    // It must really change the glyphs, not silently fall back to the default font.
    let raster = |font: &str| {
        let ed = editor_of(&h);
        let l = ed.p.find(title).unwrap();
        let Content::Text(t) = &l.content else { panic!() };
        let mut t = t.clone();
        t.font = font.into();
        crate::vector::raster_text(l, &t, 1.0, &crate::effects::Mods::default(), 1.0, &assets).expect("metin")
    };
    let (added, default) = (raster("Arkadas Yazisi.ttf"), raster("segoeui.ttf"));
    assert_ne!((added.frame.w, added.frame.h), (default.frame.w, default.frame.h), "harfler değişmeli");

    // A broken file gives a message and changes nothing.
    let bad = src_dir.join("bozuk.ttf");
    std::fs::write(&bad, b"font degil").unwrap();
    let ed = h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap();
    ed.import_media(&bad, &assets);
    assert!(ed.status.as_deref().unwrap_or("").contains("geçerli"));
    assert_eq!(ed.p.layers.len(), 4, "bozuk dosya video katmanı olmamalı");
    let _ = std::fs::remove_dir_all(&src_dir);
}

/// Renders the editor at common laptop screen sizes and scale factors.
#[test]
fn small_screens_show_the_whole_editor() {
    let _iso = isolate();
    // (physical width, physical height, Windows display scale)
    for (w, hh, scale) in [(1366.0f32, 768.0f32, 1.0f32), (1366.0, 768.0, 1.25), (1920.0, 1080.0, 1.5), (1280.0, 720.0, 1.5)] {
        // The usable area is smaller than the screen: title bar and taskbar take about 80 px.
        let size = egui::vec2(w / scale, (hh - 80.0) / scale);
        let mut h = Harness::builder().with_size(size).with_pixels_per_point(scale).wgpu().build_ui_state(
            |ui, state: &mut Option<App>| {
                let app = state.get_or_insert_with(|| App::with_ctx(ui.ctx()));
                app.root(ui);
            },
            None,
        );
        settle(&mut h);
        h.get_by_label("Örnek Proje").click();
        settle(&mut h);
        h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap().sel = vec![];
        h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap().preview_scale = 0.25;
        settle(&mut h);
        shot(&mut h, &format!("screen_{w}x{hh}_{}.png", (scale * 100.0) as u32));
        let canvas = canvas(&h);
        assert!(canvas.width() >= 300.0 && canvas.height() >= 140.0, "{w}x{hh} %{scale}: önizleme çok küçük: {canvas:?}");
        let _ = std::fs::remove_file(crate::app::projects_dir().join("Örnek Proje.cmproj"));
    }
}

static UI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Every UI test holds this: tests run one at a time, each with its own empty app-data
/// folder, so autosaves can never reach the user's real projects or another test.
fn isolate() -> std::sync::MutexGuard<'static, ()> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let guard = UI_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("compimotion_ui_data_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    unsafe { std::env::set_var("APPDATA", &dir) };
    guard
}

fn harness<'a>() -> Harness<'a, Option<App>> {
    Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_ui_state(
        |ui, state: &mut Option<App>| {
            let app = state.get_or_insert_with(|| App::with_ctx(ui.ctx()));
            app.root(ui);
        },
        None,
    )
}

/// Pointer moved one step; returns the (start, dur) of the layer afterwards.
fn move_to(h: &mut Harness<'_, Option<App>>, id: u64, x: f32, y: f32) -> (f64, f64) {
    h.hover_at(egui::pos2(x, y));
    h.step();
    let l = h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap().p.find(id).unwrap();
    (l.start, l.dur)
}

#[test]
fn ctrl_drag_sticks_clip_edges_to_other_clips() {
    let _iso = isolate();
    let mut h = harness();
    settle(&mut h);
    h.get_by_label("Örnek Proje").click();
    settle(&mut h);
    // Rows top to bottom: Başlık (ends at 5 s), Yıldız (ends at 3 s, the one being dragged), ...
    let star = {
        let ed = h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap();
        ed.p.layers[3].dur = 5.0;
        ed.p.layers[2].dur = 3.0;
        ed.p.layers[2].id
    };
    settle(&mut h);
    let area = h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap().timeline_rect;
    let x_at = |t: f32| area.left() + 210.0 + t * 60.0;
    let y = area.top() + 24.0 + 30.0 + 15.0;
    let ctrl = |h: &Harness<'_, Option<App>>, on: bool| h.event(egui::Event::ModifiersChanged(if on { egui::Modifiers::CTRL } else { egui::Modifiers::NONE }));

    // Grab the right edge with Ctrl held.
    let grab = egui::pos2(x_at(3.0) - 2.0, y);
    ctrl(&h, true);
    h.hover_at(grab);
    h.step();
    h.step();
    h.drag_at(grab);
    h.step();
    let (_, free) = move_to(&mut h, star, x_at(4.0), y);
    assert!((free - 4.03).abs() < 0.03, "serbest bölgede fareyi izlemeli: {free}");
    // Near the other clip's end the edge holds exactly on it, on both sides of it.
    for px in [-8.0, -3.0, 3.0, 8.0] {
        let (_, held) = move_to(&mut h, star, x_at(5.0) + px, y);
        assert_eq!(held, 5.0, "{px} px uzakta yapışmalı");
    }
    // Pulling further breaks free without a jump.
    let (_, away) = move_to(&mut h, star, x_at(5.5), y);
    assert!((away - 5.53).abs() < 0.03, "kurtulmalı: {away}");
    // Without Ctrl the same spot does not stick, even mid-drag.
    ctrl(&h, false);
    h.step();
    let (_, plain) = move_to(&mut h, star, x_at(5.0) + 8.0, y);
    assert!((plain - 5.17).abs() < 0.03, "Ctrl yokken yapışmamalı: {plain}");
    ctrl(&h, true);
    h.step();
    let (_, end) = move_to(&mut h, star, x_at(5.0) + 6.0, y);
    assert_eq!(end, 5.0);
    h.drop_at(egui::pos2(x_at(5.0) + 6.0, y));
    settle(&mut h);
    let l = h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap().p.find(star).unwrap();
    assert_eq!(l.dur, 5.0, "bırakınca yapıştığı yerde kalmalı");

    // Moving a whole clip: its end sticks to the 6 s end of the clips below.
    h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap().p.find_mut(star).unwrap().dur = 3.0;
    settle(&mut h);
    let grab = egui::pos2(x_at(1.0), y);
    h.hover_at(grab);
    h.step();
    h.step();
    h.drag_at(grab);
    h.step();
    let (start, _) = move_to(&mut h, star, x_at(4.0) + 7.0, y);
    assert_eq!(start, 3.0, "klibin sonu 6 sn'ye yapışmalı");
    h.drop_at(egui::pos2(x_at(4.0) + 7.0, y));
    ctrl(&h, false);
    settle(&mut h);
}

fn shift_click(h: &mut Harness<'_, Option<App>>, at: egui::Pos2, shift: bool) {
    h.event(egui::Event::ModifiersChanged(if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE }));
    h.hover_at(at);
    h.step();
    h.step();
    h.drag_at(at);
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.step();
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.step();
}

fn demo_editor<'a>() -> Harness<'a, Option<App>> {
    let mut h = harness();
    settle(&mut h);
    h.get_by_label("Örnek Proje").click();
    settle(&mut h);
    h
}

fn editor_of<'b>(h: &'b Harness<'_, Option<App>>) -> &'b crate::app::Editor {
    h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap()
}

#[test]
fn shift_click_selects_many_clips_and_they_move_together() {
    let _iso = isolate();
    let mut h = demo_editor();
    let area = editor_of(&h).timeline_rect;
    let (title, star, ring) = {
        let p = &editor_of(&h).p;
        (p.layers[3].id, p.layers[2].id, p.layers[1].id)
    };
    let x = area.left() + 210.0 + 120.0;
    let row = |i: f32| area.top() + 24.0 + 30.0 * i + 15.0;

    shift_click(&mut h, egui::pos2(x, row(0.0)), false);
    assert_eq!(editor_of(&h).sel, vec![title]);
    shift_click(&mut h, egui::pos2(x, row(1.0)), true);
    shift_click(&mut h, egui::pos2(x, row(2.0)), true);
    assert_eq!(editor_of(&h).sel, vec![title, star, ring]);
    // Shift+click on a selected clip takes only that one out.
    shift_click(&mut h, egui::pos2(x, row(1.0)), true);
    assert_eq!(editor_of(&h).sel, vec![title, ring]);

    // Dragging one selected clip by 1 s (60 px) moves both, and leaves the star where it is.
    let from = egui::pos2(x, row(0.0));
    h.hover_at(from);
    h.step();
    h.step();
    h.drag_at(from);
    h.step();
    for i in 1..=6 {
        h.hover_at(from + egui::vec2(10.0 * i as f32, 0.0));
        h.step();
    }
    h.drop_at(from + egui::vec2(60.0, 0.0));
    settle(&mut h);
    let ed = editor_of(&h);
    assert_eq!(ed.sel, vec![title, ring], "sürükleme seçimi bozmamalı");
    assert!((ed.p.find(title).unwrap().start - 1.0).abs() < 0.02);
    assert!((ed.p.find(ring).unwrap().start - 1.0).abs() < 0.02);
    assert_eq!(ed.p.find(star).unwrap().start, 0.0);

    // A plain click goes back to one clip.
    shift_click(&mut h, egui::pos2(x, row(1.0)), false);
    assert_eq!(editor_of(&h).sel, vec![star]);
}

#[test]
fn shift_click_selects_keyframes_and_delete_removes_only_them() {
    let _iso = isolate();
    let mut h = demo_editor();
    let star = editor_of(&h).p.layers[2].id;
    {
        let ed = h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap();
        let l = ed.p.find_mut(star).unwrap();
        l.tf.scale.keys[0].t = 1.0;
        l.tf.scale.keys[1].t = 2.0;
        l.tf.scale.set_key(3.0, [2.0, 2.0]);
        ed.expanded.insert(star);
    }
    settle(&mut h);
    let area = editor_of(&h).timeline_rect;
    // Rows: Başlık, Yıldız, then Yıldız's property row with the keys.
    let y = area.top() + 24.0 + 30.0 * 2.0 + 15.0;
    let key_x = |t: f32| area.left() + 210.0 + t * 60.0;
    shift_click(&mut h, egui::pos2(key_x(1.0), y), false);
    assert_eq!(editor_of(&h).sel_keys.len(), 1);
    shift_click(&mut h, egui::pos2(key_x(3.0), y), true);
    assert_eq!(editor_of(&h).sel_keys.len(), 2);
    assert_eq!(editor_of(&h).sel, vec![star]);
    settle(&mut h);
    shot(&mut h, "7_curve_editor.png");
    // The curve editor must be reachable, inside the panel.
    h.get_by_label("Zıplama").click();
    settle(&mut h);
    let keys = &editor_of(&h).p.find(star).unwrap().tf.scale.keys;
    assert_eq!((keys[0].ease, keys[2].ease), (crate::model::Ease::Bounce, crate::model::Ease::Bounce), "iki seçili keyframe'e de uygulanmalı");
    h.key_press(egui::Key::Delete);
    settle(&mut h);
    let ed = editor_of(&h);
    let times: Vec<f64> = ed.p.find(star).unwrap().tf.scale.keys.iter().map(|k| k.t).collect();
    assert_eq!(times, vec![2.0], "yalnızca seçili keyframe'ler silinmeli");
    assert_eq!(ed.p.layers.len(), 4, "katman silinmemeli");
}

#[test]
fn ctrl_shift_d_splits_the_middle_and_ctrl_d_still_duplicates() {
    let _iso = isolate();
    let mut h = demo_editor();
    let (a, b) = {
        let p = &editor_of(&h).p;
        (p.layers[2].id, p.layers[1].id)
    };
    h.state_mut().as_mut().and_then(|e| e.ed.as_mut()).unwrap().sel = vec![a, b];
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D);
    settle(&mut h);
    {
        let ed = editor_of(&h);
        assert_eq!(ed.p.layers.len(), 6, "ikisi de bölünmeli, çoğaltılmamalı");
        assert_eq!(ed.p.find(a).unwrap().dur, 3.0);
        assert_eq!(ed.sel.len(), 4);
    }
    h.get_by_label("↶ Geri").click();
    settle(&mut h);
    assert_eq!(editor_of(&h).p.layers.len(), 4, "tek geri alma adımı olmalı");

    h.state_mut().as_mut().and_then(|e| e.ed.as_mut()).unwrap().sel = vec![a];
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::D);
    settle(&mut h);
    let ed = editor_of(&h);
    assert_eq!(ed.p.layers.len(), 5);
    assert_eq!(ed.p.find(a).unwrap().dur, 6.0, "Ctrl+D bölmemeli");
}

#[test]
fn dragging_a_row_onto_a_group_adds_it_and_back_out_removes_it() {
    let _iso = isolate();
    let mut h = demo_editor();
    let (ring, star) = {
        let p = &editor_of(&h).p;
        (p.layers[1].id, p.layers[2].id)
    };
    // Group the title (top row); the group lands on top and stays expanded.
    {
        let ed = h.state_mut().as_mut().and_then(|e| e.ed.as_mut()).unwrap();
        ed.sel = vec![ed.p.layers[3].id];
        ed.group_selected();
        ed.expanded.clear();
        ed.sel.clear();
    }
    settle(&mut h);
    let g = editor_of(&h).p.layers.last().unwrap().id;
    let area = editor_of(&h).timeline_rect;
    let name_x = area.left() + 60.0;
    let row = |i: f32| area.top() + 24.0 + 30.0 * i + 15.0;
    // Rows: Grup, Yıldız, Halka, Arka Plan. Drag Yıldız's header onto Grup.
    let from = egui::pos2(name_x, row(1.0));
    let to = egui::pos2(name_x, row(0.0));
    h.hover_at(from);
    h.step();
    h.step();
    h.drag_at(from);
    h.step();
    for i in 1..=5 {
        h.hover_at(from + (to - from) * i as f32 / 5.0);
        h.step();
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    settle(&mut h);
    let ed = editor_of(&h);
    assert_eq!(crate::ops::parent_of(&ed.p.layers, star), Some(Some(g)), "yıldız gruba girmeli");
    assert_eq!(crate::ops::abs_start(&ed.p.layers, star, 0.0), Some(0.0));
    assert!(ed.expanded.contains(&g));
    assert_eq!(crate::ops::parent_of(&ed.p.layers, ring), Some(None));

    // The button takes it back out.
    h.state_mut().as_mut().and_then(|e| e.ed.as_mut()).unwrap().sel = vec![star];
    settle(&mut h);
    h.get_by_label("Gruptan Çıkar").click();
    settle(&mut h);
    assert_eq!(crate::ops::parent_of(&editor_of(&h).p.layers, star), Some(None));
}

#[test]
fn keyframes_copy_and_paste_with_shortcuts() {
    let _iso = isolate();
    let mut h = harness();
    settle(&mut h);
    h.get_by_label("Örnek Proje").click();
    settle(&mut h);
    // The star (index 2) has scale keys at 0 s and 1 s; the back layer (index 0) has none.
    let (star, back) = {
        let ed = h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap();
        let ids = (ed.p.layers[2].id, ed.p.layers[0].id);
        ed.sel = vec![ids.0];
        ids
    };
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::C);
    settle(&mut h);
    {
        let ed = h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap();
        assert_eq!(ed.key_clip.as_ref().map(|c| c.key_count()), Some(2));
        ed.sel = vec![back];
        ed.t = 2.0;
    }
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::V);
    settle(&mut h);
    {
        let ed = h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap();
        let l = ed.p.find(back).unwrap();
        let times: Vec<f64> = l.tf.scale.keys.iter().map(|k| k.t).collect();
        assert_eq!(times, vec![2.0, 3.0], "oynatma kafasından başlamalı");
        assert_eq!(l.tf.scale.keys[1].v, ed.p.find(star).unwrap().tf.scale.keys[1].v);
    }
    // One paste is one undo step.
    h.get_by_label("↶ Geri").click();
    settle(&mut h);
    assert!(h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap().p.find(back).unwrap().tf.scale.keys.is_empty());
}

#[test]
fn shared_xml_opens_as_an_editable_project() {
    let _iso = isolate();
    let dir =std::env::temp_dir().join(format!("compimotion_ui_xml_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("paylasim.xml");
    let sent = crate::demo::project();
    crate::xml::write(&sent, &f, true).unwrap();

    let mut h = harness();
    settle(&mut h);
    h.state_mut().as_mut().unwrap().start_import(f);
    settle(&mut h);
    let ed = h.state().as_ref().and_then(|a| a.ed.as_ref()).expect("içe aktarılan proje açılmalı");
    assert_eq!(ed.p.layers.len(), sent.layers.len());
    assert_eq!(ed.p.name, sent.name);
    assert!(ed.tex.is_some(), "önizleme çizilmeli");
    h.get_by_label("Paylaş (XML)").click();
    settle(&mut h);
    shot(&mut h, "6_share.png");

    // A broken file reports an error on the home screen instead of opening anything.
    let bad = dir.join("bozuk.xml");
    std::fs::write(&bad, "bu xml değil").unwrap();
    let mut h2 = harness();
    settle(&mut h2);
    h2.state_mut().as_mut().unwrap().start_import(bad);
    settle(&mut h2);
    assert!(h2.state().as_ref().unwrap().ed.is_none());
    h2.get_by_label("Tamam").click();
    settle(&mut h2);
    let _ = std::fs::remove_dir_all(&dir);
}

fn drag(h: &mut Harness<'_, Option<App>>, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.step();
    h.step();
    h.drag_at(from);
    h.step();
    for i in 1..=6 {
        h.hover_at(from + (to - from) * i as f32 / 6.0);
        h.step();
    }
    h.drop_at(to);
    h.step();
    h.hover_at(egui::pos2(700.0, 300.0));
    settle(h);
}

fn canvas(h: &Harness<'_, Option<App>>) -> egui::Rect {
    h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap().preview_rect
}

#[test]
fn resized_panels_and_windows_keep_their_size() {
    let _iso = isolate();
    let appdata = std::env::temp_dir().join(format!("compimotion_ui_resize_{}", std::process::id()));
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_ui_state(
        |ui, state: &mut Option<App>| {
            let app = state.get_or_insert_with(|| App::with_ctx(ui.ctx()));
            app.root(ui);
        },
        None,
    );
    settle(&mut h);
    h.get_by_label("Örnek Proje").click();
    settle(&mut h);

    // Right panel, both widening and narrowing, with a layer selected so its tabs are showing.
    h.get_by_label("Üçgen").click();
    settle(&mut h);
    let r0 = canvas(&h);
    drag(&mut h, egui::pos2(r0.right(), 300.0), egui::pos2(r0.right() - 150.0, 300.0));
    let r1 = canvas(&h);
    assert!((r1.right() - (r0.right() - 150.0)).abs() < 6.0, "sağ panel genişlemedi: {} -> {}", r0.right(), r1.right());
    drag(&mut h, egui::pos2(r1.right(), 300.0), egui::pos2(r1.right() + 80.0, 300.0));
    let r2 = canvas(&h);
    assert!((r2.right() - (r1.right() + 80.0)).abs() < 6.0, "sağ panel daralmadı: {} -> {}", r1.right(), r2.right());

    // Timeline height.
    let t0 = canvas(&h);
    drag(&mut h, egui::pos2(600.0, t0.bottom()), egui::pos2(600.0, t0.bottom() - 120.0));
    let t1 = canvas(&h);
    assert!((t1.bottom() - (t0.bottom() - 120.0)).abs() < 6.0, "zaman çizelgesi büyümedi: {} -> {}", t0.bottom(), t1.bottom());
    drag(&mut h, egui::pos2(600.0, t1.bottom()), egui::pos2(600.0, t1.bottom() + 60.0));
    let t2 = canvas(&h);
    assert!((t2.bottom() - (t1.bottom() + 60.0)).abs() < 6.0, "zaman çizelgesi küçülmedi: {} -> {}", t1.bottom(), t2.bottom());

    // The effect window, both growing and shrinking from its corner.
    h.get_by_label("Efektler").click();
    settle(&mut h);
    h.get_by_label("+ Efekt Ekle").click();
    settle(&mut h);
    let win = |h: &Harness<'_, Option<App>>| h.state().as_ref().and_then(|a| a.ed.as_ref()).and_then(|e| e.effect_window_rect).expect("pencere");
    let w0 = win(&h);
    drag(&mut h, w0.max - egui::vec2(3.0, 3.0), w0.max + egui::vec2(160.0, 90.0));
    let w1 = win(&h);
    assert!(w1.width() > w0.width() + 120.0 && w1.height() > w0.height() + 60.0, "pencere büyümedi: {w0:?} -> {w1:?}");
    drag(&mut h, w1.max - egui::vec2(3.0, 3.0), w1.max - egui::vec2(220.0, 200.0));
    let w2 = win(&h);
    assert!(w2.width() < w1.width() - 150.0 && w2.height() < w1.height() - 150.0, "pencere küçülmedi: {w1:?} -> {w2:?}");

    let _ = std::fs::remove_dir_all(&appdata);
}

#[test]
fn editor_flow_works_through_the_real_ui() {
    let _iso = isolate();
    let appdata = std::env::temp_dir().join(format!("compimotion_ui_appdata_{}", std::process::id()));

    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_ui_state(
        |ui, state: &mut Option<App>| {
            let app = state.get_or_insert_with(|| App::with_ctx(ui.ctx()));
            app.root(ui);
        },
        None,
    );
    settle(&mut h);
    shot(&mut h, "1_home.png");
    assert!(h.state().as_ref().is_some_and(|a| a.ed.is_none()));

    h.get_by_label("Örnek Proje").click();
    settle(&mut h);
    {
        let ed = h.state().as_ref().and_then(|a| a.ed.as_ref()).expect("düzenleyici açılmalı");
        assert_eq!(ed.p.layers.len(), 4);
        assert!(ed.tex.is_some(), "önizleme gelmeli");
    }
    shot(&mut h, "2_editor.png");

    // Adding a shape from the left panel creates and selects a layer.
    h.get_by_label("Üçgen").click();
    settle(&mut h);
    {
        let ed = h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap();
        assert_eq!(ed.p.layers.len(), 5);
        assert_eq!(ed.selected(), ed.p.layers.last().map(|l| l.id));
    }

    // Effect browser: open, pick one, and it lands on the selected layer.
    h.get_by_label("Efektler").click();
    settle(&mut h);
    h.get_by_label("+ Efekt Ekle").click();
    settle(&mut h);
    shot(&mut h, "3_effects.png");
    // The list is long, so find it the way a user would: through the search box.
    h.state_mut().as_mut().and_then(|a| a.ed.as_mut()).unwrap().effect_search = "Kaleid".into();
    settle(&mut h);
    h.get_by_label("Kaleidoscope").click();
    settle(&mut h);
    {
        let ed = h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap();
        let l = ed.p.layers.last().unwrap();
        assert_eq!(l.effects.len(), 1);
        assert_eq!(l.effects[0].def, "kaleidoscope");
    }
    shot(&mut h, "4_effect_added.png");

    // Undo removes the effect, redo brings it back.
    h.get_by_label("↶ Geri").click();
    settle(&mut h);
    assert!(h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap().p.layers.last().unwrap().effects.is_empty());
    h.get_by_label("↷ İleri").click();
    settle(&mut h);
    assert_eq!(h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap().p.layers.last().unwrap().effects.len(), 1);

    // Playback moves the clock.
    let before = h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap().t;
    h.get_by_label("▶ Oynat").click();
    settle(&mut h);
    let ed = h.state().as_ref().and_then(|a| a.ed.as_ref()).unwrap();
    assert!(ed.playing && ed.t > before, "zaman ilerlemeli");
    h.get_by_label("⏸ Duraklat").click();
    settle(&mut h);

    // Export window opens with its options.
    h.get_by_label("Dışa Aktar").click();
    settle(&mut h);
    shot(&mut h, "5_export.png");

    let _ = std::fs::remove_dir_all(&appdata);
}
