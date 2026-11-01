//! Selection-wide edits: moving layers between groups, cutting clips, multi-keyframe changes.
use crate::app::{Editor, clear_parents};
use crate::model::*;
use std::collections::HashSet;

/// Where moved layers should end up.
#[derive(Clone, Copy, Debug)]
pub enum Dest {
    /// On top of this group's children.
    Group(u64),
    /// Directly above this layer, in its stack.
    Above(u64),
}

/// Project-time start of a layer, adding up the groups it sits in.
pub fn abs_start(v: &[Layer], id: u64, base: f64) -> Option<f64> {
    v.iter().find_map(|l| {
        if l.id == id {
            return Some(base + l.start);
        }
        if let Content::Group { children } = &l.content { abs_start(children, id, base + l.start) } else { None }
    })
}

/// `Some(None)` for a top-level layer, `Some(Some(g))` inside group `g`, `None` if not found.
pub fn parent_of(v: &[Layer], id: u64) -> Option<Option<u64>> {
    fn go(v: &[Layer], id: u64, parent: Option<u64>) -> Option<Option<u64>> {
        v.iter().find_map(|l| {
            if l.id == id {
                return Some(parent);
            }
            if let Content::Group { children } = &l.content { go(children, id, Some(l.id)) } else { None }
        })
    }
    go(v, id, None)
}

fn subtree_has(l: &Layer, id: u64) -> bool {
    l.id == id || matches!(&l.content, Content::Group { children } if children.iter().any(|c| subtree_has(c, id)))
}

fn remove_ids(v: &mut Vec<Layer>, ids: &HashSet<u64>, out: &mut Vec<Layer>) {
    let mut i = 0;
    while i < v.len() {
        if ids.contains(&v[i].id) {
            out.push(v.remove(i));
        } else {
            if let Content::Group { children } = &mut v[i].content {
                remove_ids(children, ids, out);
            }
            i += 1;
        }
    }
}

/// Makes a group's clip cover all its children, without moving anything on the timeline.
fn refit(g: &mut Layer) {
    let Content::Group { children } = &mut g.content else { return };
    if children.is_empty() {
        return;
    }
    let min = children.iter().map(|c| c.start).fold(f64::MAX, f64::min);
    let mut shift = 0.0;
    if min < 0.0 {
        shift = min;
        for c in children.iter_mut() {
            c.start -= min;
        }
    }
    let end = children.iter().map(|c| c.start + c.dur).fold(0.0, f64::max);
    if shift != 0.0 {
        g.start += shift;
        g.dur -= shift;
        // The group's own animation stays where it was on the timeline.
        g.visit_tracks(&mut |_, tr| tr.shift(-shift));
    }
    g.dur = g.dur.max(end);
}

fn refit_chain(v: &mut [Layer], gid: u64) -> bool {
    for l in v {
        if let Content::Group { children } = &mut l.content {
            if l.id == gid || refit_chain(children, gid) {
                refit(l);
                return true;
            }
        }
    }
    false
}

fn insert_above(v: &mut Vec<Layer>, to: u64, new: &mut Vec<Layer>) -> bool {
    if let Some(i) = v.iter().position(|l| l.id == to) {
        for (k, l) in new.drain(..).enumerate() {
            v.insert(i + 1 + k, l);
        }
        return true;
    }
    v.iter_mut().any(|l| if let Content::Group { children } = &mut l.content { insert_above(children, to, new) } else { false })
}

impl Editor {
    /// Selected layers, leaving out any whose group is selected too: they move with it.
    pub fn top_selection(&self) -> Vec<u64> {
        self.without_nested(&self.sel)
    }

    fn without_nested(&self, ids: &[u64]) -> Vec<u64> {
        ids.iter()
            .copied()
            .filter(|id| {
                let mut cur = parent_of(&self.p.layers, *id).flatten();
                while let Some(g) = cur {
                    if ids.contains(&g) {
                        return false;
                    }
                    cur = parent_of(&self.p.layers, g).flatten();
                }
                true
            })
            .collect()
    }

    /// Moves layers to another stack, keeping each one at the same time on the timeline.
    /// Returns how many moved. Refuses cameras (they only work at the top level) and any
    /// move that would put a group inside itself.
    pub fn move_layers(&mut self, ids: &[u64], dest: Dest) -> usize {
        let dest_id = match dest {
            Dest::Group(g) | Dest::Above(g) => g,
        };
        let inner_of_dest = match dest {
            Dest::Group(g) => Some(g),
            Dest::Above(t) => parent_of(&self.p.layers, t).flatten(),
        };
        let base = match inner_of_dest {
            Some(g) => abs_start(&self.p.layers, g, 0.0),
            None => Some(0.0),
        };
        let Some(base) = base else { return 0 };
        let moving: Vec<u64> = self
            .without_nested(ids)
            .into_iter()
            .filter(|id| {
                let Some(l) = self.p.find(*id) else { return false };
                let already_there = matches!(dest, Dest::Group(_)) && parent_of(&self.p.layers, *id) == Some(Some(dest_id));
                !matches!(l.content, Content::Camera { .. }) && !subtree_has(l, dest_id) && !already_there
            })
            .collect();
        if moving.is_empty() {
            return 0;
        }
        let starts: Vec<(u64, f64)> = moving.iter().filter_map(|id| Some((*id, abs_start(&self.p.layers, *id, 0.0)?))).collect();
        let set: HashSet<u64> = moving.iter().copied().collect();
        let mut out = vec![];
        remove_ids(&mut self.p.layers, &set, &mut out);
        for l in &mut out {
            l.start = starts.iter().find(|s| s.0 == l.id).map_or(l.start, |s| s.1) - base;
            // Parent links only resolve within one stack.
            l.parent = None;
        }
        clear_parents(&mut self.p.layers, &moving);
        let count = out.len();
        match dest {
            Dest::Group(g) => {
                if let Some(Content::Group { children }) = self.p.find_mut(g).map(|l| &mut l.content) {
                    children.append(&mut out);
                }
                self.expanded.insert(g);
            }
            Dest::Above(t) => {
                insert_above(&mut self.p.layers, t, &mut out);
            }
        }
        if let Some(g) = inner_of_dest {
            refit_chain(&mut self.p.layers, g);
        }
        self.touch();
        count
    }

    pub fn add_selected_to_group(&mut self, g: u64) {
        let ids = self.sel.clone();
        let n = self.move_layers(&ids, Dest::Group(g));
        self.status = Some(if n > 0 { format!("{n} katman gruba eklendi.") } else { "Bu seçim bu gruba eklenemez.".into() });
    }

    /// Takes each selected layer one level out, to just above the group it was in.
    pub fn remove_selected_from_group(&mut self) {
        let mut n = 0;
        for id in self.top_selection() {
            if let Some(Some(g)) = parent_of(&self.p.layers, id) {
                n += self.move_layers(&[id], Dest::Above(g));
            }
        }
        self.status = Some(if n > 0 { format!("{n} katman gruptan çıkarıldı.") } else { "Seçili katman bir grupta değil.".into() });
    }

    /// Cuts a layer at project time `t`; returns the new right part's id.
    pub fn split_at(&mut self, id: u64, t: f64) -> Option<u64> {
        let abs = abs_start(&self.p.layers, id, 0.0)?;
        let cut = t - abs;
        let l = self.p.find(id)?;
        if cut <= 1e-3 || cut >= l.dur - 1e-3 || l.locked {
            return None;
        }
        let mut right = l.clone();
        crate::app::reassign_ids(&mut right, &mut self.p);
        let left = self.p.find_mut(id)?;
        left.dur = cut;
        right.start += cut;
        right.dur -= cut;
        right.in_point += cut * right.speed as f64;
        right.visit_tracks(&mut |_, tr| tr.shift(-cut));
        // Children run on the group's own clock, which now starts `cut` later.
        if let Content::Group { children } = &mut right.content {
            for c in children {
                c.start -= cut;
            }
        }
        let new_id = right.id;
        crate::app::insert_above(&mut self.p.layers, id, right);
        Some(new_id)
    }

    /// Cuts every selected clip at its own middle and keeps both halves selected,
    /// so pressing again halves them further.
    pub fn split_middle_selected(&mut self) {
        let ids = self.top_selection();
        if ids.is_empty() {
            self.status = Some("Bölmek için bir klip seç.".into());
            return;
        }
        let (mut made, mut too_short) = (vec![], 0);
        let frame = 1.0 / self.p.fps.max(1.0) as f64;
        for id in &ids {
            let (Some(abs), Some(dur)) = (abs_start(&self.p.layers, *id, 0.0), self.p.find(*id).map(|l| l.dur)) else { continue };
            // On a frame boundary, so neither half starts between two frames.
            let mid = abs + (dur / 2.0 / frame).round() * frame;
            if dur < 2.0 * frame {
                too_short += 1;
            } else if let Some(n) = self.split_at(*id, mid) {
                made.push(n);
            }
        }
        self.sel.extend(made.iter().copied());
        self.status = Some(match (made.len(), too_short) {
            (0, _) => "Klipler bölünemedi: çok kısa ya da kilitli.".into(),
            (n, 0) => format!("{n} klip ortadan bölündü."),
            (n, s) => format!("{n} klip ortadan bölündü, {s} tanesi bölünemeyecek kadar kısa."),
        });
        if !made.is_empty() {
            self.touch();
        }
    }

    /// Runs `f` on one track of one layer.
    fn with_track(&mut self, id: u64, name: &str, f: &mut dyn FnMut(&mut dyn AnyTrack)) {
        if let Some(l) = self.p.find_mut(id) {
            l.visit_tracks(&mut |n, tr| {
                if n == name {
                    f(tr);
                }
            });
        }
    }

    /// Selected keys grouped per track, as (layer, track, indices).
    fn keys_by_track(&self) -> Vec<(u64, String, Vec<usize>)> {
        let mut out: Vec<(u64, String, Vec<usize>)> = vec![];
        for (id, name, k) in &self.sel_keys {
            match out.iter_mut().find(|e| e.0 == *id && e.1 == *name) {
                Some(e) => e.2.push(*k),
                None => out.push((*id, name.clone(), vec![*k])),
            }
        }
        out
    }

    /// Moves the selected keys together, so key (id, name, k) lands on layer-local `t`.
    pub fn move_selected_keys(&mut self, id: u64, name: &str, k: usize, t: f64) {
        if !self.sel_keys.iter().any(|s| s.0 == id && s.1 == name && s.2 == k) {
            self.sel_keys = vec![(id, name.to_string(), k)];
        }
        let groups = self.keys_by_track();
        let mut current = None;
        let mut earliest = f64::MAX;
        for (gid, gname, idx) in &groups {
            self.with_track(*gid, gname, &mut |tr| {
                let times = tr.times();
                for i in idx {
                    if let Some(v) = times.get(*i) {
                        earliest = earliest.min(*v);
                        if *gid == id && gname == name && *i == k {
                            current = Some(*v);
                        }
                    }
                }
            });
        }
        let Some(current) = current else { return };
        // The earliest key stops at 0, which holds the whole selection's spacing.
        let dt = (t - current).max(-earliest);
        if dt == 0.0 {
            return;
        }
        let mut next = vec![];
        for (gid, gname, idx) in groups {
            let mut moved = vec![];
            self.with_track(gid, &gname, &mut |tr| moved = tr.move_keys(&idx, dt));
            next.extend(moved.into_iter().map(|i| (gid, gname.clone(), i)));
        }
        // The grabbed key stays the primary one, last in the list.
        let primary_t = current + dt;
        let mut primary = None;
        self.with_track(id, name, &mut |tr| primary = tr.times().iter().position(|v| (*v - primary_t).abs() < 1e-9));
        if let Some(p) = next.iter().position(|e| e.0 == id && e.1 == name && Some(e.2) == primary) {
            let last = next.remove(p);
            next.push(last);
        }
        self.sel_keys = next;
        self.touch();
    }

    pub fn delete_selected_keys(&mut self) {
        let mut n = 0;
        for (id, name, mut idx) in self.keys_by_track() {
            // Highest first, so earlier indices stay valid.
            idx.sort_unstable_by(|a, b| b.cmp(a));
            n += idx.len();
            self.with_track(id, &name, &mut |tr| {
                for i in &idx {
                    tr.remove_key(*i);
                }
            });
        }
        self.sel_keys.clear();
        if n > 0 {
            self.status = Some(format!("{n} keyframe silindi."));
            self.touch();
        }
    }

    pub fn set_selected_keys_ease(&mut self, e: Ease) {
        for (id, name, idx) in self.keys_by_track() {
            self.with_track(id, &name, &mut |tr| {
                for i in &idx {
                    tr.set_ease(*i, e);
                }
            });
        }
        self.touch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::Assets;
    use crate::render::render_project;

    fn rect(id: u64, start: f64, dur: f64, pos: [f32; 2], c: [f32; 4]) -> Layer {
        let sh = ShapeData { kind: ShapeKind::Rect, size: Track::new([40.0, 40.0]), radius: Track::new(0.0), sides: Track::new(5.0), inner: Track::new(0.5), path: vec![], closed: true, freehand: false };
        let mut l = Layer::new(id, "r", Content::Shape(sh), pos, dur);
        l.start = start;
        l.fill.a = Track::new(c);
        l
    }
    fn group(id: u64, start: f64, dur: f64, children: Vec<Layer>) -> Layer {
        let mut g = Layer::new(id, "g", Content::Group { children }, [100.0, 50.0], dur);
        g.start = start;
        g
    }
    fn editor(layers: Vec<Layer>) -> Editor {
        let mut p = Project::new("t", 200, 100, 30.0, [0.0, 0.0, 0.0, 1.0]);
        p.layers = layers;
        p.next_id = 100;
        let path = std::env::temp_dir().join("cm_ops_test_unused.cmproj");
        Editor::new(p, path)
    }
    fn pixels(p: &Project, t: f64) -> Vec<[f32; 4]> {
        render_project(p, t, 1.0, &Assets::default()).px
    }
    fn same(a: &[[f32; 4]], b: &[[f32; 4]]) -> bool {
        a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| (x - y).abs() < 0.01)
    }

    #[test]
    fn adding_to_a_group_keeps_clip_timing_and_the_picture() {
        let mut e = editor(vec![group(1, 2.0, 4.0, vec![rect(2, 0.0, 4.0, [50.0, 50.0], [1.0, 0.0, 0.0, 1.0])]), rect(3, 3.0, 1.0, [150.0, 50.0], [0.0, 1.0, 0.0, 1.0])]);
        let before = pixels(&e.p, 3.5);
        assert_eq!(e.move_layers(&[3], Dest::Group(1)), 1);
        assert_eq!(e.p.layers.len(), 1);
        assert_eq!(e.p.find(3).unwrap().start, 1.0, "grup saatine göre 1 sn");
        assert_eq!(abs_start(&e.p.layers, 3, 0.0), Some(3.0));
        assert!(same(&before, &pixels(&e.p, 3.5)));
    }

    #[test]
    fn group_grows_to_cover_a_clip_outside_its_range() {
        let mut g = group(1, 2.0, 2.0, vec![rect(2, 0.0, 2.0, [50.0, 50.0], [1.0, 0.0, 0.0, 1.0])]);
        g.tf.rot.set_key(1.0, 45.0);
        let mut e = editor(vec![g, rect(3, 0.5, 6.5, [150.0, 50.0], [0.0, 1.0, 0.0, 1.0])]);
        e.move_layers(&[3], Dest::Group(1));
        let g = e.p.find(1).unwrap();
        assert_eq!((g.start, g.dur), (0.5, 6.5), "grup her iki yöne büyümeli");
        assert_eq!(abs_start(&e.p.layers, 2, 0.0), Some(2.0), "eski üye yerinde kalmalı");
        assert_eq!(abs_start(&e.p.layers, 3, 0.0), Some(0.5));
        // The group's key was at project time 3 s, and must still be.
        assert_eq!(g.tf.rot.keys[0].t + g.start, 3.0);
    }

    #[test]
    fn removing_from_a_group_restores_project_time_and_place() {
        let c = rect(3, 1.0, 1.0, [150.0, 50.0], [0.0, 1.0, 0.0, 1.0]);
        let mut e = editor(vec![rect(9, 0.0, 9.0, [50.0, 20.0], [0.0, 0.0, 1.0, 1.0]), group(1, 2.0, 4.0, vec![rect(2, 0.0, 4.0, [50.0, 50.0], [1.0, 0.0, 0.0, 1.0]), c]), rect(8, 0.0, 9.0, [50.0, 80.0], [1.0, 1.0, 1.0, 1.0])]);
        let before = pixels(&e.p, 3.5);
        e.sel = vec![3];
        e.remove_selected_from_group();
        let order: Vec<u64> = e.p.layers.iter().map(|l| l.id).collect();
        assert_eq!(order, vec![9, 1, 3, 8], "grubun hemen üstüne gelmeli");
        assert_eq!(e.p.find(3).unwrap().start, 3.0);
        assert!(same(&before, &pixels(&e.p, 3.5)));
    }

    #[test]
    fn nested_removal_goes_one_level_only() {
        let inner = group(2, 1.0, 3.0, vec![rect(3, 0.5, 1.0, [50.0, 50.0], [1.0; 4])]);
        let mut e = editor(vec![group(1, 2.0, 5.0, vec![inner])]);
        e.sel = vec![3];
        e.remove_selected_from_group();
        assert_eq!(parent_of(&e.p.layers, 3), Some(Some(1)));
        assert_eq!(abs_start(&e.p.layers, 3, 0.0), Some(3.5));
    }

    #[test]
    fn a_group_cannot_enter_itself_or_its_own_child() {
        let inner = group(2, 0.0, 3.0, vec![]);
        let mut e = editor(vec![group(1, 0.0, 5.0, vec![inner])]);
        assert_eq!(e.move_layers(&[1], Dest::Group(1)), 0);
        assert_eq!(e.move_layers(&[1], Dest::Group(2)), 0);
        assert_eq!(e.p.layers.len(), 1, "hiçbir şey kaybolmamalı");
        assert!(e.p.find(2).is_some());
    }

    #[test]
    fn cameras_stay_out_and_group_with_child_moves_once() {
        let cam = Layer::new(4, "k", Content::Camera { fov: Track::new(50.0) }, [0.0, 0.0], 5.0);
        let mut e = editor(vec![group(1, 0.0, 5.0, vec![]), cam, group(2, 1.0, 2.0, vec![rect(3, 0.0, 1.0, [50.0, 50.0], [1.0; 4])])]);
        assert_eq!(e.move_layers(&[4], Dest::Group(1)), 0);
        assert_eq!(e.move_layers(&[2, 3], Dest::Group(1)), 1, "çocuk grubuyla gider");
        assert_eq!(parent_of(&e.p.layers, 3), Some(Some(2)));
    }

    #[test]
    fn moved_layer_loses_only_links_that_cross_stacks() {
        let mut child = rect(3, 0.0, 1.0, [0.0, 0.0], [1.0; 4]);
        child.parent = Some(7);
        let mut follower = rect(5, 0.0, 1.0, [0.0, 0.0], [1.0; 4]);
        follower.parent = Some(3);
        let mut e = editor(vec![group(1, 0.0, 5.0, vec![]), child, follower, rect(7, 0.0, 1.0, [0.0, 0.0], [1.0; 4])]);
        e.move_layers(&[3], Dest::Group(1));
        assert_eq!(e.p.find(3).unwrap().parent, None);
        assert_eq!(e.p.find(5).unwrap().parent, None);
    }

    #[test]
    fn middle_split_halves_every_selected_clip_and_can_repeat() {
        let mut a = rect(1, 1.0, 4.0, [50.0, 50.0], [1.0; 4]);
        a.tf.rot.set_key(3.0, 10.0);
        let mut e = editor(vec![a, rect(2, 0.0, 2.0, [50.0, 50.0], [1.0; 4]), rect(3, 0.0, 9.0, [50.0, 50.0], [1.0; 4])]);
        e.sel = vec![1, 2];
        e.split_middle_selected();
        assert_eq!(e.p.layers.len(), 5);
        assert_eq!(e.sel.len(), 4, "iki yarı da seçili kalmalı");
        let l = e.p.find(1).unwrap();
        assert_eq!((l.start, l.dur), (1.0, 2.0));
        let r = e.p.layers.iter().find(|q| q.id != 1 && q.start == 3.0).unwrap();
        assert_eq!(r.dur, 2.0);
        assert_eq!(r.tf.rot.keys[0].t, 1.0, "keyframe aynı anda kalmalı");
        assert_eq!(e.p.find(3).unwrap().dur, 9.0, "seçili olmayan bölünmemeli");
        e.split_middle_selected();
        assert_eq!(e.p.layers.len(), 9);
    }

    #[test]
    fn middle_split_lands_on_a_frame_and_skips_clips_too_short() {
        let mut e = editor(vec![rect(1, 0.0, 1.0, [0.0, 0.0], [1.0; 4]), rect(2, 0.0, 0.03, [0.0, 0.0], [1.0; 4])]);
        e.sel = vec![1, 2];
        e.split_middle_selected();
        assert_eq!(e.p.layers.len(), 3);
        assert_eq!(e.p.find(1).unwrap().dur, 0.5);
        assert!(e.status.as_deref().unwrap_or("").contains("kısa"));
        e.p.layers.clear();
        e.sel.clear();
        e.split_middle_selected();
        assert!(e.status.as_deref().unwrap_or("").contains("seç"));
    }

    #[test]
    fn splitting_a_group_keeps_the_picture_on_both_sides() {
        let mut child = rect(2, 0.0, 6.0, [20.0, 50.0], [1.0, 0.0, 0.0, 1.0]);
        child.tf.pos.set_key(0.0, [20.0, 50.0]);
        child.tf.pos.set_key(6.0, [180.0, 50.0]);
        let mut e = editor(vec![group(1, 0.0, 6.0, vec![child])]);
        let want = [pixels(&e.p, 1.0), pixels(&e.p, 4.5)];
        e.sel = vec![1];
        e.split_middle_selected();
        assert!(same(&want[0], &pixels(&e.p, 1.0)), "sol yarı aynı kalmalı");
        assert!(same(&want[1], &pixels(&e.p, 4.5)), "sağ yarı baştan başlamamalı");
    }

    #[test]
    fn splitting_inside_a_group_uses_project_time() {
        let mut e = editor(vec![group(1, 2.0, 4.0, vec![rect(2, 1.0, 2.0, [0.0, 0.0], [1.0; 4])])]);
        assert!(e.split_at(2, 4.0).is_some());
        assert_eq!(e.p.find(2).unwrap().dur, 1.0);
        assert!(e.split_at(2, 9.0).is_none());
    }

    #[test]
    fn selected_keys_move_together_and_keep_spacing() {
        let mut a = rect(1, 0.0, 9.0, [0.0, 0.0], [1.0; 4]);
        a.tf.rot.set_key(1.0, 10.0);
        a.tf.rot.set_key(2.0, 20.0);
        a.tf.rot.set_key(5.0, 50.0);
        a.opacity.set_key(1.5, 0.5);
        let mut e = editor(vec![a]);
        e.sel_keys = vec![(1, "Dönme".into(), 0), (1, "Opaklık".into(), 0), (1, "Dönme".into(), 1)];
        e.move_selected_keys(1, "Dönme", 1, 3.0);
        let l = e.p.find(1).unwrap();
        assert_eq!(l.tf.rot.times(), vec![2.0, 3.0, 5.0], "seçili olmayan yerinde kalmalı");
        assert_eq!(l.tf.rot.keys[0].v, 10.0);
        assert_eq!(l.opacity.times(), vec![2.5]);
        assert_eq!(e.sel_keys.last().map(|s| s.2), Some(1), "tutulan keyframe birincil kalmalı");

        // Past zero, the whole selection stops together.
        e.move_selected_keys(1, "Dönme", 1, -9.0);
        let l = e.p.find(1).unwrap();
        assert_eq!(l.tf.rot.times(), vec![0.0, 1.0, 5.0]);
        assert_eq!(l.opacity.times(), vec![0.5]);
    }

    #[test]
    fn keys_can_pass_unselected_ones_and_selection_follows() {
        let mut a = rect(1, 0.0, 9.0, [0.0, 0.0], [1.0; 4]);
        a.tf.rot.set_key(1.0, 10.0);
        a.tf.rot.set_key(2.0, 20.0);
        let mut e = editor(vec![a]);
        e.sel_keys = vec![(1, "Dönme".into(), 0)];
        e.move_selected_keys(1, "Dönme", 0, 3.0);
        let l = e.p.find(1).unwrap();
        assert_eq!(l.tf.rot.keys[1].v, 10.0);
        assert_eq!(e.sel_keys, vec![(1, "Dönme".to_string(), 1)]);
    }

    #[test]
    fn deleting_and_easing_apply_to_all_selected_keys() {
        let mut a = rect(1, 0.0, 9.0, [0.0, 0.0], [1.0; 4]);
        for t in [1.0, 2.0, 3.0] {
            a.tf.rot.set_key(t, t as f32);
        }
        let mut e = editor(vec![a]);
        e.sel_keys = vec![(1, "Dönme".into(), 0), (1, "Dönme".into(), 2)];
        e.set_selected_keys_ease(Ease::Bounce);
        let l = e.p.find(1).unwrap();
        assert_eq!(l.tf.rot.keys[0].ease, Ease::Bounce);
        assert_ne!(l.tf.rot.keys[1].ease, Ease::Bounce);
        assert_eq!(l.tf.rot.keys[2].ease, Ease::Bounce);
        e.delete_selected_keys();
        assert_eq!(e.p.find(1).unwrap().tf.rot.times(), vec![2.0]);
        assert!(e.sel_keys.is_empty());
    }
}
