//! Media loading through Windows Media Foundation, plus image and font caches.
use crate::frame::Frame;
use ab_glyph::FontArc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Variant::VT_I8;
use windows::core::{GUID, HSTRING};

pub const AUDIO_RATE: u32 = 48_000;

/// Starts COM and Media Foundation once per thread that touches them.
pub fn ensure_mf() {
    thread_local!(static READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    READY.with(|r| {
        if !r.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
            }
            r.set(true);
        }
    });
}

fn seek_time(reader: &IMFSourceReader, hns: i64) -> windows::core::Result<()> {
    let mut pv = PROPVARIANT::default();
    unsafe {
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_I8;
        inner.Anonymous.hVal = hns;
        reader.SetCurrentPosition(&GUID::zeroed(), &pv)
    }
}

fn duration_of(reader: &IMFSourceReader) -> f64 {
    unsafe {
        reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .map(|pv| pv.Anonymous.Anonymous.Anonymous.uhVal as f64 / 1e7)
            .unwrap_or(0.0)
    }
}

pub struct VideoReader {
    reader: IMFSourceReader,
    pub w: usize,
    pub h: usize,
    stride: i32,
    pub duration: f64,
    pub fps: f64,
    cache: Vec<(f64, f64, Arc<Frame>)>,
    /// Timestamp of the next frame the decoder will return, when known.
    pos: f64,
    eof: bool,
}
// The source reader is free-threaded; each VideoReader is only used behind a Mutex.
unsafe impl Send for VideoReader {}

impl VideoReader {
    pub fn open(path: &str) -> windows::core::Result<Self> {
        ensure_mf();
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2)?;
            let attrs = attrs.ok_or(windows::core::Error::empty())?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path), &attrs)?;
            let first_video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(first_video, true)?;
            let ty = MFCreateMediaType()?;
            ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            ty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            reader.SetCurrentMediaType(first_video, None, &ty)?;
            let cur = reader.GetCurrentMediaType(first_video)?;
            let size = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (w, h) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
            let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(w as i32 * 4);
            let fps = cur.GetUINT64(&MF_MT_FRAME_RATE).map(|r| (r >> 32) as f64 / ((r & 0xffff_ffff) as f64).max(1.0)).unwrap_or(30.0);
            let duration = duration_of(&reader);
            Ok(VideoReader { reader, w, h, stride, duration, fps: if fps > 0.0 { fps } else { 30.0 }, cache: vec![], pos: 0.0, eof: false })
        }
    }

    fn decode_next(&mut self) -> Option<(f64, Arc<Frame>)> {
        unsafe {
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
            self.reader.ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).ok()?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                self.eof = true;
                return None;
            }
            let sample: IMFSample = sample?;
            let buf = sample.ConvertToContiguousBuffer().ok()?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            buf.Lock(&mut ptr, None, Some(&mut len)).ok()?;
            let data = std::slice::from_raw_parts(ptr, len as usize);
            let f = self.convert(data);
            let _ = buf.Unlock();
            Some((ts as f64 / 1e7, Arc::new(f)))
        }
    }

    fn convert(&self, d: &[u8]) -> Frame {
        use rayon::prelude::*;
        let (w, h) = (self.w, self.h);
        let pitch = self.stride.unsigned_abs() as usize;
        let mut fr = Frame::new(w, h);
        // A negative stride means the buffer stores rows bottom-up.
        let flip = self.stride < 0;
        fr.px.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            let sy = if flip { h - 1 - y } else { y };
            let Some(src) = d.get(sy * pitch..sy * pitch + w * 4) else { return };
            for (o, b) in row.iter_mut().zip(src.chunks_exact(4)) {
                *o = [b[2] as f32 / 255.0, b[1] as f32 / 255.0, b[0] as f32 / 255.0, 1.0];
            }
        });
        fr
    }

    pub fn frame_at(&mut self, t: f64) -> Option<Arc<Frame>> {
        let t = t.clamp(0.0, (self.duration - 1e-3).max(0.0));
        let fd = 1.0 / self.fps;
        if let Some(c) = self.cache.iter().find(|c| t >= c.0 && t < c.1) {
            return Some(c.2.clone());
        }
        if t < self.pos - 1e-3 || t > self.pos + 1.0 {
            seek_time(&self.reader, (t * 1e7) as i64).ok()?;
            self.eof = false;
            self.cache.clear();
            self.pos = t;
        }
        let mut last = None;
        while !self.eof {
            let Some((ts, f)) = self.decode_next() else { break };
            let end = ts + fd;
            self.pos = end;
            self.cache.push((ts, end, f.clone()));
            if self.cache.len() > 4 {
                self.cache.remove(0);
            }
            last = Some(f);
            if end > t {
                break;
            }
        }
        last.or_else(|| self.cache.last().map(|c| c.2.clone()))
    }
}

/// Container duration in seconds, without decoding.
pub fn probe_duration(path: &str) -> Option<f64> {
    ensure_mf();
    let reader = unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(path), None) }.ok()?;
    Some(duration_of(&reader)).filter(|d| *d > 0.0)
}

/// Decodes the first audio stream to stereo float at AUDIO_RATE.
pub fn decode_audio(path: &str) -> Option<Arc<Vec<f32>>> {
    ensure_mf();
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path), None).ok()?;
        let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false).ok()?;
        reader.SetStreamSelection(stream, true).ok()?;
        let ty = MFCreateMediaType().ok()?;
        ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ok()?;
        ty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).ok()?;
        reader.SetCurrentMediaType(stream, None, &ty).ok()?;
        let cur = reader.GetCurrentMediaType(stream).ok()?;
        let rate = cur.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).ok()? as f64;
        let ch = cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).ok()?.max(1) as usize;
        let mut raw: Vec<f32> = vec![];
        loop {
            let (mut flags, mut sample) = (0u32, None);
            reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)).ok()?;
            if let Some(s) = sample {
                let s: IMFSample = s;
                let buf = s.ConvertToContiguousBuffer().ok()?;
                let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
                buf.Lock(&mut ptr, None, Some(&mut len)).ok()?;
                let data = std::slice::from_raw_parts(ptr as *const f32, len as usize / 4);
                raw.extend_from_slice(data);
                let _ = buf.Unlock();
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        let frames = raw.len() / ch;
        let out_frames = (frames as f64 * AUDIO_RATE as f64 / rate) as usize;
        let mut out = Vec::with_capacity(out_frames * 2);
        let at = |f: usize, c: usize| raw.get(f * ch + c.min(ch - 1)).copied().unwrap_or(0.0);
        for i in 0..out_frames {
            let pos = i as f64 * rate / AUDIO_RATE as f64;
            let (fi, k) = (pos as usize, pos.fract() as f32);
            for c in 0..2 {
                out.push(at(fi, c) + (at(fi + 1, c) - at(fi, c)) * k);
            }
        }
        Some(Arc::new(out))
    }
}

#[derive(Default)]
pub struct Assets {
    images: Mutex<HashMap<String, Option<Arc<Frame>>>>,
    videos: Mutex<HashMap<String, Option<Arc<Mutex<VideoReader>>>>>,
    audio: Mutex<HashMap<String, Option<Arc<Vec<f32>>>>>,
    fonts: Mutex<HashMap<String, Option<FontArc>>>,
}

impl Assets {
    pub fn image(&self, path: &str) -> Option<Arc<Frame>> {
        let mut m = self.images.lock().ok()?;
        m.entry(path.to_string())
            .or_insert_with(|| {
                let img = image::open(path).ok()?.to_rgba8();
                Some(Arc::new(Frame::from_rgba8(img.width() as usize, img.height() as usize, img.as_raw())))
            })
            .clone()
    }
    fn reader(&self, path: &str) -> Option<Arc<Mutex<VideoReader>>> {
        let mut m = self.videos.lock().ok()?;
        m.entry(path.to_string()).or_insert_with(|| VideoReader::open(path).ok().map(|r| Arc::new(Mutex::new(r)))).clone()
    }
    pub fn video_frame(&self, path: &str, t: f64) -> Option<Arc<Frame>> {
        self.reader(path)?.lock().ok()?.frame_at(t)
    }
    pub fn video_info(&self, path: &str) -> Option<(usize, usize, f64, f64)> {
        let r = self.reader(path)?;
        let r = r.lock().ok()?;
        Some((r.w, r.h, r.duration, r.fps))
    }
    pub fn audio(&self, path: &str) -> Option<Arc<Vec<f32>>> {
        if let Some(v) = self.audio.lock().ok()?.get(path) {
            return v.clone();
        }
        // Decoding is slow; the map is not held meanwhile, so playback can keep reading it.
        let decoded = decode_audio(path);
        self.audio.lock().ok()?.insert(path.to_string(), decoded.clone());
        decoded
    }
    /// Non-blocking read for the audio callback.
    pub fn audio_cached(&self, path: &str) -> Option<Arc<Vec<f32>>> {
        self.audio.try_lock().ok()?.get(path).cloned().flatten()
    }
    /// Forgets failed lookups and fallbacks, for when new fonts have just been installed.
    pub fn clear_missing_fonts(&self) {
        if let Ok(mut m) = self.fonts.lock() {
            m.clear();
        }
    }
    pub fn font(&self, name: &str) -> Option<FontArc> {
        let mut m = self.fonts.lock().ok()?;
        m.entry(name.to_string()).or_insert_with(|| load_font(name)).clone()
    }
}

/// `name` is a file in the user's added fonts or the Windows fonts folder,
/// with common fallbacks when it is missing.
fn load_font(name: &str) -> Option<FontArc> {
    let plain = !name.contains(['/', '\\']);
    let wanted = [(plain, custom_fonts_dir().join(name)), (plain, fonts_dir().join(name))];
    let fallbacks = ["segoeui.ttf", "arial.ttf", "tahoma.ttf"].map(|f| (true, fonts_dir().join(f)));
    for (ok, p) in wanted.into_iter().chain(fallbacks) {
        if !ok {
            continue;
        }
        if let Ok(data) = std::fs::read(&p) {
            if let Ok(font) = ab_glyph::FontVec::try_from_vec(data) {
                return Some(FontArc::new(font));
            }
        }
    }
    None
}

/// Fonts the user added, available to every project.
pub fn custom_fonts_dir() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&base).join("CompiMotion").join("fonts")
}

/// Stores font bytes in `dir` under a safe name and returns that name. The same font again
/// reuses its file; a different font with a taken name gets a numbered one.
pub fn install_font(dir: &std::path::Path, name: &str, bytes: &[u8]) -> Result<String, String> {
    use ab_glyph::Font;
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base.chars().filter(|c| c.is_alphanumeric() || " ._-()".contains(*c)).collect();
    let clean = clean.trim_matches(['.', ' ']).to_string();
    let path = std::path::Path::new(&clean);
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != "ttf" && ext != "otf" {
        return Err(format!("Yalnızca TTF ve OTF yazı tipleri eklenebilir: {base}"));
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().chars().take(80).collect::<String>()).filter(|s| !s.is_empty()).unwrap_or_else(|| "font".into());
    let parsed = ab_glyph::FontVec::try_from_vec(bytes.to_vec()).map_err(|_| format!("Bu dosya geçerli bir yazı tipi değil: {base}"))?;
    if parsed.glyph_count() == 0 {
        return Err(format!("Bu yazı tipinde hiç karakter yok: {base}"));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for n in 1..1000 {
        let candidate = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem}_{n}.{ext}") };
        let dest = dir.join(&candidate);
        match std::fs::read(&dest) {
            Ok(existing) if existing == bytes => return Ok(candidate),
            Ok(_) => continue,
            Err(_) => {
                std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
                return Ok(candidate);
            }
        }
    }
    Err(format!("Bu adla çok fazla yazı tipi var: {stem}"))
}

pub fn import_font_file(path: &std::path::Path) -> Result<String, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| format!("Yazı tipi okunamadı: {e}"))?;
    install_font(&custom_fonts_dir(), &name, &bytes)
}

/// Whether `name` resolves in `custom` or the system folder, without any fallback.
pub fn font_available(custom: &std::path::Path, name: &str) -> bool {
    !name.contains(['/', '\\']) && (custom.join(name).is_file() || fonts_dir().join(name).is_file())
}

pub fn fonts_dir() -> std::path::PathBuf {
    let win = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    std::path::Path::new(&win).join("Fonts")
}

/// Added fonts first, then the system's.
pub fn list_fonts() -> Vec<String> {
    let read = |dir: std::path::PathBuf| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| {
                        let l = n.to_lowercase();
                        l.ends_with(".ttf") || l.ends_with(".otf")
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|s| s.to_lowercase());
        v
    };
    let mut all = read(custom_fonts_dir());
    for f in read(fonts_dir()) {
        if !all.contains(&f) {
            all.push(f);
        }
    }
    all
}

#[cfg(test)]
mod font_tests {
    use super::*;

    fn system_font() -> Vec<u8> {
        std::fs::read(fonts_dir().join("arial.ttf")).expect("arial.ttf")
    }
    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cm_font_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_real_font_installs_and_reinstalling_reuses_it() {
        let d = dir("ok");
        let f = system_font();
        assert_eq!(install_font(&d, "Benim Yazım.TTF", &f).unwrap(), "Benim Yazım.ttf");
        assert_eq!(install_font(&d, "Benim Yazım.ttf", &f).unwrap(), "Benim Yazım.ttf");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    }

    #[test]
    fn a_different_font_with_the_same_name_keeps_both() {
        let d = dir("clash");
        let tahoma = std::fs::read(fonts_dir().join("tahoma.ttf")).expect("tahoma.ttf");
        install_font(&d, "a.ttf", &system_font()).unwrap();
        assert_eq!(install_font(&d, "a.ttf", &tahoma).unwrap(), "a_2.ttf");
    }

    #[test]
    fn bad_files_and_hostile_names_are_refused_or_contained() {
        let d = dir("bad");
        assert!(install_font(&d, "bozuk.ttf", b"bu bir font degil").is_err());
        assert!(install_font(&d, "virus.exe", &system_font()).is_err());
        assert_eq!(install_font(&d, "..\\..\\kacak.ttf", &system_font()).unwrap(), "kacak.ttf");
        assert!(d.join("kacak.ttf").is_file(), "klasörün içinde kalmalı");
        assert!(!font_available(&d, "..\\kacak.ttf"));
    }
}
