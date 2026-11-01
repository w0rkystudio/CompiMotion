//! Media loading through Windows Media Foundation, plus image and font caches.
use crate::frame::Frame;
use ab_glyph::FontArc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Variant::VT_I8;
use windows::core::{GUID, HSTRING, Interface};

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

/// How decoded buffers map to the picture a player would show.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Layout {
    /// Buffer size, which can include padding rows and columns the codec needs.
    coded: (usize, usize),
    /// Visible part of the buffer: x, y, width, height.
    crop: (usize, usize, usize, usize),
    /// Clockwise degrees to turn the picture, from the file's metadata.
    rotation: u32,
    /// Pixel width over pixel height; 1 for square pixels.
    pixel_aspect: f64,
    /// Row stride from the media type, if it states one.
    stride: Option<i32>,
}

impl Layout {
    /// Picture size after square-pixel correction and rotation.
    fn display_size(&self) -> (usize, usize) {
        let w = ((self.crop.2 as f64 * self.pixel_aspect).round() as usize).max(1);
        if self.rotation == 90 || self.rotation == 270 { (self.crop.3, w) } else { (w, self.crop.3) }
    }
}

/// Reads an MFVideoArea attribute as x, y, width, height.
unsafe fn read_area(ty: &IMFMediaType, key: &GUID) -> Option<(i32, i32, i32, i32)> {
    let mut b = [0u8; 16];
    unsafe { ty.GetBlob(key, &mut b, None) }.ok()?;
    let i16_at = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]) as i32;
    let i32_at = |i: usize| i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    Some((i16_at(2), i16_at(6), i32_at(8), i32_at(12)))
}

unsafe fn read_layout(reader: &IMFSourceReader, stream: u32) -> windows::core::Result<Layout> {
    unsafe {
        let cur = reader.GetCurrentMediaType(stream)?;
        let native = reader.GetNativeMediaType(stream, 0).ok();
        let size = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let coded = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);

        // The output type says what the converter delivers; the native type what the file says.
        let types: Vec<&IMFMediaType> = std::iter::once(&cur).chain(native.iter()).collect();
        let area = types.iter().find_map(|t| read_area(t, &MF_MT_MINIMUM_DISPLAY_APERTURE).or_else(|| read_area(t, &MF_MT_GEOMETRIC_APERTURE)));
        let mut crop = (0, 0, coded.0, coded.1);
        if let Some((x, y, w, h)) = area {
            let fits = x >= 0 && y >= 0 && w > 0 && h > 0 && (x + w) as usize <= coded.0 && (y + h) as usize <= coded.1;
            if fits {
                crop = (x as usize, y as usize, w as usize, h as usize);
            }
        }
        let rotation = types.iter().find_map(|t| t.GetUINT32(&MF_MT_VIDEO_ROTATION).ok().filter(|r| *r != 0)).unwrap_or(0) % 360;
        let pixel_aspect = native
            .as_ref()
            .and_then(|t| t.GetUINT64(&MF_MT_PIXEL_ASPECT_RATIO).ok())
            .map(|r| ((r >> 32) as f64, (r & 0xffff_ffff) as f64))
            .filter(|r| r.0 > 0.0 && r.1 > 0.0)
            .map_or(1.0, |r| r.0 / r.1);
        // Tiny ratios only come from rounding in the file, and would just soften the picture.
        let pixel_aspect = if (pixel_aspect - 1.0).abs() < 0.01 { 1.0 } else { pixel_aspect.clamp(0.25, 4.0) };
        let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).ok().map(|s| s as i32);
        Ok(Layout { coded, crop, rotation, pixel_aspect, stride })
    }
}

/// Copies the visible part of a BGRX buffer. `row` gives each coded row's bytes.
fn frame_from_rows<'a>(l: &Layout, row: impl Fn(usize) -> Option<&'a [u8]> + Sync) -> Frame {
    use rayon::prelude::*;
    let (cx, cy, cw, ch) = l.crop;
    let mut fr = Frame::new(cw, ch);
    fr.px.par_chunks_mut(cw.max(1)).enumerate().for_each(|(y, out)| {
        let Some(src) = row(cy + y).and_then(|r| r.get(cx * 4..(cx + cw) * 4)) else { return };
        for (o, b) in out.iter_mut().zip(src.chunks_exact(4)) {
            *o = [b[2] as f32 / 255.0, b[1] as f32 / 255.0, b[0] as f32 / 255.0, 1.0];
        }
    });
    fr
}

/// Applies pixel aspect, then rotation, giving the picture as displayed.
fn to_display(fr: Frame, l: &Layout) -> Frame {
    let (src_w, src_h) = (fr.w as f32, fr.h as f32);
    let squared = if l.pixel_aspect == 1.0 {
        fr
    } else {
        let w = ((src_w as f64 * l.pixel_aspect).round() as usize).max(1);
        let k = src_w / w as f32;
        let mut o = Frame::new(w, fr.h);
        o.shade(|x, y, _| fr.sample_clamp(x * k, y));
        o
    };
    if l.rotation == 0 {
        return squared;
    }
    let (w, h) = (squared.w as f32, squared.h as f32);
    let (ow, oh) = if l.rotation == 180 { (squared.w, squared.h) } else { (squared.h, squared.w) };
    let mut o = Frame::new(ow, oh);
    let rot = l.rotation;
    o.shade(|x, y, _| match rot {
        90 => squared.sample_clamp(y, h - x),
        180 => squared.sample_clamp(w - x, h - y),
        _ => squared.sample_clamp(w - y, x),
    });
    o
}

pub struct VideoReader {
    reader: IMFSourceReader,
    pub w: usize,
    pub h: usize,
    layout: Layout,
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
            let layout = read_layout(&reader, first_video)?;
            let (w, h) = layout.display_size();
            let fps = cur.GetUINT64(&MF_MT_FRAME_RATE).map(|r| (r >> 32) as f64 / ((r & 0xffff_ffff) as f64).max(1.0)).unwrap_or(30.0);
            let duration = duration_of(&reader);
            Ok(VideoReader { reader, w, h, layout, duration, fps: if fps > 0.0 { fps } else { 30.0 }, cache: vec![], pos: 0.0, eof: false })
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
            // A stream may change resolution partway through.
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                if let Ok(l) = read_layout(&self.reader, MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32) {
                    self.layout = l;
                    (self.w, self.h) = l.display_size();
                }
            }
            let sample: IMFSample = sample?;
            let f = self.read_sample(&sample)?;
            Some((ts as f64 / 1e7, Arc::new(to_display(f, &self.layout))))
        }
    }

    unsafe fn read_sample(&self, sample: &IMFSample) -> Option<Frame> {
        unsafe {
            let l = self.layout;
            let (cw, ch) = l.coded;
            let row_bytes = cw * 4;
            // A 2D buffer reports the real pitch; padding differs between decoders and sizes.
            let single = if sample.GetBufferCount().ok()? == 1 { sample.GetBufferByIndex(0).ok() } else { None };
            if let Some(b2d) = single.as_ref().and_then(|b| b.cast::<IMF2DBuffer>().ok()) {
                let (mut scan0, mut pitch) = (std::ptr::null_mut(), 0i32);
                if b2d.Lock2D(&mut scan0, &mut pitch).is_ok() {
                    let base = scan0 as usize;
                    let f = frame_from_rows(&l, |y| {
                        if y >= ch {
                            return None;
                        }
                        let at = (base as isize + y as isize * pitch as isize) as *const u8;
                        Some(std::slice::from_raw_parts(at, row_bytes))
                    });
                    let _ = b2d.Unlock2D();
                    return Some(f);
                }
            }
            let buf = sample.ConvertToContiguousBuffer().ok()?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            buf.Lock(&mut ptr, None, Some(&mut len)).ok()?;
            let data = std::slice::from_raw_parts(ptr as *const u8, len as usize);
            // Only trust the stated stride when it fits this buffer. Otherwise the padding is
            // what is left per row; uncompressed RGB with no stated stride is stored bottom-up.
            let stated = l.stride.filter(|s| {
                let a = s.unsigned_abs() as usize;
                a >= row_bytes && a * ch <= data.len()
            });
            let pitch = stated.map_or(data.len() / ch.max(1), |s| s.unsigned_abs() as usize);
            let flip = stated.map_or(l.stride.is_none(), |s| s < 0);
            let f = frame_from_rows(&l, |y| {
                let sy = if flip { ch.checked_sub(1 + y)? } else { y };
                data.get(sy * pitch..sy * pitch + row_bytes)
            });
            let _ = buf.Unlock();
            Some(f)
        }
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
mod video_tests {
    use super::*;

    fn layout(coded: (usize, usize), crop: (usize, usize, usize, usize), rotation: u32, pixel_aspect: f64) -> Layout {
        Layout { coded, crop, rotation, pixel_aspect, stride: None }
    }

    /// A 2x2 picture: red, green / blue, white. The red corner shows any turn.
    fn corners() -> Frame {
        let mut f = Frame::new(2, 2);
        f.px = vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0]];
        f
    }

    #[test]
    fn rows_use_the_real_pitch_and_visible_area() {
        // 3 x 2 visible pixels at (1, 1) in a padded buffer, BGRX, pitch 32 instead of 5 * 4.
        let (pitch, coded_h) = (32, 4);
        let mut buf = vec![0u8; pitch * coded_h];
        for y in 0..coded_h {
            for x in 0..5 {
                buf[y * pitch + x * 4] = (x * 10) as u8;
                buf[y * pitch + x * 4 + 1] = (y * 10) as u8;
            }
        }
        let l = layout((5, 4), (1, 1, 3, 2), 0, 1.0);
        let f = frame_from_rows(&l, |y| buf.get(y * pitch..y * pitch + 20));
        assert_eq!((f.w, f.h), (3, 2));
        let at = |x: usize, y: usize| f.px[y * f.w + x];
        assert!((at(0, 0)[2] - 10.0 / 255.0).abs() < 1e-5, "x kırpması");
        assert!((at(0, 0)[1] - 10.0 / 255.0).abs() < 1e-5, "y kırpması");
        assert!((at(2, 1)[2] - 30.0 / 255.0).abs() < 1e-5, "satır kaymamalı");
        assert!((at(2, 1)[1] - 20.0 / 255.0).abs() < 1e-5);
    }

    #[test]
    fn rotation_turns_the_picture_clockwise() {
        let close = |a: [f32; 4], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 0.01);
        let (red, green, blue, white) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 1.0]);
        for (deg, want) in [(90, [blue, red, white, green]), (180, [white, blue, green, red]), (270, [green, white, red, blue])] {
            let out = to_display(corners(), &layout((2, 2), (0, 0, 2, 2), deg, 1.0));
            for i in 0..4 {
                assert!(close(out.px[i], want[i]), "{deg} derece, piksel {i}: {:?}", out.px[i]);
            }
        }
    }

    #[test]
    fn display_size_applies_pixel_aspect_before_turning() {
        assert_eq!(layout((720, 576), (0, 0, 720, 576), 0, 16.0 / 15.0).display_size(), (768, 576));
        assert_eq!(layout((720, 576), (0, 0, 720, 576), 90, 16.0 / 15.0).display_size(), (576, 768));
        assert_eq!(layout((856, 480), (0, 0, 854, 480), 0, 1.0).display_size(), (854, 480));
        let wide = to_display(Frame::new(720, 4), &layout((720, 4), (0, 0, 720, 4), 0, 16.0 / 15.0));
        assert_eq!((wide.w, wide.h), (768, 4));
    }

    /// Compares decoded frames with reference images: CM_VIDEO_DIR holds videos, each with a
    /// "<file>.ref.png" made by ffmpeg at 1 s. Reports every file, then fails if any differ.
    #[test]
    #[ignore]
    fn decoded_frames_match_references() {
        let dir = std::env::var("CM_VIDEO_DIR").unwrap();
        let (mut bad, mut names) = (vec![], vec![]);
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".ref.png") && !name.ends_with(".got.png") && p.with_file_name(format!("{name}.ref.png")).exists() {
                names.push((p, name));
            }
        }
        names.sort();
        for (p, name) in names {
            let reference = image::open(p.with_file_name(format!("{name}.ref.png"))).unwrap().to_rgba8();
            let got = Assets::default().video_frame(&p.to_string_lossy(), 1.0);
            let Some(got) = got else {
                // Windows' H.264 decoder has no 10-bit support; the app reports this on import.
                println!("VIDEO {name:30} ACILAMADI");
                if !name.contains("10bit") {
                    bad.push(name);
                }
                continue;
            };
            let _ = image::save_buffer(p.with_file_name(format!("{name}.got.png")), &got.to_rgba8(), got.w as u32, got.h as u32, image::ExtendedColorType::Rgba8);
            let (rw, rh) = (reference.width() as usize, reference.height() as usize);
            let mut sum = 0.0f64;
            if (got.w, got.h) == (rw, rh) {
                for (o, q) in got.px.iter().zip(reference.pixels()) {
                    for c in 0..3 {
                        sum += (o[c] as f64 - q[c] as f64 / 255.0).abs();
                    }
                }
                sum /= (rw * rh * 3) as f64;
            } else {
                sum = 1.0;
            }
            let ok = sum < 0.04;
            println!("VIDEO {name:30} okunan {}x{} referans {rw}x{rh} fark {sum:.3} {}", got.w, got.h, if ok { "tamam" } else { "BOZUK" });
            if !ok {
                bad.push(name);
            }
        }
        assert!(bad.is_empty(), "bozuk: {bad:?}");
    }
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
