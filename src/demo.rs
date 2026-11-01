//! A small sample project that exercises animation, effects, masks and text.
use crate::effects;
use crate::model::*;

fn shape(id: u64, kind: ShapeKind, size: f32, pos: [f32; 2], c: [f32; 4]) -> Layer {
    let sh = ShapeData { kind, size: Track::new([size, size]), radius: Track::new(size * 0.15), sides: Track::new(5.0), inner: Track::new(0.5), path: vec![], closed: true, freehand: false };
    let mut l = Layer::new(id, "Şekil", Content::Shape(sh), pos, 6.0);
    l.fill.a = Track::new(c);
    l
}

fn set(e: &mut EffectInst, key: &str, v: [f32; 4]) {
    e.p.insert(key.into(), Track::new(v));
}

pub fn project() -> Project {
    let mut p = Project::new("Örnek Proje", 1280, 720, 30.0, [0.05, 0.05, 0.09, 1.0]);
    p.dur = 6.0;
    let (cx, cy) = (640.0, 360.0);

    let mut back = shape(1, ShapeKind::Rect, 1400.0, [cx, cy], [1.0; 4]);
    back.name = "Arka Plan".into();
    back.effects.push(effects::instance("four-color-gradient"));
    let mut e = effects::instance("fractal-warp");
    set(&mut e, "amt", [60.0, 0.0, 0.0, 0.0]);
    back.effects.push(e);
    back.opacity = Track::new(0.5);

    let mut star = shape(2, ShapeKind::Star, 300.0, [cx, cy], [1.0, 0.8, 0.25, 1.0]);
    star.name = "Yıldız".into();
    star.tf.scale.set_key(0.0, [0.0, 0.0]);
    star.tf.scale.set_key(1.0, [1.0, 1.0]);
    star.tf.scale.keys[0].ease = Ease::Bezier(0.34, 1.56, 0.64, 1.0);
    star.effects.push(effects::instance("spin"));
    star.effects.push(effects::instance("glow"));

    let mut ring = shape(3, ShapeKind::Ring, 520.0, [cx, cy], [0.5, 0.45, 1.0, 1.0]);
    ring.name = "Halka".into();
    ring.tf.rot_y.set_key(0.0, -60.0);
    ring.tf.rot_y.set_key(6.0, 60.0);
    ring.effects.push(effects::instance("edge-glow"));

    let tx = TextData { text: "CompiMotion".into(), font: "segoeui.ttf".into(), size: Track::new(110.0), align: Align::Center, tracking: Track::new(2.0), leading: Track::new(0.0) };
    let mut text = Layer::new(4, "Başlık", Content::Text(tx), [cx, 590.0], 6.0);
    let mut anim = effects::instance("text-transform");
    let mut prog = Track::new([0.0, 0.0, 0.0, 0.0]);
    prog.set_key(0.3, [0.0, 0.0, 0.0, 0.0]);
    prog.set_key(1.8, [1.0, 0.0, 0.0, 0.0]);
    anim.p.insert("prog".into(), prog);
    text.effects.push(anim);
    text.shadow = Some(Shadow { color: Track::new([0.0, 0.0, 0.0, 0.7]), angle: Track::new(60.0), dist: Track::new(8.0), blur: Track::new(10.0) });

    p.layers = vec![back, ring, star, text];
    p.next_id = 5;
    p
}
