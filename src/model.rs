//! Project data model: projects, layers, animated properties and easing.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Rgba = [f32; 4];

/// Cubic-bezier easing between two keyframes (x1, y1, x2, y2), or a hold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Ease {
    Linear,
    Hold,
    Bezier(f32, f32, f32, f32),
    Bounce,
    Elastic,
}

impl Ease {
    pub const PRESETS: &'static [(&'static str, Ease)] = &[
        ("Doğrusal", Ease::Linear),
        ("Sabit", Ease::Hold),
        ("Yumuşak", Ease::Bezier(0.42, 0.0, 0.58, 1.0)),
        ("Yumuşak giriş", Ease::Bezier(0.42, 0.0, 1.0, 1.0)),
        ("Yumuşak çıkış", Ease::Bezier(0.0, 0.0, 0.58, 1.0)),
        ("Güçlü", Ease::Bezier(0.85, 0.0, 0.15, 1.0)),
        ("Geri sekme", Ease::Bezier(0.34, 1.56, 0.64, 1.0)),
        ("Zıplama", Ease::Bounce),
        ("Elastik", Ease::Elastic),
    ];

    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            Ease::Hold => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Ease::Bezier(x1, y1, x2, y2) => bezier(t, x1, y1, x2, y2),
            Ease::Bounce => {
                let (n, d) = (7.5625, 2.75);
                if t < 1.0 / d {
                    n * t * t
                } else if t < 2.0 / d {
                    let t = t - 1.5 / d;
                    n * t * t + 0.75
                } else if t < 2.5 / d {
                    let t = t - 2.25 / d;
                    n * t * t + 0.9375
                } else {
                    let t = t - 2.625 / d;
                    n * t * t + 0.984375
                }
            }
            Ease::Elastic => {
                if t <= 0.0 || t >= 1.0 {
                    t
                } else {
                    2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * std::f32::consts::TAU / 3.0).sin() + 1.0
                }
            }
        }
    }
}

fn bezier(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let curve = |a: f32, b: f32, s: f32| {
        let m = 1.0 - s;
        3.0 * m * m * s * a + 3.0 * m * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if curve(x1, x2, mid) < x {
            lo = mid
        } else {
            hi = mid
        }
    }
    curve(y1, y2, (lo + hi) * 0.5)
}

/// A keyframe value with its type erased, for copying between tracks.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum KeyVal {
    F(f32),
    V2([f32; 2]),
    V4([f32; 4]),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyCopy {
    pub t: f64,
    pub v: KeyVal,
    pub ease: Ease,
}

pub trait Lerp: Clone {
    fn lerp(&self, other: &Self, t: f32) -> Self;
    fn to_val(&self) -> KeyVal;
    fn from_val(v: &KeyVal) -> Option<Self>;
}
impl Lerp for f32 {
    fn lerp(&self, o: &f32, t: f32) -> f32 {
        self + (o - self) * t
    }
    fn to_val(&self) -> KeyVal {
        KeyVal::F(*self)
    }
    fn from_val(v: &KeyVal) -> Option<Self> {
        if let KeyVal::F(x) = v { Some(*x) } else { None }
    }
}
impl Lerp for [f32; 2] {
    fn lerp(&self, o: &Self, t: f32) -> Self {
        [self[0].lerp(&o[0], t), self[1].lerp(&o[1], t)]
    }
    fn to_val(&self) -> KeyVal {
        KeyVal::V2(*self)
    }
    fn from_val(v: &KeyVal) -> Option<Self> {
        if let KeyVal::V2(x) = v { Some(*x) } else { None }
    }
}
impl Lerp for [f32; 4] {
    fn lerp(&self, o: &Self, t: f32) -> Self {
        std::array::from_fn(|i| self[i].lerp(&o[i], t))
    }
    fn to_val(&self) -> KeyVal {
        KeyVal::V4(*self)
    }
    fn from_val(v: &KeyVal) -> Option<Self> {
        if let KeyVal::V4(x) = v { Some(*x) } else { None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Key<T> {
    pub t: f64,
    pub v: T,
    pub ease: Ease,
}

/// A value that is constant, or animated once it has keyframes. `t` is seconds from layer start.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track<T> {
    pub base: T,
    pub keys: Vec<Key<T>>,
}

impl<T: Lerp> Track<T> {
    pub fn new(v: T) -> Self {
        Track { base: v, keys: vec![] }
    }
    pub fn at(&self, t: f64) -> T {
        let k = &self.keys;
        if k.is_empty() {
            return self.base.clone();
        }
        if t <= k[0].t {
            return k[0].v.clone();
        }
        for w in k.windows(2) {
            if t < w[1].t {
                let f = ((t - w[0].t) / (w[1].t - w[0].t).max(1e-9)) as f32;
                return w[0].v.lerp(&w[1].v, w[0].ease.apply(f));
            }
        }
        k[k.len() - 1].v.clone()
    }
    pub fn key_index(&self, t: f64) -> Option<usize> {
        self.keys.iter().position(|k| (k.t - t).abs() < 1e-3)
    }
    /// Sets the value at `t`: moves the key there when animated, otherwise changes the base.
    pub fn set(&mut self, t: f64, v: T) {
        if self.keys.is_empty() {
            self.base = v;
        } else {
            self.set_key(t, v);
        }
    }
    pub fn set_key(&mut self, t: f64, v: T) {
        if let Some(i) = self.key_index(t) {
            self.keys[i].v = v;
            return;
        }
        let i = self.keys.partition_point(|k| k.t < t);
        let ease = if i > 0 { self.keys[i - 1].ease } else { Ease::Bezier(0.42, 0.0, 0.58, 1.0) };
        self.keys.insert(i, Key { t, v, ease });
    }
    pub fn toggle_key(&mut self, t: f64) {
        if let Some(i) = self.key_index(t) {
            self.base = self.keys[i].v.clone();
            self.keys.remove(i);
        } else {
            let v = self.at(t);
            self.set_key(t, v);
        }
    }
}

/// Type-erased keyframe access, for the timeline and for splitting layers.
pub trait AnyTrack {
    fn times(&self) -> Vec<f64>;
    fn ease(&self, i: usize) -> Option<Ease>;
    fn set_ease(&mut self, i: usize, e: Ease);
    /// Moves key `i`, keeping keys sorted; returns its new index.
    fn move_key(&mut self, i: usize, t: f64) -> usize;
    fn remove_key(&mut self, i: usize);
    fn shift(&mut self, dt: f64);
    /// Moves several keys by `dt` at once; returns their new indices, in the given order.
    fn move_keys(&mut self, idx: &[usize], dt: f64) -> Vec<usize>;
    fn read_keys(&self) -> Vec<KeyCopy>;
    /// Adds or replaces a key; false when the value's type does not fit this track.
    fn write_key(&mut self, k: &KeyCopy) -> bool;
    fn sort_keys(&mut self);
}

impl<T: Lerp> AnyTrack for Track<T> {
    fn times(&self) -> Vec<f64> {
        self.keys.iter().map(|k| k.t).collect()
    }
    fn ease(&self, i: usize) -> Option<Ease> {
        self.keys.get(i).map(|k| k.ease)
    }
    fn set_ease(&mut self, i: usize, e: Ease) {
        if let Some(k) = self.keys.get_mut(i) {
            k.ease = e;
        }
    }
    fn move_key(&mut self, i: usize, t: f64) -> usize {
        if i >= self.keys.len() {
            return i;
        }
        let mut k = self.keys.remove(i);
        k.t = t.max(0.0);
        let at = self.keys.partition_point(|q| q.t < k.t);
        self.keys.insert(at, k);
        at
    }
    fn remove_key(&mut self, i: usize) {
        if i < self.keys.len() {
            let k = self.keys.remove(i);
            if self.keys.is_empty() {
                self.base = k.v;
            }
        }
    }
    fn shift(&mut self, dt: f64) {
        for k in &mut self.keys {
            k.t += dt;
        }
    }
    fn move_keys(&mut self, idx: &[usize], dt: f64) -> Vec<usize> {
        // Tag each key with whether it moves and where it sat, then re-sort stably.
        let mut tagged: Vec<(Option<usize>, Key<T>)> = self.keys.drain(..).enumerate().map(|(i, k)| (idx.iter().position(|q| *q == i), k)).collect();
        for (mark, k) in &mut tagged {
            if mark.is_some() {
                k.t += dt;
            }
        }
        tagged.sort_by(|a, b| a.1.t.total_cmp(&b.1.t));
        let mut out = vec![0; idx.len()];
        for (i, (mark, k)) in tagged.into_iter().enumerate() {
            if let Some(m) = mark {
                out[m] = i;
            }
            self.keys.push(k);
        }
        out
    }
    fn read_keys(&self) -> Vec<KeyCopy> {
        self.keys.iter().map(|k| KeyCopy { t: k.t, v: k.v.to_val(), ease: k.ease }).collect()
    }
    fn write_key(&mut self, k: &KeyCopy) -> bool {
        let Some(v) = T::from_val(&k.v) else { return false };
        self.set_key(k.t, v);
        if let Some(i) = self.key_index(k.t) {
            self.keys[i].ease = k.ease;
        }
        true
    }
    fn sort_keys(&mut self) {
        self.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blend {
    Normal, Add, Subtract, Multiply, Screen, Overlay, Darken, Lighten, ColorDodge, ColorBurn,
    LinearBurn, HardLight, SoftLight, VividLight, LinearLight, PinLight, HardMix, Difference,
    Exclusion, Divide, Hue, Saturation, Color, Luminosity, DarkerColor, LighterColor,
    // Compositing operators
    AlphaIn, AlphaOut, Atop, Behind, Xor,
}

impl Blend {
    pub const ALL: &'static [(Blend, &'static str)] = &[
        (Blend::Normal, "Normal"), (Blend::Add, "Ekle"), (Blend::Subtract, "Çıkar"),
        (Blend::Multiply, "Çarp"), (Blend::Screen, "Ekran"), (Blend::Overlay, "Kaplama"),
        (Blend::Darken, "Koyulaştır"), (Blend::Lighten, "Açıklaştır"),
        (Blend::ColorDodge, "Renk Soldur"), (Blend::ColorBurn, "Renk Yak"),
        (Blend::LinearBurn, "Doğrusal Yak"), (Blend::HardLight, "Sert Işık"),
        (Blend::SoftLight, "Yumuşak Işık"), (Blend::VividLight, "Canlı Işık"),
        (Blend::LinearLight, "Doğrusal Işık"), (Blend::PinLight, "Nokta Işık"),
        (Blend::HardMix, "Sert Karışım"), (Blend::Difference, "Fark"),
        (Blend::Exclusion, "Dışlama"), (Blend::Divide, "Böl"), (Blend::Hue, "Ton"),
        (Blend::Saturation, "Doygunluk"), (Blend::Color, "Renk"),
        (Blend::Luminosity, "Parlaklık"), (Blend::DarkerColor, "Koyu Renk"),
        (Blend::LighterColor, "Açık Renk"), (Blend::AlphaIn, "Maske: İçinde"),
        (Blend::AlphaOut, "Maske: Dışında"), (Blend::Atop, "Üstünde"),
        (Blend::Behind, "Arkasında"), (Blend::Xor, "Özel VEYA"),
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rect, RoundRect, Ellipse, Triangle, Polygon, Star, Ring, Heart, Arrow, Cross, Crescent, Teardrop, Line, Path,
}

impl ShapeKind {
    pub const ALL: &'static [(ShapeKind, &'static str)] = &[
        (ShapeKind::Rect, "Dikdörtgen"), (ShapeKind::RoundRect, "Yuvarlak Köşe"),
        (ShapeKind::Ellipse, "Elips"), (ShapeKind::Triangle, "Üçgen"),
        (ShapeKind::Polygon, "Çokgen"), (ShapeKind::Star, "Yıldız"), (ShapeKind::Ring, "Halka"),
        (ShapeKind::Heart, "Kalp"), (ShapeKind::Arrow, "Ok"), (ShapeKind::Cross, "Artı"),
        (ShapeKind::Crescent, "Hilal"), (ShapeKind::Teardrop, "Damla"), (ShapeKind::Line, "Çizgi"),
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillKind {
    None,
    Solid,
    Linear,
    Radial,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fill {
    pub kind: FillKind,
    pub a: Track<Rgba>,
    pub b: Track<Rgba>,
    pub angle: Track<f32>,
}

/// One node of an editable bezier path, in layer-local pixels.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PathPt {
    pub p: [f32; 2],
    pub h_in: [f32; 2],
    pub h_out: [f32; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShapeData {
    pub kind: ShapeKind,
    pub size: Track<[f32; 2]>,
    pub radius: Track<f32>,
    pub sides: Track<f32>,
    pub inner: Track<f32>,
    pub path: Vec<PathPt>,
    pub closed: bool,
    /// Pressure-free freehand strokes: smoothed with round caps.
    pub freehand: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextData {
    pub text: String,
    pub font: String,
    pub size: Track<f32>,
    pub align: Align,
    pub tracking: Track<f32>,
    pub leading: Track<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Content {
    Shape(ShapeData),
    Text(TextData),
    Image { path: String },
    Video { path: String },
    Audio { path: String },
    /// Children are composited together and then treated as one layer.
    Group { children: Vec<Layer> },
    /// Invisible; only offers a transform to parent others to.
    Null,
    /// Views the 3D scene: its position/rotation drive perspective.
    Camera { fov: Track<f32> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskMode {
    None,
    /// This layer's alpha cuts the layers beneath it in the same stack; it is not drawn.
    Mask,
    InverseMask,
    /// Like Mask, using luminance.
    LumaMask,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transform {
    pub pos: Track<[f32; 2]>,
    pub z: Track<f32>,
    pub scale: Track<[f32; 2]>,
    pub rot: Track<f32>,
    pub rot_x: Track<f32>,
    pub rot_y: Track<f32>,
    pub anchor: Track<[f32; 2]>,
    pub skew: Track<[f32; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectInst {
    pub def: String,
    pub on: bool,
    #[serde(default)]
    pub p: BTreeMap<String, Track<[f32; 4]>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub content: Content,
    pub start: f64,
    pub dur: f64,
    /// Seconds trimmed from the source's beginning.
    pub in_point: f64,
    pub speed: f32,
    pub volume: Track<f32>,
    pub muted: bool,
    pub visible: bool,
    pub locked: bool,
    pub tf: Transform,
    pub opacity: Track<f32>,
    pub blend: Blend,
    pub mask: MaskMode,
    pub parent: Option<u64>,
    pub fill: Fill,
    pub border: Option<Border>,
    pub shadow: Option<Shadow>,
    pub motion_blur: bool,
    pub three_d: bool,
    pub effects: Vec<EffectInst>,
    pub color_tag: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Border {
    pub color: Track<Rgba>,
    pub width: Track<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shadow {
    pub color: Track<Rgba>,
    pub angle: Track<f32>,
    pub dist: Track<f32>,
    pub blur: Track<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub fps: f32,
    pub bg: Rgba,
    pub dur: f64,
    pub layers: Vec<Layer>,
    pub next_id: u64,
}

impl Project {
    pub fn new(name: &str, w: u32, h: u32, fps: f32, bg: Rgba) -> Self {
        Project { name: name.into(), w, h, fps, bg, dur: 10.0, layers: vec![], next_id: 1 }
    }
    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }
    /// Longest layer end, or the stored length when no layer exists.
    pub fn fit_duration(&mut self) {
        let end = self.layers.iter().map(|l| l.start + l.dur).fold(0.0, f64::max);
        if end > 0.0 {
            self.dur = end;
        }
    }
    pub fn find(&self, id: u64) -> Option<&Layer> {
        fn go(v: &[Layer], id: u64) -> Option<&Layer> {
            v.iter().find_map(|l| {
                if l.id == id {
                    return Some(l);
                }
                if let Content::Group { children } = &l.content { go(children, id) } else { None }
            })
        }
        go(&self.layers, id)
    }
    pub fn find_mut(&mut self, id: u64) -> Option<&mut Layer> {
        fn go(v: &mut [Layer], id: u64) -> Option<&mut Layer> {
            v.iter_mut().find_map(|l| {
                if l.id == id {
                    return Some(l);
                }
                if let Content::Group { children } = &mut l.content { go(children, id) } else { None }
            })
        }
        go(&mut self.layers, id)
    }
}

impl Layer {
    pub fn new(id: u64, name: &str, content: Content, center: [f32; 2], dur: f64) -> Self {
        Layer {
            id,
            name: name.into(),
            content,
            start: 0.0,
            dur,
            in_point: 0.0,
            speed: 1.0,
            volume: Track::new(1.0),
            muted: false,
            visible: true,
            locked: false,
            tf: Transform {
                pos: Track::new(center),
                z: Track::new(0.0),
                scale: Track::new([1.0, 1.0]),
                rot: Track::new(0.0),
                rot_x: Track::new(0.0),
                rot_y: Track::new(0.0),
                anchor: Track::new([0.0, 0.0]),
                skew: Track::new([0.0, 0.0]),
            },
            opacity: Track::new(1.0),
            blend: Blend::Normal,
            mask: MaskMode::None,
            parent: None,
            fill: Fill {
                kind: FillKind::Solid,
                a: Track::new([1.0, 1.0, 1.0, 1.0]),
                b: Track::new([0.2, 0.5, 1.0, 1.0]),
                angle: Track::new(90.0),
            },
            border: None,
            shadow: None,
            motion_blur: false,
            three_d: false,
            effects: vec![],
            color_tag: (id % 6) as u8,
        }
    }
    /// Calls `f` with a display name and every animatable track of the layer.
    pub fn visit_tracks(&mut self, f: &mut dyn FnMut(&str, &mut dyn AnyTrack)) {
        f("Konum", &mut self.tf.pos);
        f("Z", &mut self.tf.z);
        f("Ölçek", &mut self.tf.scale);
        f("Dönme", &mut self.tf.rot);
        f("X dönmesi", &mut self.tf.rot_x);
        f("Y dönmesi", &mut self.tf.rot_y);
        f("Çapa", &mut self.tf.anchor);
        f("Eğim", &mut self.tf.skew);
        f("Opaklık", &mut self.opacity);
        f("Ses", &mut self.volume);
        f("Renk 1", &mut self.fill.a);
        f("Renk 2", &mut self.fill.b);
        f("Gradyan açısı", &mut self.fill.angle);
        if let Some(b) = &mut self.border {
            f("Kenar rengi", &mut b.color);
            f("Kenar kalınlığı", &mut b.width);
        }
        if let Some(s) = &mut self.shadow {
            f("Gölge rengi", &mut s.color);
            f("Gölge açısı", &mut s.angle);
            f("Gölge uzaklığı", &mut s.dist);
            f("Gölge bulanıklığı", &mut s.blur);
        }
        match &mut self.content {
            Content::Shape(s) => {
                f("Boyut", &mut s.size);
                f("Köşe yarıçapı", &mut s.radius);
                f("Köşe sayısı", &mut s.sides);
                f("İç oran", &mut s.inner);
            }
            Content::Text(s) => {
                f("Yazı boyutu", &mut s.size);
                f("Harf aralığı", &mut s.tracking);
                f("Satır aralığı", &mut s.leading);
            }
            Content::Camera { fov } => f("Görüş açısı", fov),
            _ => {}
        }
        for (i, e) in self.effects.iter_mut().enumerate() {
            for (k, tr) in e.p.iter_mut() {
                f(&format!("{}#{i}/{k}", e.def), tr);
            }
        }
    }
    /// Seconds into the layer's own animation for comp time `t`.
    pub fn local(&self, t: f64) -> f64 {
        t - self.start
    }
    pub fn source_time(&self, t: f64) -> f64 {
        self.in_point + (t - self.start) * self.speed as f64
    }
    pub fn active(&self, t: f64) -> bool {
        self.visible && t >= self.start && t < self.start + self.dur
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_interpolates_and_holds_ends() {
        let mut tr = Track::new(0.0f32);
        tr.keys.push(Key { t: 1.0, v: 10.0, ease: Ease::Linear });
        tr.keys.push(Key { t: 3.0, v: 20.0, ease: Ease::Linear });
        assert_eq!(tr.at(0.0), 10.0);
        assert_eq!(tr.at(2.0), 15.0);
        assert_eq!(tr.at(9.0), 20.0);
    }

    #[test]
    fn eases_hit_their_endpoints() {
        for (_, e) in Ease::PRESETS {
            assert!(e.apply(0.0).abs() < 1e-3, "{e:?}");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-3, "{e:?}");
        }
    }

    #[test]
    fn toggling_a_key_twice_restores_static_value() {
        let mut tr = Track::new(5.0f32);
        tr.toggle_key(1.0);
        assert_eq!(tr.keys.len(), 1);
        tr.toggle_key(1.0);
        assert!(tr.keys.is_empty());
        assert_eq!(tr.at(0.0), 5.0);
    }
}
