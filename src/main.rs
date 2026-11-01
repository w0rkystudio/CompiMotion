#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(dead_code)]
mod app;
mod audio;
mod cli;
mod demo;
mod effects;
mod export;
mod frame;
mod keys;
mod media;
mod model;
mod ops;
mod render;
mod ui_preview;
mod ui_props;
#[cfg(test)]
mod ui_tests;
mod ui_timeline;
mod vector;
mod xml;

use eframe::egui;

/// The window icon is drawn in code, so the exe needs no image files beside it.
fn icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (u, v) = ((x as f32 + 0.5) / n as f32 - 0.5, (y as f32 + 0.5) / n as f32 - 0.5);
            let d = u.hypot(v);
            // A play triangle cut out of a gradient disc.
            let inside_play = u > -0.14 && u < 0.24 && v.abs() < (0.24 - u) * 0.62;
            let i = (y * n + x) * 4;
            if d < 0.48 && !inside_play {
                rgba[i] = (124.0 + 130.0 * (u + 0.5)) as u8;
                rgba[i + 1] = (108.0 - 20.0 * (u + 0.5)) as u8;
                rgba[i + 2] = (255.0 - 120.0 * (v + 0.5)) as u8;
                rgba[i + 3] = 255;
            }
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}

fn options(renderer: eframe::Renderer) -> eframe::NativeOptions {
    eframe::NativeOptions {
        renderer,
        // Maximized fits any screen and display scale, which a fixed size cannot.
        viewport: egui::ViewportBuilder::default().with_title("CompiMotion").with_inner_size([1280.0, 720.0]).with_min_inner_size([760.0, 460.0]).with_maximized(true).with_icon(icon()).with_drag_and_drop(true),
        ..Default::default()
    }
}

fn log_path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    let dir = std::path::Path::new(&base).join("CompiMotion");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("hata.log")
}

fn log_line(text: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = writeln!(f, "[sürüm {}] {text}", env!("CARGO_PKG_VERSION"));
    }
}

/// With no console attached, a failure would otherwise look like the app just not opening.
fn show_error(text: &str) {
    log_line(text);
    rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("CompiMotion").set_description(format!("{text}\n\nAyrıntılar: {}", log_path().display())).show();
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }
    std::panic::set_hook(Box::new(|info| log_line(&format!("Beklenmedik hata: {info}"))));

    // Older or virtual graphics drivers may lack one of these, so the other gets a try.
    let mut last_error = None;
    for renderer in [eframe::Renderer::Wgpu, eframe::Renderer::Glow] {
        match eframe::run_native("CompiMotion", options(renderer), Box::new(|cc| Ok(Box::new(app::App::new(cc))))) {
            Ok(()) => return Ok(()),
            Err(e) => {
                log_line(&format!("{renderer} açılamadı: {e}"));
                last_error = Some(e);
            }
        }
    }
    if let Some(e) = &last_error {
        show_error(&format!("Grafik sistemi başlatılamadı. Ekran kartı sürücüsünü güncellemek sorunu çözebilir.\n\n{e}"));
    }
    last_error.map_or(Ok(()), Err)
}
