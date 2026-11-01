//! Application shell: home screen, editor state, background preview rendering, file actions.
use crate::audio::Player;
use crate::export::{self, Format, Progress, Settings};
use crate::media::{self, Assets};
use crate::model::*;
use crate::render;
use eframe::egui;
use egui::{Color32, RichText};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

pub const ACCENT: Color32 = Color32::from_rgb(124, 108, 255);
pub const ACCENT2: Color32 = Color32::from_rgb(255, 92, 138);
pub const BG0: Color32 = Color32::from_rgb(16, 17, 22);
pub const BG1: Color32 = Color32::from_rgb(24, 26, 33);
pub const BG2: Color32 = Color32::from_rgb(34, 37, 47);
pub const BG3: Color32 = Color32::from_rgb(48, 52, 65);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 155, 170);
pub const TAGS: [Color32; 6] = [
    Color32::from_rgb(124, 108, 255),
    Color32::from_rgb(255, 92, 138),
    Color32::from_rgb(60, 190, 170),
    Color32::from_rgb(240, 170, 60),
    Color32::from_rgb(80, 160, 255),
    Color32::from_rgb(170, 200, 80),
];

#[derive(Clone, Copy, PartialEq)]
pub enum Tool {
    Select,
    Pen,
    Freehand,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Content,
    Transform,
    Color,
    Border,
    Effects,
    Blend,
}

// ---- Background preview renderer ----

struct Job {
    p: Arc<Project>,
    t: f64,
    scale: f32,
}
pub struct Rendered {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}
#[derive(Default)]
struct Slots {
    job: Mutex<Option<Job>>,
    wake: Condvar,
    out: Mutex<Option<Rendered>>,
}
pub struct Renderer {
    slots: Arc<Slots>,
}
impl Renderer {
    fn new(assets: Arc<Assets>, ctx: egui::Context) -> Self {
        let slots = Arc::new(Slots::default());
        let s = slots.clone();
        std::thread::spawn(move || {
            loop {
                let job = {
                    let Ok(mut g) = s.job.lock() else { return };
                    while g.is_none() {
                        let Ok(n) = s.wake.wait(g) else { return };
                        g = n;
                    }
                    g.take()
                };
                let Some(j) = job else { continue };
                // A bad media file must not take the whole editor down with it.
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render::render_project(&j.p, j.t, j.scale, &assets)));
                if let (Ok(f), Ok(mut o)) = (done, s.out.lock()) {
                    *o = Some(Rendered { w: f.w, h: f.h, rgba: f.to_rgba8() });
                    ctx.request_repaint();
                }
            }
        });
        Renderer { slots }
    }
    fn request(&self, p: Arc<Project>, t: f64, scale: f32) {
        if let Ok(mut g) = self.slots.job.lock() {
            *g = Some(Job { p, t, scale });
            self.slots.wake.notify_one();
        }
    }
    fn take(&self) -> Option<Rendered> {
        self.slots.out.lock().ok()?.take()
    }
}

// ---- Export job ----

pub struct ExportJob {
    pub prog: Arc<Progress>,
    pub result: Arc<Mutex<Option<Result<(), String>>>>,
    pub out: PathBuf,
}

pub struct ExportUi {
    pub open: bool,
    pub settings: Settings,
    pub job: Option<ExportJob>,
    pub message: Option<String>,
}

// ---- Editor ----

pub struct Editor {
    pub p: Project,
    committed: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
    pub dirty: bool,
    pub path: PathBuf,
    pub sel: Vec<u64>,
    pub t: f64,
    pub playing: bool,
    last_tick: Instant,
    pub tool: Tool,
    pub tab: Tab,
    pub preview_scale: f32,
    pub tex: Option<egui::TextureHandle>,
    snapshot: Arc<Project>,
    snapshot_stale: bool,
    last_req: (f64, f32),
    pub force_render: bool,
    pub tl_zoom: f32,
    pub tl_scroll: f32,
    pub tl_vscroll: f32,
    pub expanded: HashSet<u64>,
    /// Selected keyframes as (layer, track, index). The last one is the primary,
    /// whose curve the easing editor shows.
    pub sel_keys: Vec<(u64, String, usize)>,
    pub effect_browser: bool,
    pub effect_search: String,
    pub effect_clip: Vec<EffectInst>,
    pub export: ExportUi,
    pub pen: Vec<PathPt>,
    pub stroke: Vec<[f32; 2]>,
    pub fonts: Vec<String>,
    pub drag_row: Option<u64>,
    pub show_grid: bool,
    pub media_dims: std::collections::HashMap<String, (f32, f32)>,
    pub status: Option<String>,
    /// Canvas area from the last frame; its edges are the panels' inner edges.
    pub preview_rect: egui::Rect,
    pub effect_window_rect: Option<egui::Rect>,
    pub key_clip: Option<crate::keys::KeyClip>,
    /// Where the clip edge being dragged would be if nothing were snapping it.
    pub clip_drag_raw: f64,
    pub timeline_rect: egui::Rect,
    status_seen: (Option<String>, f64),
    pub pick_font: bool,
    pub share_open: bool,
    pub share_embed: bool,
    pub share_job: Option<JobSlot<crate::xml::Report>>,
    pub share_message: Option<String>,
}

/// Where a background thread leaves its result for the UI to pick up.
pub type JobSlot<T> = Arc<Mutex<Option<Result<T, String>>>>;

pub fn media_dir() -> PathBuf {
    projects_dir().parent().map(|p| p.join("media")).unwrap_or_else(|| PathBuf::from("media"))
}

fn missing_text(missing: &[String]) -> String {
    format!("{} medya dosyası bulunamadı: {}", missing.len(), missing.join(", "))
}

pub struct App {
    pub assets: Arc<Assets>,
    renderer: Renderer,
    player: Player,
    pub ed: Option<Editor>,
    home: Home,
    /// Copied keyframes survive switching between projects.
    key_clip: Option<crate::keys::KeyClip>,
}

struct Home {
    entries: Vec<(PathBuf, String, std::time::SystemTime)>,
    new_open: bool,
    name: String,
    w: u32,
    h: u32,
    fps: f32,
    bg: [f32; 4],
    confirm_delete: Option<PathBuf>,
    error: Option<String>,
    import_job: Option<JobSlot<(Project, crate::xml::Report)>>,
}

pub fn projects_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    let d = Path::new(&base).join("CompiMotion").join("projects");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn scan_projects() -> Vec<(PathBuf, String, std::time::SystemTime)> {
    let mut v: Vec<_> = std::fs::read_dir(projects_dir())
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "cmproj"))
                .map(|e| {
                    let m = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                    (e.path(), e.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), m)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.2.cmp(&a.2));
    v
}

fn safe_filename(name: &str) -> String {
    let s: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "Proje".into() } else { s }
}

fn unique_path(name: &str) -> PathBuf {
    let dir = projects_dir();
    let base = safe_filename(name);
    let mut p = dir.join(format!("{base}.cmproj"));
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{base} {i}.cmproj"));
        i += 1;
    }
    p
}

fn collect_media(layers: &[Layer], out: &mut Vec<String>) {
    for l in layers {
        match &l.content {
            Content::Audio { path } | Content::Video { path } => out.push(path.clone()),
            Content::Group { children } => collect_media(children, out),
            _ => {}
        }
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        Self::with_ctx(&cc.egui_ctx)
    }

    pub fn with_ctx(ctx: &egui::Context) -> Self {
        setup_style(ctx);
        let assets = Arc::new(Assets::default());
        App {
            renderer: Renderer::new(assets.clone(), ctx.clone()),
            player: Player::new(assets.clone()),
            assets,
            ed: None,
            home: Home {
                entries: scan_projects(),
                new_open: false,
                name: "Yeni Proje".into(),
                w: 1920,
                h: 1080,
                fps: 30.0,
                bg: [0.0, 0.0, 0.0, 1.0],
                confirm_delete: None,
                error: None,
                import_job: None,
            },
            key_clip: None,
        }
    }

    /// Reads a shared XML project in the background; embedded media can be large.
    pub fn start_import(&mut self, path: PathBuf) {
        let slot: JobSlot<(Project, crate::xml::Report)> = Arc::new(Mutex::new(None));
        let out = slot.clone();
        // A larger stack, because the parsed tree is walked recursively.
        let spawned = std::thread::Builder::new().stack_size(32 << 20).spawn(move || {
            let r = std::panic::catch_unwind(|| crate::xml::read(&path, &media_dir())).unwrap_or_else(|_| Err("Dosya içe aktarılamadı.".into()));
            if let Ok(mut g) = out.lock() {
                *g = Some(r);
            }
        });
        match spawned {
            Ok(_) => self.home.import_job = Some(slot),
            Err(e) => self.home.error = Some(e.to_string()),
        }
    }

    fn poll_import(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.home.import_job else { return };
        let done = job.lock().ok().and_then(|mut g| g.take());
        let Some(result) = done else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        };
        self.home.import_job = None;
        match result {
            Ok((p, report)) => {
                let path = unique_path(&p.name);
                self.open_editor(p, path);
                if let Some(ed) = &mut self.ed {
                    ed.save();
                    if !report.missing.is_empty() {
                        ed.status = Some(missing_text(&report.missing));
                    }
                }
            }
            Err(e) => self.home.error = Some(e),
        }
    }

    fn open_editor(&mut self, p: Project, path: PathBuf) {
        let mut paths = vec![];
        collect_media(&p.layers, &mut paths);
        for m in paths {
            preload_audio(self.assets.clone(), m);
        }
        // A font that was missing earlier may have arrived with this project.
        self.assets.clear_missing_fonts();
        let mut ed = Editor::new(p, path);
        ed.key_clip = self.key_clip.take();
        self.ed = Some(ed);
    }

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG0).inner_margin(32.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("CompiMotion").size(32.0).strong().color(ACCENT));
                ui.add_space(12.0);
                ui.label(RichText::new(format!("Sürüm {} · Tüm efektler ücretsiz. Reklam yok, filigran yok.", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM));
            });
            ui.add_space(20.0);
            let mut open = None;
            let mut import = None;
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("+  Yeni Proje").size(16.0).color(Color32::WHITE)).fill(ACCENT).min_size(egui::vec2(160.0, 40.0))).clicked() {
                    self.home.new_open = true;
                }
                if ui.add(egui::Button::new("Dosyadan Aç").min_size(egui::vec2(120.0, 40.0))).clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("CompiMotion Projesi", &["cmproj"]).pick_file() {
                        open = Some(f);
                    }
                }
                let importing = self.home.import_job.is_some();
                if ui.add_enabled(!importing, egui::Button::new("XML İçe Aktar").min_size(egui::vec2(120.0, 40.0))).on_hover_text("Başkasının paylaştığı projeyi aç").clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("CompiMotion XML", &["xml"]).pick_file() {
                        import = Some(f);
                    }
                }
                if importing {
                    ui.spinner();
                    ui.label(RichText::new("İçe aktarılıyor...").color(TEXT_DIM));
                }
                if ui.add(egui::Button::new("Örnek Proje").min_size(egui::vec2(120.0, 40.0))).clicked() {
                    let p = crate::demo::project();
                    let path = unique_path(&p.name);
                    self.open_editor(p, path);
                }
            });
            ui.add_space(24.0);
            ui.label(RichText::new("Projeler").size(18.0).strong());
            ui.add_space(8.0);
            if self.home.entries.is_empty() {
                ui.label(RichText::new("Henüz proje yok. Yeni bir proje oluştur ya da örnek projeyi aç.").color(TEXT_DIM));
            }
            let mut dup = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (path, name, _) in &self.home.entries {
                    egui::Frame::new().fill(BG1).corner_radius(8.0).inner_margin(10.0).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(RichText::new(name).size(15.0)).frame(false)).clicked() {
                                open = Some(path.clone());
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Sil").clicked() {
                                    self.home.confirm_delete = Some(path.clone());
                                }
                                if ui.button("Çoğalt").clicked() {
                                    dup = Some((path.clone(), name.clone()));
                                }
                                if ui.button("Aç").clicked() {
                                    open = Some(path.clone());
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                }
            });
            if let Some((src, name)) = dup {
                let to = unique_path(&format!("{name} kopya"));
                if std::fs::copy(&src, &to).is_err() {
                    self.home.error = Some("Proje çoğaltılamadı.".into());
                }
                self.home.entries = scan_projects();
            }
            if let Some(f) = import {
                self.start_import(f);
            }
            if let Some(f) = open {
                match load_project(&f) {
                    Ok(p) => self.open_editor(p, f),
                    Err(e) => self.home.error = Some(e),
                }
            }
        });

        let ctx = ui.ctx().clone();
        self.poll_import(&ctx);
        if self.home.new_open {
            let mut create = false;
            let mut open = true;
            egui::Window::new("Yeni Proje").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                let h = &mut self.home;
                ui.horizontal(|ui| {
                    ui.label("Ad");
                    ui.text_edit_singleline(&mut h.name);
                });
                ui.add_space(6.0);
                ui.label("Hazır boyutlar");
                ui.horizontal_wrapped(|ui| {
                    for (n, w, hh) in [("16:9 1080p", 1920, 1080), ("9:16 Dikey", 1080, 1920), ("1:1 Kare", 1080, 1080), ("4:5", 1080, 1350), ("4:3", 1440, 1080), ("720p", 1280, 720), ("4K", 3840, 2160)] {
                        if ui.selectable_label(h.w == w && h.h == hh, n).clicked() {
                            h.w = w;
                            h.h = hh;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Genişlik");
                    ui.add(egui::DragValue::new(&mut h.w).range(16..=7680));
                    ui.label("Yükseklik");
                    ui.add(egui::DragValue::new(&mut h.h).range(16..=7680));
                });
                ui.horizontal(|ui| {
                    ui.label("Kare hızı");
                    for f in [24.0, 25.0, 30.0, 50.0, 60.0] {
                        if ui.selectable_label(h.fps == f, format!("{f}")).clicked() {
                            h.fps = f;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Arka plan");
                    ui.color_edit_button_rgba_unmultiplied(&mut h.bg);
                });
                ui.add_space(8.0);
                if ui.add(egui::Button::new(RichText::new("Oluştur").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    create = true;
                }
            });
            if !open {
                self.home.new_open = false;
            }
            if create {
                let h = &self.home;
                let p = Project::new(&h.name, h.w, h.h, h.fps, h.bg);
                let path = unique_path(&h.name);
                self.home.new_open = false;
                self.open_editor(p, path);
            }
        }
        if let Some(path) = self.home.confirm_delete.clone() {
            egui::Window::new("Proje silinsin mi?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                ui.label(path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                ui.label(RichText::new("Bu işlem geri alınamaz.").color(TEXT_DIM));
                ui.horizontal(|ui| {
                    if ui.button("Vazgeç").clicked() {
                        self.home.confirm_delete = None;
                    }
                    if ui.add(egui::Button::new(RichText::new("Sil").color(Color32::WHITE)).fill(ACCENT2)).clicked() {
                        let _ = std::fs::remove_file(&path);
                        self.home.confirm_delete = None;
                        self.home.entries = scan_projects();
                    }
                });
            });
        }
        if let Some(msg) = self.home.error.clone() {
            egui::Window::new("Hata").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                ui.label(msg);
                if ui.button("Tamam").clicked() {
                    self.home.error = None;
                }
            });
        }
    }
}

pub fn preload_audio(assets: Arc<Assets>, path: String) {
    std::thread::spawn(move || {
        let _ = assets.audio(&path);
    });
}

pub fn load_project(path: &Path) -> Result<Project, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("Dosya okunamadı: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("Proje dosyası bozuk: {e}"))
}

impl Editor {
    pub fn new(p: Project, path: PathBuf) -> Self {
        Editor {
            committed: p.clone(),
            snapshot: Arc::new(p.clone()),
            p,
            undo: vec![],
            redo: vec![],
            dirty: false,
            path,
            sel: vec![],
            t: 0.0,
            playing: false,
            last_tick: Instant::now(),
            tool: Tool::Select,
            tab: Tab::Transform,
            preview_scale: 0.5,
            tex: None,
            snapshot_stale: false,
            last_req: (-1.0, 0.0),
            force_render: true,
            tl_zoom: 60.0,
            tl_scroll: 0.0,
            tl_vscroll: 0.0,
            expanded: HashSet::new(),
            sel_keys: vec![],
            effect_browser: false,
            effect_search: String::new(),
            effect_clip: vec![],
            export: ExportUi {
                open: false,
                settings: Settings { format: Format::Mp4, scale: 1.0, fps: 30.0, bitrate: 12.0, audio: true, transparent: false },
                job: None,
                message: None,
            },
            pen: vec![],
            stroke: vec![],
            fonts: media::list_fonts(),
            drag_row: None,
            show_grid: false,
            media_dims: Default::default(),
            status: None,
            preview_rect: egui::Rect::NOTHING,
            effect_window_rect: None,
            key_clip: None,
            clip_drag_raw: 0.0,
            timeline_rect: egui::Rect::NOTHING,
            status_seen: (None, 0.0),
            pick_font: false,
            share_open: false,
            share_embed: true,
            share_job: None,
            share_message: None,
        }
    }

    pub fn touch(&mut self) {
        self.dirty = true;
        self.snapshot_stale = true;
    }

    pub fn selected(&self) -> Option<u64> {
        self.sel.last().copied()
    }

    pub fn save(&mut self) {
        match serde_json::to_string(&self.p).map_err(|e| e.to_string()).and_then(|s| std::fs::write(&self.path, s).map_err(|e| e.to_string())) {
            Ok(()) => {}
            Err(e) => self.status = Some(format!("Kaydedilemedi: {e}")),
        }
    }

    /// Commits a finished edit: one mouse gesture becomes one undo step, and is autosaved.
    fn commit_if_done(&mut self, pointer_down: bool) {
        if self.dirty && !pointer_down {
            self.dirty = false;
            self.p.fit_duration();
            self.undo.push(std::mem::replace(&mut self.committed, self.p.clone()));
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.save();
            self.snapshot_stale = true;
        }
    }

    pub fn do_undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.sel_keys.clear();
            self.redo.push(std::mem::replace(&mut self.committed, prev.clone()));
            self.p = prev;
            self.snapshot_stale = true;
            self.save();
        }
    }
    pub fn do_redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.sel_keys.clear();
            self.undo.push(std::mem::replace(&mut self.committed, next.clone()));
            self.p = next;
            self.snapshot_stale = true;
            self.save();
        }
    }

    pub fn add_layer(&mut self, name: &str, content: Content) -> u64 {
        let id = self.p.alloc_id();
        let center = [self.p.w as f32 / 2.0, self.p.h as f32 / 2.0];
        let dur = if self.p.layers.is_empty() { 5.0 } else { self.p.dur.max(1.0) };
        let mut l = Layer::new(id, name, content, center, dur);
        l.start = 0.0;
        self.p.layers.push(l);
        self.sel = vec![id];
        self.tab = Tab::Content;
        self.touch();
        id
    }

    pub fn add_shape(&mut self, kind: ShapeKind, name: &str) {
        let k = self.p.h.min(self.p.w) as f32 * 0.4;
        let size = if kind == ShapeKind::Line { [k * 1.5, 0.0] } else { [k, k] };
        let sh = ShapeData { kind, size: Track::new(size), radius: Track::new(k * 0.12), sides: Track::new(if kind == ShapeKind::Star { 5.0 } else { 6.0 }), inner: Track::new(0.5), path: vec![], closed: true, freehand: false };
        let id = self.add_layer(name, Content::Shape(sh));
        if kind == ShapeKind::Line {
            if let Some(l) = self.p.find_mut(id) {
                l.border = Some(Border { color: Track::new([1.0; 4]), width: Track::new(8.0) });
            }
        }
    }

    pub fn add_text(&mut self) {
        let size = self.p.h as f32 * 0.12;
        let tx = TextData { text: "Metin".into(), font: "segoeui.ttf".into(), size: Track::new(size), align: Align::Center, tracking: Track::new(0.0), leading: Track::new(0.0) };
        self.add_layer("Metin", Content::Text(tx));
    }

    /// Installs a font file for all projects and applies it to the selected text layer.
    pub fn add_font(&mut self, path: &Path, assets: &Arc<Assets>) {
        let name = match media::import_font_file(path) {
            Ok(n) => n,
            Err(e) => {
                self.status = Some(e);
                return;
            }
        };
        assets.clear_missing_fonts();
        self.fonts = media::list_fonts();
        let target = self.selected().and_then(|id| self.p.find_mut(id));
        if let Some(Content::Text(t)) = target.map(|l| &mut l.content) {
            t.font = name.clone();
            self.touch();
        }
        self.status = Some(format!("Yazı tipi eklendi: {name}"));
    }

    pub fn import_media(&mut self, path: &Path, assets: &Arc<Assets>) {
        let p = path.to_string_lossy().to_string();
        let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Medya".into());
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == "ttf" || ext == "otf" {
            self.add_font(path, assets);
            return;
        }
        let (cw, ch) = (self.p.w as f32, self.p.h as f32);
        let fit = |w: f32, h: f32| (cw / w.max(1.0)).min(ch / h.max(1.0)).min(1.0);
        if ["png", "jpg", "jpeg", "webp", "bmp", "gif"].contains(&ext.as_str()) {
            let Some(f) = assets.image(&p) else {
                self.status = Some(format!("Görüntü açılamadı: {name}"));
                return;
            };
            let id = self.add_layer(&name, Content::Image { path: p });
            let s = fit(f.w as f32, f.h as f32);
            if let Some(l) = self.p.find_mut(id) {
                l.tf.scale = Track::new([s, s]);
            }
        } else if ["mp3", "wav", "aac", "m4a", "wma", "flac"].contains(&ext.as_str()) {
            let Some(d) = media::probe_duration(&p) else {
                self.status = Some(format!("Ses açılamadı: {name}"));
                return;
            };
            let id = self.add_layer(&name, Content::Audio { path: p.clone() });
            if let Some(l) = self.p.find_mut(id) {
                l.dur = d;
            }
            preload_audio(assets.clone(), p);
        } else {
            let Some((w, h, d, fps)) = assets.video_info(&p) else {
                self.status = Some(format!("Video açılamadı: {name}"));
                return;
            };
            let id = self.add_layer(&name, Content::Video { path: p.clone() });
            let s = fit(w as f32, h as f32);
            if let Some(l) = self.p.find_mut(id) {
                l.dur = d.max(0.1);
                l.tf.scale = Track::new([s, s]);
            }
            let _ = fps;
            preload_audio(assets.clone(), p);
        }
    }

    /// Removes selected layers from any stack and returns them in stack order.
    fn take_selected(&mut self) -> Vec<Layer> {
        fn go(v: &mut Vec<Layer>, sel: &[u64], out: &mut Vec<Layer>) {
            let mut i = 0;
            while i < v.len() {
                if sel.contains(&v[i].id) {
                    out.push(v.remove(i));
                } else {
                    if let Content::Group { children } = &mut v[i].content {
                        go(children, sel, out);
                    }
                    i += 1;
                }
            }
        }
        let mut out = vec![];
        let sel = self.sel.clone();
        go(&mut self.p.layers, &sel, &mut out);
        out
    }

    pub fn delete_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        let gone: Vec<u64> = self.sel.clone();
        self.take_selected();
        clear_parents(&mut self.p.layers, &gone);
        self.sel.clear();
        self.sel_keys.clear();
        self.touch();
    }

    pub fn duplicate_selected(&mut self) {
        let Some(id) = self.selected() else { return };
        let Some(src) = self.p.find(id).cloned() else { return };
        let mut c = src;
        reassign_ids(&mut c, &mut self.p);
        c.name = format!("{} kopya", c.name);
        let new_id = c.id;
        insert_above(&mut self.p.layers, id, c);
        self.sel = vec![new_id];
        self.touch();
    }

    /// Cuts the selected layers at the playhead; the right parts become the selection.
    pub fn split_selected(&mut self) {
        let t = self.t;
        let made: Vec<u64> = self.top_selection().into_iter().filter_map(|id| self.split_at(id, t)).collect();
        if made.is_empty() {
            self.status = Some("Bölmek için oynatma kafası katmanın içinde olmalı.".into());
            return;
        }
        self.sel = made;
        self.touch();
    }

    pub fn group_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        // An empty group appears first; moving layers in makes it fit them.
        let ids = self.top_selection();
        let times: Vec<(f64, f64)> = ids.iter().filter_map(|i| Some((crate::ops::abs_start(&self.p.layers, *i, 0.0)?, self.p.find(*i)?.dur))).collect();
        let start = times.iter().map(|q| q.0).fold(f64::MAX, f64::min);
        let end = times.iter().map(|q| q.0 + q.1).fold(0.0, f64::max);
        if times.is_empty() {
            return;
        }
        let id = self.p.alloc_id();
        let center = [self.p.w as f32 / 2.0, self.p.h as f32 / 2.0];
        let mut g = Layer::new(id, "Grup", Content::Group { children: vec![] }, center, end - start);
        g.start = start;
        self.p.layers.push(g);
        if self.move_layers(&ids, crate::ops::Dest::Group(id)) == 0 {
            self.p.layers.retain(|l| l.id != id);
            self.status = Some("Bu seçim gruplanamaz.".into());
            return;
        }
        self.sel = vec![id];
        self.expanded.insert(id);
        self.touch();
    }

    pub fn ungroup_selected(&mut self) {
        let Some(id) = self.selected() else { return };
        let Some(i) = self.p.layers.iter().position(|l| l.id == id) else { return };
        if !matches!(self.p.layers[i].content, Content::Group { .. }) {
            return;
        }
        let g = self.p.layers.remove(i);
        let (start, Content::Group { mut children }) = (g.start, g.content) else { return };
        for c in &mut children {
            c.start += start;
        }
        let ids: Vec<u64> = children.iter().map(|c| c.id).collect();
        for (k, c) in children.into_iter().enumerate() {
            self.p.layers.insert(i + k, c);
        }
        self.sel = ids;
        self.touch();
    }

    pub fn move_selected(&mut self, delta: i32) {
        let Some(id) = self.selected() else { return };
        fn go(v: &mut [Layer], id: u64, delta: i32) -> bool {
            if let Some(i) = v.iter().position(|l| l.id == id) {
                let j = (i as i32 + delta).clamp(0, v.len() as i32 - 1) as usize;
                v.swap(i, j);
                return true;
            }
            v.iter_mut().any(|l| if let Content::Group { children } = &mut l.content { go(children, id, delta) } else { false })
        }
        if go(&mut self.p.layers, id, delta) {
            self.touch();
        }
    }

    pub fn set_time(&mut self, t: f64) {
        self.t = t.clamp(0.0, self.p.dur.max(0.0));
    }

    pub fn step_frame(&mut self, n: i32) {
        let f = (self.t * self.p.fps as f64).round() + n as f64;
        self.set_time(f / self.p.fps as f64);
    }

    pub fn snap_time(&self, t: f64) -> f64 {
        let f = self.p.fps as f64;
        (t * f).round() / f
    }

    fn start_export(&mut self, out: PathBuf, assets: Arc<Assets>) {
        let prog = Arc::new(Progress::default());
        let result = Arc::new(Mutex::new(None));
        let (p, s, t) = (self.p.clone(), self.export.settings.clone(), self.t);
        let (pr, rs, path) = (prog.clone(), result.clone(), out.clone());
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| export::run(&p, &path, &s, t, &assets, &pr)));
            let r = r.unwrap_or_else(|_| Err("Dışa aktarma sırasında beklenmedik bir hata oluştu.".into()));
            if let Ok(mut g) = rs.lock() {
                *g = Some(r);
            }
        });
        self.export.job = Some(ExportJob { prog, result, out });
    }

    fn export_window(&mut self, ctx: &egui::Context, assets: &Arc<Assets>) {
        if !self.export.open {
            return;
        }
        let mut open = true;
        let mut begin = false;
        egui::Window::new("Dışa Aktar").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            if let Some(job) = &self.export.job {
                let done = job.prog.done.load(Ordering::Relaxed) as f32 / 1000.0;
                ui.add(egui::ProgressBar::new(done).show_percentage().desired_width(320.0));
                if ui.button("İptal").clicked() {
                    job.prog.cancel.store(true, Ordering::Relaxed);
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            let (pw, ph) = (self.p.w, self.p.h);
            let s = &mut self.export.settings;
            ui.horizontal(|ui| {
                ui.label("Biçim");
                ui.selectable_value(&mut s.format, Format::Mp4, "MP4 Video");
                ui.selectable_value(&mut s.format, Format::Gif, "GIF");
                ui.selectable_value(&mut s.format, Format::PngSeq, "PNG Dizisi");
                ui.selectable_value(&mut s.format, Format::PngFrame, "Bu Kare (PNG)");
            });
            ui.horizontal(|ui| {
                ui.label("Çözünürlük");
                for (n, k) in [("%25", 0.25), ("%50", 0.5), ("%100", 1.0), ("%200", 2.0)] {
                    ui.selectable_value(&mut s.scale, k, n);
                }
                ui.label(RichText::new(format!("{} × {}", (pw as f32 * s.scale) as u32, (ph as f32 * s.scale) as u32)).color(TEXT_DIM));
            });
            if s.format != Format::PngFrame {
                ui.horizontal(|ui| {
                    ui.label("Kare hızı");
                    for f in [12.0, 15.0, 24.0, 25.0, 30.0, 50.0, 60.0] {
                        ui.selectable_value(&mut s.fps, f, format!("{f}"));
                    }
                });
            }
            if s.format == Format::Mp4 {
                ui.add(egui::Slider::new(&mut s.bitrate, 1.0..=100.0).text("Mbps"));
                ui.checkbox(&mut s.audio, "Sesi dahil et");
            }
            if matches!(s.format, Format::PngSeq | Format::PngFrame) {
                ui.checkbox(&mut s.transparent, "Saydam arka plan");
            }
            if let Some(m) = &self.export.message {
                ui.separator();
                ui.label(m);
            }
            ui.add_space(6.0);
            if ui.add(egui::Button::new(RichText::new("Dışa Aktar").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                begin = true;
            }
        });
        if begin {
            let name = safe_filename(&self.p.name);
            let target = match self.export.settings.format {
                Format::Mp4 => rfd::FileDialog::new().add_filter("MP4", &["mp4"]).set_file_name(format!("{name}.mp4")).save_file(),
                Format::Gif => rfd::FileDialog::new().add_filter("GIF", &["gif"]).set_file_name(format!("{name}.gif")).save_file(),
                Format::PngFrame => rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(format!("{name}.png")).save_file(),
                Format::PngSeq => rfd::FileDialog::new().set_title("PNG dizisi için klasör seç").pick_folder(),
            };
            if let Some(t) = target {
                self.export.message = None;
                self.start_export(t, assets.clone());
            }
        }
        if !open && self.export.job.is_none() {
            self.export.open = false;
        }
        let finished = self.export.job.as_ref().and_then(|j| j.result.lock().ok()?.take().map(|r| (r, j.out.clone())));
        if let Some((r, out)) = finished {
            self.export.job = None;
            self.export.message = Some(match r {
                Ok(()) => format!("Tamamlandı: {}", out.display()),
                Err(e) => e,
            });
        }
    }

    fn share_window(&mut self, ctx: &egui::Context) {
        if !self.share_open {
            return;
        }
        let mut open = true;
        let mut begin = false;
        egui::Window::new("Projeyi Paylaş").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            if self.share_job.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("XML yazılıyor...");
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            ui.label("Proje tek bir XML dosyası olur. Karşı taraf giriş ekranından \"XML İçe Aktar\" ile açar.");
            ui.checkbox(&mut self.share_embed, "Medyayı dosyanın içine göm (önerilir)");
            if self.share_embed {
                let mb = crate::xml::media_size(&self.p) as f64 / (1024.0 * 1024.0);
                ui.label(RichText::new(format!("Yaklaşık dosya boyutu: {:.1} MB", mb * 4.0 / 3.0)).color(TEXT_DIM));
            } else {
                ui.label(RichText::new("Medya dosyalarını da XML ile aynı klasörde göndermelisin.").color(TEXT_DIM));
            }
            ui.label(RichText::new("Bilgisayarındaki klasör yolları dosyaya yazılmaz.").color(TEXT_DIM));
            if let Some(m) = &self.share_message {
                ui.separator();
                ui.label(m);
            }
            ui.add_space(6.0);
            if ui.add(egui::Button::new(RichText::new("XML Olarak Kaydet").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                begin = true;
            }
        });
        if begin {
            let name = safe_filename(&self.p.name);
            if let Some(out) = rfd::FileDialog::new().add_filter("CompiMotion XML", &["xml"]).set_file_name(format!("{name}.xml")).save_file() {
                self.start_share(out);
            }
        }
        if !open && self.share_job.is_none() {
            self.share_open = false;
        }
        let done = self.share_job.as_ref().and_then(|j| j.lock().ok()?.take());
        if let Some(r) = done {
            self.share_job = None;
            self.share_message = Some(match r {
                Ok(rep) if rep.missing.is_empty() => format!("Kaydedildi. {} medya dosyası gömüldü.", rep.embedded),
                Ok(rep) => format!("Kaydedildi, ama {}", missing_text(&rep.missing)),
                Err(e) => format!("Kaydedilemedi: {e}"),
            });
        }
    }

    fn start_share(&mut self, out: PathBuf) {
        let slot: JobSlot<crate::xml::Report> = Arc::new(Mutex::new(None));
        let (p, embed, res) = (self.p.clone(), self.share_embed, slot.clone());
        std::thread::spawn(move || {
            let r = crate::xml::write(&p, &out, embed);
            if let Ok(mut g) = res.lock() {
                *g = Some(r);
            }
        });
        self.share_job = Some(slot);
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, Modifiers};
        let press = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        // The window layer turns Ctrl+C/V into clipboard events; plain key events also count.
        let (copied, pasted) = ctx.input(|i| {
            let c = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let p = i.events.iter().find_map(|e| if let egui::Event::Paste(s) = e { Some(s.clone()) } else { None });
            (c, p)
        });
        if copied || press(Modifiers::COMMAND, Key::C) {
            self.copy_shortcut(ctx);
        }
        let key_v = press(Modifiers::COMMAND, Key::V);
        if pasted.is_some() || key_v {
            let external = pasted.as_deref().and_then(crate::keys::KeyClip::from_text);
            self.paste_keys(external);
        }
        if press(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || press(Modifiers::COMMAND, Key::Y) {
            self.do_redo();
        }
        if press(Modifiers::COMMAND, Key::Z) {
            self.do_undo();
        }
        if press(Modifiers::COMMAND, Key::S) {
            self.save();
        }
        // Checked first: a plain Ctrl+D match would also accept Ctrl+Shift+D.
        if press(Modifiers::COMMAND | Modifiers::SHIFT, Key::D) {
            self.split_middle_selected();
        }
        if press(Modifiers::COMMAND, Key::D) {
            self.duplicate_selected();
        }
        if press(Modifiers::COMMAND, Key::G) {
            self.group_selected();
        }
        if press(Modifiers::NONE, Key::Space) {
            self.playing = !self.playing;
        }
        if press(Modifiers::NONE, Key::Delete) || press(Modifiers::NONE, Key::Backspace) {
            // Selected keyframes go first, so deleting one never takes its whole layer.
            if self.sel_keys.is_empty() {
                self.delete_selected();
            } else {
                self.delete_selected_keys();
            }
        }
        if press(Modifiers::NONE, Key::S) {
            self.split_selected();
        }
        if press(Modifiers::NONE, Key::V) {
            self.tool = Tool::Select;
        }
        if press(Modifiers::NONE, Key::P) {
            self.tool = Tool::Pen;
        }
        if press(Modifiers::NONE, Key::B) {
            self.tool = Tool::Freehand;
        }
        if press(Modifiers::NONE, Key::ArrowLeft) {
            self.step_frame(-1);
        }
        if press(Modifiers::NONE, Key::ArrowRight) {
            self.step_frame(1);
        }
        if press(Modifiers::NONE, Key::Home) {
            self.t = 0.0;
        }
        if press(Modifiers::NONE, Key::End) {
            self.t = self.p.dur;
        }
    }

    fn top_actions(&mut self, ui: &mut egui::Ui) {
        if ui.add(egui::Button::new(RichText::new("Dışa Aktar").color(Color32::WHITE)).fill(ACCENT)).clicked() {
            self.export.settings.fps = self.p.fps;
            self.export.open = true;
        }
        if ui.button("Paylaş (XML)").on_hover_text("Projeyi başkasına gönderilecek tek bir dosya yap").clicked() {
            self.share_open = true;
            self.share_message = None;
        }
        egui::ComboBox::from_id_salt("pq").selected_text(format!("Önizleme %{}", (self.preview_scale * 100.0) as u32)).show_ui(ui, |ui| {
            for k in [0.25, 0.5, 0.75, 1.0] {
                ui.selectable_value(&mut self.preview_scale, k, format!("%{}", (k * 100.0) as u32));
            }
        });
        ui.checkbox(&mut self.show_grid, "Kılavuz");
    }

    fn top_left(&mut self, ui: &mut egui::Ui, leave: &mut bool, wide: bool) {
        if ui.button("← Projeler").clicked() {
            *leave = true;
        }
        let r = ui.add(egui::TextEdit::singleline(&mut self.p.name).desired_width(if wide { 200.0 } else { 120.0 }));
        if r.changed() {
            self.touch();
        }
        if wide {
            ui.label(RichText::new(format!("{}×{} · {} kare/sn", self.p.w, self.p.h, self.p.fps)).color(TEXT_DIM));
        }
        if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("↶ Geri")).on_hover_text("Ctrl+Z").clicked() {
            self.do_undo();
        }
        if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("↷ İleri")).on_hover_text("Ctrl+Y").clicked() {
            self.do_redo();
        }
    }

    /// On narrow windows the bar wraps onto a second row instead of overlapping itself.
    fn top_bar(&mut self, ui: &mut egui::Ui, leave: &mut bool) {
        if ui.available_width() >= 980.0 {
            ui.horizontal(|ui| {
                self.top_left(ui, leave, true);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.top_actions(ui));
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                self.top_left(ui, leave, false);
                self.top_actions(ui);
            });
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, assets: &Arc<Assets>, renderer: &Renderer, player: &Player) -> bool {
        let ctx = ui.ctx().clone();
        let mut leave = false;
        self.shortcuts(&ctx);

        // Messages fade out on their own, a few seconds after they change.
        let clock = ctx.input(|i| i.time);
        if self.status != self.status_seen.0 {
            self.status_seen = (self.status.clone(), clock);
        } else if self.status.is_some() {
            if clock - self.status_seen.1 > 7.0 {
                self.status = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
        }

        // Clock: playback advances by wall time so slow frames skip instead of slowing down.
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        if self.playing {
            self.t += dt;
            if self.t >= self.p.dur {
                self.t = 0.0;
                if let Ok(mut s) = player.shared.lock() {
                    s.pos = 0.0;
                }
            }
            ctx.request_repaint();
        }
        if let Ok(mut s) = player.shared.lock() {
            if self.playing && !s.playing {
                s.pos = self.t;
            }
            s.playing = self.playing;
            s.project = Some(self.snapshot.clone());
        }

        egui::Panel::top("top").frame(egui::Frame::new().fill(BG1).inner_margin(8.0)).show(ui, |ui| self.top_bar(ui, &mut leave));
        // Sizes follow the window, so the canvas keeps its share on small or scaled screens.
        let room = ui.available_rect_before_wrap().size();
        let timeline_h = (room.y * 0.38).clamp(180.0, 320.0);
        let props_w = (room.x * 0.26).clamp(220.0, 340.0);
        egui::Panel::bottom("timeline").resizable(true).default_size(timeline_h).min_size(130.0).max_size((room.y * 0.65).max(140.0)).frame(egui::Frame::new().fill(BG1)).show(ui, |ui| crate::ui_timeline::show(self, ui, assets));
        egui::Panel::left("tools").exact_size(148.0).frame(egui::Frame::new().fill(BG1).inner_margin(8.0)).show(ui, |ui| crate::ui_props::add_panel(self, ui, assets));
        egui::Panel::right("props").resizable(true).default_size(props_w).min_size(200.0).max_size((room.x * 0.5).max(210.0)).frame(egui::Frame::new().fill(BG1).inner_margin(8.0)).show(ui, |ui| crate::ui_props::show(self, ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG0)).show(ui, |ui| crate::ui_preview::show(self, ui, assets));

        if self.effect_browser {
            crate::ui_props::effect_browser(self, &ctx);
        }
        self.export_window(&ctx, assets);
        self.share_window(&ctx);

        let down = ctx.input(|i| i.pointer.any_down());
        self.commit_if_done(down);

        // Hand the latest state to the render thread.
        if self.snapshot_stale {
            self.snapshot = Arc::new(self.p.clone());
            self.snapshot_stale = false;
            self.force_render = true;
        }
        if let Some(r) = renderer.take() {
            let img = egui::ColorImage::from_rgba_unmultiplied([r.w, r.h], &r.rgba);
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.tex = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR)),
            }
        }
        if self.force_render || self.last_req != (self.t, self.preview_scale) {
            renderer.request(self.snapshot.clone(), self.t, self.preview_scale);
            self.last_req = (self.t, self.preview_scale);
            self.force_render = false;
        }
        leave
    }
}

pub fn clear_parents(v: &mut [Layer], gone: &[u64]) {
    for l in v {
        if l.parent.is_some_and(|p| gone.contains(&p)) {
            l.parent = None;
        }
        if let Content::Group { children } = &mut l.content {
            clear_parents(children, gone);
        }
    }
}

pub fn reassign_ids(l: &mut Layer, p: &mut Project) {
    l.id = p.alloc_id();
    if let Content::Group { children } = &mut l.content {
        for c in children {
            reassign_ids(c, p);
        }
    }
}

pub fn insert_above(v: &mut Vec<Layer>, id: u64, new: Layer) {
    fn go(v: &mut Vec<Layer>, id: u64, new: &mut Option<Layer>) {
        if let Some(i) = v.iter().position(|l| l.id == id) {
            if let Some(n) = new.take() {
                v.insert(i + 1, n);
            }
            return;
        }
        for l in v.iter_mut() {
            if let Content::Group { children } = &mut l.content {
                go(children, id, new);
            }
        }
    }
    go(v, id, &mut Some(new));
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.root(ui);
    }
}

impl App {
    pub fn root(&mut self, ui: &mut egui::Ui) {
        // Files dropped on the window become layers.
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        let is_xml = |p: &PathBuf| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml"));
        let (xmls, dropped): (Vec<PathBuf>, Vec<PathBuf>) = dropped.into_iter().partition(is_xml);
        if let Some(x) = xmls.into_iter().next() {
            // A shared project replaces whatever is open, which is saved first.
            if let Some(ed) = &mut self.ed {
                ed.save();
                self.key_clip = ed.key_clip.take();
                if let Ok(mut s) = self.player.shared.lock() {
                    s.playing = false;
                    s.project = None;
                }
            }
            self.ed = None;
            self.start_import(x);
        }
        if let Some(ed) = &mut self.ed {
            for d in dropped {
                ed.import_media(&d, &self.assets);
            }
            let leave = ed.ui(ui, &self.assets, &self.renderer, &self.player);
            if leave {
                ed.save();
                if let Ok(mut s) = self.player.shared.lock() {
                    s.playing = false;
                    s.project = None;
                }
                self.key_clip = ed.key_clip.take();
                self.ed = None;
                self.home.entries = scan_projects();
            }
        } else {
            self.home_ui(ui);
        }
    }
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = media::fonts_dir();
    let mut add = |name: &str, file: &str, front: bool| {
        if let Ok(data) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(name.to_string(), Arc::new(egui::FontData::from_owned(data)));
            let list = fonts.families.entry(egui::FontFamily::Proportional).or_default();
            if front {
                list.insert(0, name.to_string());
            } else {
                list.push(name.to_string());
            }
        }
    };
    add("segoe", "segoeui.ttf", true);
    add("symbols", "seguisym.ttf", false);
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG1;
    v.window_fill = BG1;
    v.extreme_bg_color = BG0;
    v.selection.bg_fill = ACCENT;
    v.widgets.inactive.weak_bg_fill = BG2;
    // Slider rails and checkboxes use this fill, and often sit on BG2 panels.
    v.widgets.inactive.bg_fill = BG3;
    v.widgets.hovered.weak_bg_fill = BG3;
    v.widgets.hovered.bg_fill = BG3;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.widgets.active.bg_fill = ACCENT;
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
}
