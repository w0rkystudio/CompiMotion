//! Mixes a project's audio, for playback and for export.
use crate::media::{AUDIO_RATE, Assets};
use crate::model::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

fn source_path(l: &Layer) -> Option<&str> {
    match &l.content {
        Content::Audio { path } | Content::Video { path } => Some(path),
        _ => None,
    }
}

/// Adds the layers' audio into `out` (stereo interleaved) starting at comp time `t0`.
pub fn mix_layers(layers: &[Layer], t0: f64, rate: u32, out: &mut [f32], fetch: &dyn Fn(&str) -> Option<Arc<Vec<f32>>>) {
    let frames = out.len() / 2;
    let dt = 1.0 / rate as f64;
    for l in layers.iter().filter(|l| !l.muted) {
        if let Content::Group { children } = &l.content {
            // Children's clocks are relative to the group's start.
            if t0 + frames as f64 * dt > l.start && t0 < l.start + l.dur {
                mix_layers(children, t0 - l.start, rate, out, fetch);
            }
            continue;
        }
        let Some(clip) = source_path(l).and_then(fetch) else { continue };
        let total = clip.len() / 2;
        // The volume is evaluated once per block; blocks are far shorter than any fade.
        let vol = l.volume.at((t0 - l.start).max(0.0));
        for i in 0..frames {
            let t = t0 + i as f64 * dt;
            if t < l.start || t >= l.start + l.dur {
                continue;
            }
            let pos = l.source_time(t) * AUDIO_RATE as f64;
            if pos < 0.0 {
                continue;
            }
            let (fi, k) = (pos as usize, pos.fract() as f32);
            if fi + 1 >= total {
                continue;
            }
            for c in 0..2 {
                let (a, b) = (clip[fi * 2 + c], clip[(fi + 1) * 2 + c]);
                out[i * 2 + c] += (a + (b - a) * k) * vol;
            }
        }
    }
}

pub struct PlayShared {
    pub project: Option<Arc<Project>>,
    pub pos: f64,
    pub playing: bool,
}

pub struct Player {
    _stream: Option<cpal::Stream>,
    pub shared: Arc<Mutex<PlayShared>>,
}

impl Player {
    /// Opens the default output device. With no device the app still runs, silently.
    pub fn new(assets: Arc<Assets>) -> Self {
        let shared = Arc::new(Mutex::new(PlayShared { project: None, pos: 0.0, playing: false }));
        let stream = Self::open(assets, shared.clone());
        Player { _stream: stream, shared }
    }

    fn open(assets: Arc<Assets>, shared: Arc<Mutex<PlayShared>>) -> Option<cpal::Stream> {
        let dev = cpal::default_host().default_output_device()?;
        let cfg = dev.default_output_config().ok()?;
        let config: cpal::StreamConfig = cfg.config();
        let (rate, ch) = (config.sample_rate as u32, config.channels as usize);
        let mut stereo: Vec<f32> = vec![];
        let stream = dev
            .build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    data.fill(0.0);
                    let Ok(mut s) = shared.try_lock() else { return };
                    if !s.playing {
                        return;
                    }
                    let Some(p) = s.project.clone() else { return };
                    let frames = data.len() / ch.max(1);
                    stereo.clear();
                    stereo.resize(frames * 2, 0.0);
                    mix_layers(&p.layers, s.pos, rate, &mut stereo, &|path| assets.audio_cached(path));
                    s.pos += frames as f64 / rate as f64;
                    for (i, fr) in data.chunks_mut(ch.max(1)).enumerate() {
                        for (c, o) in fr.iter_mut().enumerate() {
                            *o = stereo[i * 2 + c.min(1)].clamp(-1.0, 1.0);
                        }
                    }
                },
                |_| {},
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_only_inside_layer_time_with_volume() {
        let clip = Arc::new(vec![0.5f32; 48_000 * 2 * 2]);
        let mut l = Layer::new(1, "a", Content::Audio { path: "x".into() }, [0.0, 0.0], 1.0);
        l.start = 0.5;
        l.volume = Track::new(0.5);
        let mut out = vec![0.0; 48_000 * 2];
        mix_layers(&[l], 0.0, 48_000, &mut out, &|_| Some(clip.clone()));
        assert_eq!(out[0], 0.0, "katman başlamadan sessiz");
        assert!((out[48_000 + 1000] - 0.25).abs() < 1e-5, "katman içinde sesli");
    }
}
