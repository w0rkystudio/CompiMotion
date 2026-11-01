//! Renders a project to MP4 (H.264 + AAC), GIF, PNG sequence or a single PNG.
use crate::audio::mix_layers;
use crate::frame::Frame;
use crate::media::{AUDIO_RATE, Assets, ensure_mf};
use crate::model::*;
use crate::render::render_project;
use rayon::prelude::*;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows::Win32::Media::MediaFoundation::*;
use windows::core::HSTRING;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Format {
    Mp4,
    Gif,
    PngSeq,
    PngFrame,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub format: Format,
    pub scale: f32,
    pub fps: f32,
    /// Megabits per second, for MP4.
    pub bitrate: f32,
    pub audio: bool,
    pub transparent: bool,
}

#[derive(Default)]
pub struct Progress {
    /// Thousandths of the job done.
    pub done: AtomicU32,
    pub cancel: AtomicBool,
}

/// `size` rounded down to even numbers, as H.264 requires.
fn out_size(p: &Project, scale: f32, even: bool) -> (u32, u32, f32) {
    let (mut w, mut h) = (((p.w as f32 * scale).round() as u32).max(2), ((p.h as f32 * scale).round() as u32).max(2));
    if even {
        w &= !1;
        h &= !1;
    }
    (w, h, w as f32 / p.w as f32)
}

pub fn run(p: &Project, out: &Path, s: &Settings, t_single: f64, assets: &Assets, prog: &Progress) -> Result<(), String> {
    match s.format {
        Format::Mp4 => write_mp4(p, out, s, assets, prog).map_err(|e| format!("MP4 yazılamadı: {e}")),
        Format::Gif => write_gif(p, out, s, assets, prog),
        Format::PngSeq => write_png_seq(p, out, s, assets, prog),
        Format::PngFrame => {
            let mut q = p.clone();
            if s.transparent {
                q.bg = [0.0; 4];
            }
            let (.., sc) = out_size(p, s.scale, false);
            let f = render_project(&q, t_single, sc, assets);
            save_png(&f, out)
        }
    }
}

fn save_png(f: &Frame, out: &Path) -> Result<(), String> {
    image::save_buffer(out, &f.to_rgba8(), f.w as u32, f.h as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())
}

fn frame_count(p: &Project, fps: f32) -> u32 {
    ((p.dur * fps as f64).ceil() as u32).max(1)
}

fn write_png_seq(p: &Project, dir: &Path, s: &Settings, assets: &Assets, prog: &Progress) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut q = p.clone();
    if s.transparent {
        q.bg = [0.0; 4];
    }
    let n = frame_count(p, s.fps);
    let (.., sc) = out_size(p, s.scale, false);
    for i in 0..n {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("İptal edildi".into());
        }
        let f = render_project(&q, i as f64 / s.fps as f64, sc, assets);
        save_png(&f, &dir.join(format!("kare_{i:06}.png")))?;
        prog.done.store((i + 1) * 1000 / n, Ordering::Relaxed);
    }
    Ok(())
}

fn write_gif(p: &Project, out: &Path, s: &Settings, assets: &Assets, prog: &Progress) -> Result<(), String> {
    let (w, h, sc) = out_size(p, s.scale, false);
    if w > 65535 || h > 65535 {
        return Err("GIF boyutu çok büyük".into());
    }
    let file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut enc = gif::Encoder::new(file, w as u16, h as u16, &[]).map_err(|e| e.to_string())?;
    enc.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
    let n = frame_count(p, s.fps);
    let delay = (100.0 / s.fps).round().max(2.0) as u16;
    for i in 0..n {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("İptal edildi".into());
        }
        let f = render_project(p, i as f64 / s.fps as f64, sc, assets);
        let mut rgba = f.to_rgba8();
        let mut g = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut rgba, 10);
        g.delay = delay;
        enc.write_frame(&g).map_err(|e| e.to_string())?;
        prog.done.store((i + 1) * 1000 / n, Ordering::Relaxed);
    }
    Ok(())
}

/// BT.709 limited-range conversion to NV12, flattening any transparency over black.
fn to_nv12(f: &Frame) -> Vec<u8> {
    let (w, h) = (f.w, f.h);
    let mut o = vec![0u8; w * h * 3 / 2];
    let (yp, uv) = o.split_at_mut(w * h);
    yp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, d) in row.iter_mut().enumerate() {
            let p = f.px[y * w + x];
            *d = (16.0 + 219.0 * (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).clamp(0.0, 1.0) + 0.5) as u8;
        }
    });
    uv.par_chunks_mut(w).enumerate().for_each(|(cy, row)| {
        for cx in 0..w / 2 {
            let mut s = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = f.px[(cy * 2 + dy) * w + cx * 2 + dx];
                for c in 0..3 {
                    s[c] += p[c].clamp(0.0, 1.0) * 0.25;
                }
            }
            row[cx * 2] = (128.0 + 224.0 * (-0.1146 * s[0] - 0.3854 * s[1] + 0.5 * s[2]) + 0.5).clamp(0.0, 255.0) as u8;
            row[cx * 2 + 1] = (128.0 + 224.0 * (0.5 * s[0] - 0.4542 * s[1] - 0.0458 * s[2]) + 0.5).clamp(0.0, 255.0) as u8;
        }
    });
    o
}

unsafe fn write_sample(sw: &IMFSinkWriter, stream: u32, data: &[u8], time: i64, dur: i64) -> windows::core::Result<()> {
    unsafe {
        let buf = MFCreateMemoryBuffer(data.len() as u32)?;
        let mut ptr = std::ptr::null_mut();
        buf.Lock(&mut ptr, None, None)?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        buf.Unlock()?;
        buf.SetCurrentLength(data.len() as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buf)?;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(dur)?;
        sw.WriteSample(stream, &sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a real file: set CM_TEST_VIDEO to a video with audio, CM_TEST_OUT to the output.
    #[test]
    #[ignore]
    fn video_with_audio_round_trips() {
        let src = std::env::var("CM_TEST_VIDEO").unwrap();
        let out = std::env::var("CM_TEST_OUT").unwrap();
        let assets = Assets::default();
        let (w, h, dur, _) = assets.video_info(&src).expect("video açılamadı");
        assert!(w > 0 && h > 0 && dur > 0.5, "{w}x{h} {dur}");
        // Decoded frames must differ over time, or seeking/decoding is stuck.
        let (a, b) = (assets.video_frame(&src, 0.2).unwrap(), assets.video_frame(&src, 1.5).unwrap());
        let d: f32 = a.px.iter().zip(b.px.iter()).map(|(x, y)| (x[0] - y[0]).abs()).sum();
        assert!(d > 1.0, "kareler aynı");
        // Going backward must return the earlier frame again.
        let again = assets.video_frame(&src, 0.2).unwrap();
        let back: f32 = a.px.iter().zip(again.px.iter()).map(|(x, y)| (x[0] - y[0]).abs()).sum::<f32>() / a.px.len() as f32;
        assert!(back < 0.05, "geri sarma farkı {back}");
        let clip = assets.audio(&src).expect("ses çözülemedi");
        assert!(clip.iter().map(|v| v.abs()).fold(0.0, f32::max) > 0.05, "ses boş");

        let mut p = Project::new("t", 640, 360, 30.0, [0.0, 0.0, 0.0, 1.0]);
        let mut l = Layer::new(p.alloc_id(), "v", Content::Video { path: src.clone() }, [320.0, 180.0], dur.min(2.0));
        l.tf.scale = Track::new([640.0 / w as f32, 360.0 / h as f32]);
        p.layers.push(l);
        p.fit_duration();
        let s = Settings { format: Format::Mp4, scale: 1.0, fps: 30.0, bitrate: 6.0, audio: true, transparent: false };
        run(&p, Path::new(&out), &s, 0.0, &assets, &Progress::default()).unwrap();

        // The same project through a shared XML, on a "recipient" with only the XML file.
        let dir = Path::new(&out).with_extension("xmltest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let xml = dir.join("paylasim.xml");
        assert_eq!(crate::xml::write(&p, &xml, true).unwrap().embedded, 1);
        let (back, rep) = crate::xml::read(&xml, &dir.join("medya")).unwrap();
        assert!(rep.missing.is_empty());
        let Content::Video { path } = &back.layers[0].content else { panic!() };
        assert_ne!(path, &src, "alıcı kendi kopyasını kullanmalı");
        let fresh = Assets::default();
        assert_eq!(fresh.video_info(path).map(|i| (i.0, i.1)), Some((w, h)));
        assert!(fresh.video_frame(path, 1.0).is_some());
        assert!(fresh.audio(path).is_some_and(|c| !c.is_empty()), "ses de gelmeli");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn has_audio(layers: &[Layer]) -> bool {
    layers.iter().any(|l| match &l.content {
        Content::Audio { .. } | Content::Video { .. } => !l.muted,
        Content::Group { children } => has_audio(children),
        _ => false,
    })
}

fn write_mp4(p: &Project, out: &Path, s: &Settings, assets: &Assets, prog: &Progress) -> windows::core::Result<()> {
    ensure_mf();
    let (w, h, sc) = out_size(p, s.scale, true);
    let size = ((w as u64) << 32) | h as u64;
    // Frame rate as a fraction, keeping NTSC-style rates exact enough.
    let fps_num = (s.fps * 1000.0).round() as u64;
    let rate = (fps_num << 32) | 1000;
    let use_audio = s.audio && has_audio(&p.layers);
    let _ = std::fs::remove_file(out);
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or(windows::core::Error::empty())?;
        attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        let sw = MFCreateSinkWriterFromURL(&HSTRING::from(out.as_os_str()), None, &attrs)?;

        let v_out = MFCreateMediaType()?;
        v_out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        v_out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        v_out.SetUINT32(&MF_MT_AVG_BITRATE, (s.bitrate.max(0.1) * 1e6) as u32)?;
        v_out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        v_out.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        v_out.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
        v_out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        let vs = sw.AddStream(&v_out)?;
        let v_in = MFCreateMediaType()?;
        v_in.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        v_in.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        v_in.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        v_in.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        v_in.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
        v_in.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        v_in.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)?;
        sw.SetInputMediaType(vs, &v_in, None)?;

        let mut audio_stream = None;
        if use_audio {
            let a_out = MFCreateMediaType()?;
            a_out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            a_out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            a_out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            a_out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
            a_out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            a_out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?;
            let a_s = sw.AddStream(&a_out)?;
            let a_in = MFCreateMediaType()?;
            a_in.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            a_in.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            a_in.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            a_in.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
            a_in.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            a_in.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
            a_in.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * 4)?;
            sw.SetInputMediaType(a_s, &a_in, None)?;
            audio_stream = Some(a_s);
        }
        sw.BeginWriting()?;

        let n = frame_count(p, s.fps);
        let frame_hns = (1e7 / s.fps as f64) as i64;
        // Audio clips are decoded up front so that mixing below cannot stall per frame.
        let fetch = |path: &str| -> Option<Arc<Vec<f32>>> { assets.audio(path) };
        let mut audio_written = 0u64;
        for i in 0..n {
            if prog.cancel.load(Ordering::Relaxed) {
                let _ = sw.Finalize();
                let _ = std::fs::remove_file(out);
                return Err(windows::core::Error::new(windows::core::HRESULT(-1), "İptal edildi"));
            }
            let t = i as f64 / s.fps as f64;
            let f = render_project(p, t, sc, assets);
            write_sample(&sw, vs, &to_nv12(&f), (t * 1e7) as i64, frame_hns)?;
            if let Some(a_s) = audio_stream {
                let end = ((i + 1) as f64 / s.fps as f64 * AUDIO_RATE as f64) as u64;
                let frames = (end - audio_written) as usize;
                let mut mix = vec![0.0f32; frames * 2];
                mix_layers(&p.layers, audio_written as f64 / AUDIO_RATE as f64, AUDIO_RATE, &mut mix, &fetch);
                let bytes: Vec<u8> = mix.iter().flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
                let at = (audio_written as f64 / AUDIO_RATE as f64 * 1e7) as i64;
                write_sample(&sw, a_s, &bytes, at, (frames as f64 / AUDIO_RATE as f64 * 1e7) as i64)?;
                audio_written = end;
            }
            prog.done.store((i + 1) * 1000 / n, Ordering::Relaxed);
        }
        sw.Finalize()?;
    }
    Ok(())
}
