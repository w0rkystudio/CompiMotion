//! Keyframe clipboard: copy one key, one property or a whole layer, and paste at the playhead.
use crate::app::Editor;
use crate::effects;
use crate::model::*;
use serde::{Deserialize, Serialize};

const CLIP_PREFIX: &str = "CompiMotionKeyframes:";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackClip {
    pub name: String,
    pub keys: Vec<KeyCopy>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeyClip {
    pub tracks: Vec<TrackClip>,
}

impl KeyClip {
    pub fn key_count(&self) -> usize {
        self.tracks.iter().map(|t| t.keys.len()).sum()
    }
    /// Text form for the system clipboard, so keys also travel between open CompiMotion windows.
    pub fn to_text(&self) -> String {
        format!("{CLIP_PREFIX}{}", serde_json::to_string(self).unwrap_or_default())
    }
    pub fn from_text(s: &str) -> Option<KeyClip> {
        serde_json::from_str(s.strip_prefix(CLIP_PREFIX)?).ok()
    }
}

/// Effect tracks are named "effect-id#index/param".
fn split_effect_name(name: &str) -> Option<(&str, usize, &str)> {
    let (head, param) = name.split_once('/')?;
    let (def, idx) = head.split_once('#')?;
    Some((def, idx.parse().ok()?, param))
}

/// Makes the target track exist when it is an effect parameter nobody has touched yet.
/// Returns the track's name on this layer, which may use a different effect index.
fn resolve_track_name(l: &mut Layer, name: &str) -> String {
    let Some((def_id, idx, param)) = split_effect_name(name) else { return name.to_string() };
    let at = if l.effects.get(idx).is_some_and(|e| e.def == def_id) { Some(idx) } else { l.effects.iter().position(|e| e.def == def_id) };
    let Some(i) = at else { return name.to_string() };
    if let Some(q) = effects::find(def_id).and_then(|d| d.params.iter().find(|q| q.key == param)) {
        effects::track_mut(&mut l.effects[i], q);
    }
    format!("{def_id}#{i}/{param}")
}

fn write_track(l: &mut Layer, name: &str, keys: &[KeyCopy]) -> usize {
    let mut written = 0;
    l.visit_tracks(&mut |n, tr| {
        if n == name {
            for k in keys {
                if tr.write_key(k) {
                    written += 1;
                }
            }
        }
    });
    written
}

/// Pastes so the clip's earliest key lands on `at` (layer-local seconds).
/// Returns (keys written, keys that had no matching property here).
pub fn paste_into_layer(l: &mut Layer, clip: &KeyClip, at: f64, only_track: Option<&str>) -> (usize, usize) {
    let first = clip.tracks.iter().flat_map(|t| t.keys.iter().map(|k| k.t)).fold(f64::MAX, f64::min);
    let (mut done, mut skipped) = (0, 0);
    for tr in &clip.tracks {
        let keys: Vec<KeyCopy> = tr.keys.iter().map(|k| KeyCopy { t: (at + k.t - first).max(0.0), ..*k }).collect();
        let target = match only_track {
            Some(n) => n.to_string(),
            None => resolve_track_name(l, &tr.name),
        };
        let n = write_track(l, &target, &keys);
        done += n;
        skipped += keys.len() - n;
    }
    (done, skipped)
}

impl Editor {
    fn read_track(&mut self, id: u64, name: &str) -> Vec<KeyCopy> {
        let mut out = vec![];
        if let Some(l) = self.p.find_mut(id) {
            l.visit_tracks(&mut |n, tr| {
                if n == name {
                    out = tr.read_keys();
                }
            });
        }
        out
    }

    fn set_clip(&mut self, clip: KeyClip, ctx: &eframe::egui::Context) {
        if clip.key_count() == 0 {
            self.status = Some("Kopyalanacak keyframe yok.".into());
            return;
        }
        self.status = Some(format!("{} keyframe kopyalandı.", clip.key_count()));
        ctx.copy_text(clip.to_text());
        self.key_clip = Some(clip);
    }

    pub fn copy_key(&mut self, id: u64, name: &str, k: usize, ctx: &eframe::egui::Context) {
        let keys: Vec<KeyCopy> = self.read_track(id, name).get(k).copied().into_iter().collect();
        self.set_clip(KeyClip { tracks: vec![TrackClip { name: name.to_string(), keys }] }, ctx);
    }

    pub fn copy_track(&mut self, id: u64, name: &str, ctx: &eframe::egui::Context) {
        let keys = self.read_track(id, name);
        self.set_clip(KeyClip { tracks: vec![TrackClip { name: name.to_string(), keys }] }, ctx);
    }

    pub fn copy_layer_keys(&mut self, id: u64, ctx: &eframe::egui::Context) {
        let mut tracks = vec![];
        if let Some(l) = self.p.find_mut(id) {
            l.visit_tracks(&mut |n, tr| {
                let keys = tr.read_keys();
                if !keys.is_empty() {
                    tracks.push(TrackClip { name: n.to_string(), keys });
                }
            });
        }
        self.set_clip(KeyClip { tracks }, ctx);
    }

    /// Ctrl+C: the keyframe picked on the timeline if there is one, otherwise the whole layer.
    pub fn copy_shortcut(&mut self, ctx: &eframe::egui::Context) {
        let Some(id) = self.selected() else { return };
        if self.sel_keys.iter().any(|s| self.sel.contains(&s.0)) {
            self.copy_selected_keys(ctx);
        } else {
            self.copy_layer_keys(id, ctx);
        }
    }

    pub fn copy_selected_keys(&mut self, ctx: &eframe::egui::Context) {
        let mut tracks: Vec<TrackClip> = vec![];
        for (id, name, k) in self.sel_keys.clone() {
            let Some(key) = self.read_track(id, &name).get(k).copied() else { continue };
            match tracks.iter_mut().find(|t| t.name == name) {
                Some(t) => t.keys.push(key),
                None => tracks.push(TrackClip { name, keys: vec![key] }),
            }
        }
        self.set_clip(KeyClip { tracks }, ctx);
    }

    /// Pastes onto all selected layers at the playhead.
    pub fn paste_keys(&mut self, external: Option<KeyClip>) {
        if let Some(c) = external {
            self.key_clip = Some(c);
        }
        let Some(clip) = self.key_clip.clone() else { return };
        let t = self.t;
        let (mut done, mut skipped) = (0, 0);
        for id in self.sel.clone() {
            if let Some(l) = self.p.find_mut(id) {
                let at = l.local(t).max(0.0);
                let (a, b) = paste_into_layer(l, &clip, at, None);
                done += a;
                skipped += b;
            }
        }
        self.report_paste(done, skipped);
    }

    /// Pastes a one-property clip onto a chosen property, which may be a different one of the same type.
    pub fn paste_keys_to_track(&mut self, id: u64, name: &str) {
        let Some(clip) = self.key_clip.clone() else { return };
        let t = self.t;
        let Some(l) = self.p.find_mut(id) else { return };
        let at = l.local(t).max(0.0);
        let (done, skipped) = paste_into_layer(l, &clip, at, Some(name));
        self.report_paste(done, skipped);
    }

    fn report_paste(&mut self, done: usize, skipped: usize) {
        self.status = Some(if skipped > 0 {
            format!("{done} keyframe yapıştırıldı, {skipped} tanesinin bu katmanda karşılığı yok.")
        } else {
            format!("{done} keyframe yapıştırıldı.")
        });
        if done > 0 {
            self.touch();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(id: u64) -> Layer {
        Layer::new(id, "l", Content::Null, [0.0, 0.0], 10.0)
    }
    fn source() -> Layer {
        let mut l = layer(1);
        l.tf.rot.set_key(1.0, 10.0);
        l.tf.rot.set_key(2.0, 90.0);
        l.tf.rot.keys[0].ease = Ease::Bounce;
        l.tf.pos.set_key(3.0, [5.0, 6.0]);
        l
    }
    fn all_keys(l: &mut Layer) -> KeyClip {
        let mut tracks = vec![];
        l.visit_tracks(&mut |n, tr| {
            let keys = tr.read_keys();
            if !keys.is_empty() {
                tracks.push(TrackClip { name: n.into(), keys });
            }
        });
        KeyClip { tracks }
    }

    #[test]
    fn paste_keeps_spacing_values_and_easing_from_the_playhead() {
        let clip = all_keys(&mut source());
        let mut to = layer(2);
        assert_eq!(paste_into_layer(&mut to, &clip, 4.0, None), (3, 0));
        // Earliest source key was at 1 s, so it lands exactly on 4 s.
        assert_eq!(to.tf.rot.times(), vec![4.0, 5.0]);
        assert_eq!(to.tf.rot.keys[0].v, 10.0);
        assert_eq!(to.tf.rot.keys[0].ease, Ease::Bounce);
        assert_eq!(to.tf.pos.times(), vec![6.0]);
    }

    #[test]
    fn pasting_again_replaces_keys_at_the_same_time() {
        let clip = all_keys(&mut source());
        let mut to = layer(2);
        paste_into_layer(&mut to, &clip, 0.0, None);
        paste_into_layer(&mut to, &clip, 0.0, None);
        assert_eq!(to.tf.rot.keys.len(), 2);
    }

    #[test]
    fn wrong_value_type_is_skipped_not_corrupted() {
        // A position key (two numbers) cannot go on rotation (one number).
        let clip = KeyClip { tracks: vec![TrackClip { name: "Konum".into(), keys: vec![KeyCopy { t: 0.0, v: KeyVal::V2([1.0, 2.0]), ease: Ease::Linear }] }] };
        let mut to = layer(2);
        assert_eq!(paste_into_layer(&mut to, &clip, 1.0, Some("Dönme")), (0, 1));
        assert!(to.tf.rot.keys.is_empty());
    }

    #[test]
    fn effect_keys_create_untouched_params_and_follow_a_different_index() {
        let mut from = layer(1);
        from.effects.push(effects::instance("invert"));
        from.effects.push(effects::instance("gaussian-blur"));
        let mut tr = Track::new([0.0, 0.0, 0.0, 0.0]);
        tr.set_key(0.5, [40.0, 0.0, 0.0, 0.0]);
        from.effects[1].p.insert("str".into(), tr);
        let clip = all_keys(&mut from);

        // On the target the same effect sits at index 0, and "str" has never been edited.
        let mut to = layer(2);
        to.effects.push(effects::instance("gaussian-blur"));
        assert_eq!(paste_into_layer(&mut to, &clip, 2.0, None), (1, 0));
        assert_eq!(to.effects[0].p["str"].keys[0].t, 2.0);

        // Without that effect the key is reported as skipped.
        let mut none = layer(3);
        assert_eq!(paste_into_layer(&mut none, &clip, 2.0, None), (0, 1));
    }

    #[test]
    fn clipboard_text_round_trips_and_ignores_foreign_text() {
        let clip = all_keys(&mut source());
        let back = KeyClip::from_text(&clip.to_text()).expect("metin geri okunmalı");
        assert_eq!(back.key_count(), 3);
        assert!(KeyClip::from_text("merhaba").is_none());
        assert!(KeyClip::from_text("CompiMotionKeyframes:{bozuk").is_none());
    }
}
