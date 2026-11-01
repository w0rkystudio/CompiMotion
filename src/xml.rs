//! Shareable XML projects. Media can be embedded so a single file is all a recipient needs.
//! Imported files come from other people, so everything read here is treated as untrusted.
use crate::effects::{self, Kind};
use crate::model::*;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;
const MAX_DEPTH: usize = 128;
const MEDIA_REF: &str = "media/";
/// Marks a parsed entry as a font rather than a numbered media file.
const FONT_ENTRY: usize = usize::MAX;
const MEDIA_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "mp4", "mov", "m4v", "avi", "wmv", "mkv", "webm", "mp3", "wav", "aac", "m4a", "wma", "flac"];

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub embedded: usize,
    /// Media that could not be included or found.
    pub missing: Vec<String>,
}

fn file_name_of(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or("").to_string()
}

fn for_each_media(layers: &mut [Layer], f: &mut dyn FnMut(&mut String)) {
    for l in layers {
        match &mut l.content {
            Content::Image { path } | Content::Video { path } | Content::Audio { path } => f(path),
            Content::Group { children } => for_each_media(children, f),
            _ => {}
        }
    }
}

/// Total size of the project's media in bytes, for showing before a share.
pub fn media_size(p: &Project) -> u64 {
    let mut q = p.clone();
    let mut seen = HashSet::new();
    let mut total = 0;
    for_each_media(&mut q.layers, &mut |path| {
        if seen.insert(path.clone()) {
            total += std::fs::metadata(&*path).map(|m| m.len()).unwrap_or(0);
        }
    });
    total
}

fn for_each_font(layers: &mut [Layer], f: &mut dyn FnMut(&mut String)) {
    for l in layers {
        match &mut l.content {
            Content::Text(t) => f(&mut t.font),
            Content::Group { children } => for_each_font(children, f),
            _ => {}
        }
    }
}

// ---- Writing ----

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // Attribute values would otherwise lose these to whitespace normalization.
            '\n' => o.push_str("&#10;"),
            '\r' => o.push_str("&#13;"),
            '\t' => o.push_str("&#9;"),
            // Other control characters cannot appear in XML 1.0 at all.
            c if (c as u32) < 0x20 => {}
            c => o.push(c),
        }
    }
    o
}

fn valid_name(k: &str) -> bool {
    let mut it = k.chars();
    it.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && it.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !k.eq_ignore_ascii_case("xml") && k != "e" && k != "i"
}

fn write_value(w: &mut impl Write, tag: &str, attr: &str, v: &Value, depth: usize) -> std::io::Result<()> {
    let pad = "  ".repeat(depth);
    let scalar = |w: &mut dyn Write, t: &str, val: &str| writeln!(w, "{pad}<{tag}{attr} t=\"{t}\" v=\"{}\"/>", escape(val));
    match v {
        Value::Null => writeln!(w, "{pad}<{tag}{attr} t=\"z\"/>"),
        Value::Bool(b) => scalar(w, "b", if *b { "1" } else { "0" }),
        Value::Number(n) => scalar(w, "n", &n.to_string()),
        Value::String(s) => scalar(w, "s", s),
        Value::Array(a) => {
            writeln!(w, "{pad}<{tag}{attr} t=\"a\">")?;
            for x in a {
                write_value(w, "i", "", x, depth + 1)?;
            }
            writeln!(w, "{pad}</{tag}>")
        }
        Value::Object(m) => {
            writeln!(w, "{pad}<{tag}{attr} t=\"o\">")?;
            for (k, x) in m {
                if valid_name(k) {
                    write_value(w, k, "", x, depth + 1)?;
                } else {
                    write_value(w, "e", &format!(" k=\"{}\"", escape(k)), x, depth + 1)?;
                }
            }
            writeln!(w, "{pad}</{tag}>")
        }
    }
}

/// Writes the project. Local paths never enter the file: they reveal folder and user names.
pub fn write(p: &Project, out: &Path, embed: bool) -> Result<Report, String> {
    write_with_fonts(p, out, embed, &crate::media::custom_fonts_dir())
}

/// `fonts`: the added-fonts folder. Those fonts are embedded along with the media, because a
/// recipient cannot be expected to have them. System fonts stay out.
pub fn write_with_fonts(p: &Project, out: &Path, embed: bool, fonts: &Path) -> Result<Report, String> {
    let tmp = out.with_extension("xml.tmp");
    let r = write_inner(p, &tmp, embed, fonts).map_err(|e| e.to_string());
    match r {
        Ok(rep) => {
            std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
            Ok(rep)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Streams a file as base64. Chunks are a multiple of 3 bytes, so pieces join with no padding between.
fn write_base64(w: &mut impl Write, path: &Path) -> std::io::Result<()> {
    let mut buf = vec![0u8; 3 << 18];
    let mut f = std::fs::File::open(path)?;
    loop {
        let mut filled = 0;
        while filled < buf.len() {
            let n = f.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            return Ok(());
        }
        w.write_all(STANDARD.encode(&buf[..filled]).as_bytes())?;
        if filled < buf.len() {
            return Ok(());
        }
    }
}

fn write_inner(p: &Project, tmp: &Path, embed: bool, fonts: &Path) -> std::io::Result<Report> {
    let mut q = p.clone();
    let mut report = Report::default();
    let mut files: Vec<(String, String)> = vec![];
    let mut ids: HashMap<String, usize> = HashMap::new();
    for_each_media(&mut q.layers, &mut |path| {
        let name = file_name_of(path);
        if embed && Path::new(path.as_str()).is_file() {
            let id = *ids.entry(path.clone()).or_insert_with(|| {
                files.push((path.clone(), name.clone()));
                files.len() - 1
            });
            *path = format!("{MEDIA_REF}{id}/{name}");
        } else {
            if embed {
                report.missing.push(name.clone());
            }
            *path = name;
        }
    });
    let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(tmp)?);
    writeln!(w, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(w, "<compimotion version=\"{VERSION}\">")?;
    let tree = serde_json::to_value(&q).map_err(std::io::Error::other)?;
    write_value(&mut w, "project", "", &tree, 1)?;
    writeln!(w, "  <media>")?;
    for (id, (path, name)) in files.iter().enumerate() {
        write!(w, "    <file id=\"{id}\" name=\"{}\">", escape(name))?;
        write_base64(&mut w, Path::new(path))?;
        writeln!(w, "</file>")?;
        report.embedded += 1;
    }
    if embed {
        let mut used: Vec<String> = vec![];
        for_each_font(&mut q.layers, &mut |f| {
            if !used.contains(f) && !f.contains(['/', '\\']) && fonts.join(f.as_str()).is_file() {
                used.push(f.clone());
            }
        });
        for f in used {
            write!(w, "    <font name=\"{}\">", escape(&f))?;
            write_base64(&mut w, &fonts.join(&f))?;
            writeln!(w, "</font>")?;
            report.embedded += 1;
        }
    }
    writeln!(w, "  </media>")?;
    writeln!(w, "</compimotion>")?;
    w.flush()?;
    Ok(report)
}

// ---- Reading ----

struct Node {
    key: String,
    t: u8,
    object: Map<String, Value>,
    array: Vec<Value>,
    scalar: Value,
}

fn attr(e: &quick_xml::events::BytesStart, name: &[u8]) -> Result<Option<String>, String> {
    match e.try_get_attribute(std::str::from_utf8(name).unwrap_or_default()).map_err(|x| x.to_string())? {
        Some(a) => Ok(Some(a.unescape_value().map_err(|x| x.to_string())?.into_owned())),
        None => Ok(None),
    }
}

fn scalar_of(t: &str, v: Option<String>) -> Result<Value, String> {
    let v = v.unwrap_or_default();
    Ok(match t {
        "z" => Value::Null,
        "b" => Value::Bool(v == "1"),
        "s" => Value::String(v),
        "n" => {
            let n: Value = v.parse().map_err(|_| format!("Geçersiz sayı: {v}"))?;
            if !n.is_number() {
                return Err(format!("Geçersiz sayı: {v}"));
            }
            n
        }
        _ => return Err(format!("Bilinmeyen değer türü: {t}")),
    })
}

fn attach(parent: &mut Node, key: String, v: Value) {
    if parent.t == b'a' {
        parent.array.push(v);
    } else {
        parent.object.insert(key, v);
    }
}

/// Parses the file into a project tree and a list of (id, name, base64) media entries.
fn parse(text: &str) -> Result<(Value, Vec<(usize, String, String)>), String> {
    let mut r = Reader::from_str(text);
    let mut stack: Vec<Node> = vec![];
    let mut tree = None;
    let mut media = vec![];
    let mut seen_root = false;
    let mut in_file: Option<(usize, String, String)> = None;
    loop {
        let ev = r.read_event().map_err(|e| format!("XML okunamadı: {e}"))?;
        match ev {
            Event::Eof => break,
            Event::DocType(_) => return Err("DOCTYPE içeren dosyalar kabul edilmiyor.".into()),
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(ev, Event::Empty(_));
                let name = e.name().into_inner().to_string();
                if !seen_root {
                    if name != "compimotion" {
                        return Err("Bu bir CompiMotion proje dosyası değil.".into());
                    }
                    let ver: u32 = attr(e, b"version")?.and_then(|s| s.parse().ok()).ok_or("Dosyanın sürümü okunamadı.")?;
                    if ver > VERSION {
                        return Err(format!("Bu dosya daha yeni bir CompiMotion sürümüyle yapılmış (sürüm {ver})."));
                    }
                    seen_root = true;
                    continue;
                }
                if stack.is_empty() && name == "media" {
                    continue;
                }
                if stack.is_empty() && (name == "file" || name == "font") {
                    let id: usize = if name == "font" { FONT_ENTRY } else { attr(e, b"id")?.and_then(|s| s.parse().ok()).ok_or("Medya kimliği okunamadı.")? };
                    let fname = attr(e, b"name")?.unwrap_or_default();
                    if empty {
                        media.push((id, fname, String::new()));
                    } else {
                        in_file = Some((id, fname, String::new()));
                    }
                    continue;
                }
                if stack.is_empty() && name != "project" {
                    continue;
                }
                if stack.len() >= MAX_DEPTH {
                    return Err("Dosya çok derin iç içe.".into());
                }
                let t = attr(e, b"t")?.ok_or("Değer türü eksik.")?;
                let key = attr(e, b"k")?.unwrap_or(name);
                if t == "o" || t == "a" {
                    let node = Node { key, t: t.as_bytes()[0], object: Map::new(), array: vec![], scalar: Value::Null };
                    if empty {
                        let v = if node.t == b'a' { Value::Array(vec![]) } else { Value::Object(Map::new()) };
                        match stack.last_mut() {
                            Some(p) => attach(p, node.key, v),
                            None => tree = Some(v),
                        }
                    } else {
                        stack.push(node);
                    }
                } else {
                    let v = scalar_of(&t, attr(e, b"v")?)?;
                    if empty {
                        match stack.last_mut() {
                            Some(p) => attach(p, key, v),
                            None => return Err("Proje kökü geçersiz.".into()),
                        }
                    } else {
                        stack.push(Node { key, t: b's', object: Map::new(), array: vec![], scalar: v });
                    }
                }
            }
            Event::Text(t) => {
                if let Some(f) = &mut in_file {
                    f.2.push_str(&t);
                }
            }
            Event::End(e) => {
                let name = e.name();
                if let Some(f) = in_file.take() {
                    media.push(f);
                    continue;
                }
                if stack.is_empty() {
                    let _ = name;
                    continue;
                }
                let Some(n) = stack.pop() else { continue };
                let v = match n.t {
                    b'a' => Value::Array(n.array),
                    b'o' => Value::Object(n.object),
                    _ => n.scalar,
                };
                match stack.last_mut() {
                    Some(p) => attach(p, n.key, v),
                    None => tree = Some(v),
                }
            }
            _ => {}
        }
    }
    if !seen_root {
        return Err("Bu bir CompiMotion proje dosyası değil.".into());
    }
    Ok((tree.ok_or("Dosyada proje bulunamadı.")?, media))
}

/// Keeps only a plain file name with an allowed media extension.
fn safe_media_name(name: &str) -> Option<String> {
    let base = file_name_of(name);
    let clean: String = base.chars().filter(|c| c.is_alphanumeric() || " ._-()".contains(*c)).collect();
    let clean = clean.trim_matches(['.', ' ']).to_string();
    let ext = Path::new(&clean).extension()?.to_string_lossy().to_lowercase();
    if !MEDIA_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let keep = clean.chars().count().min(80);
    let stem_end = clean.chars().count() - ext.chars().count() - 1;
    let stem: String = clean.chars().take(stem_end.min(keep)).collect();
    Some(format!("{stem}.{ext}"))
}

fn extract(name: &str, b64: &str, dir: &Path) -> Result<PathBuf, String> {
    use std::hash::{Hash, Hasher};
    let safe = safe_media_name(name).ok_or_else(|| format!("İzin verilmeyen dosya türü: {}", file_name_of(name)))?;
    let compact: String = b64.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let data = STANDARD.decode(compact).map_err(|_| format!("Bozuk medya verisi: {safe}"))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("{:016x}_{safe}", h.finish()));
    if !dest.is_file() {
        let tmp = dest.with_extension("tmp");
        std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    }
    Ok(dest)
}

/// Brings imported values into the ranges the editor assumes.
fn sanitize_layers(v: &mut [Layer], seen: &mut HashSet<u64>, next: &mut u64) {
    for l in v {
        if !seen.insert(l.id) {
            l.id = *next;
            seen.insert(l.id);
            *next += 1;
        }
        if !l.start.is_finite() || l.start < 0.0 {
            l.start = 0.0;
        }
        if !l.dur.is_finite() || l.dur <= 0.0 {
            l.dur = 1.0;
        }
        if !l.in_point.is_finite() || l.in_point < 0.0 {
            l.in_point = 0.0;
        }
        l.speed = if l.speed.is_finite() { l.speed.clamp(0.01, 100.0) } else { 1.0 };
        l.visit_tracks(&mut |_, tr| tr.sort_keys());
        for e in &mut l.effects {
            let Some(def) = effects::find(&e.def) else { continue };
            for q in def.params {
                let (Kind::F(min, max, _), Some(tr)) = (q.kind, e.p.get_mut(q.key)) else { continue };
                tr.base[0] = tr.base[0].clamp(min, max);
                for k in &mut tr.keys {
                    k.v[0] = k.v[0].clamp(min, max);
                }
            }
        }
        match &mut l.content {
            Content::Text(t) => {
                // A font is a file name inside the system fonts folder, never a path.
                if t.font.contains(['/', '\\']) || t.font.contains("..") {
                    t.font = "segoeui.ttf".into();
                }
            }
            Content::Group { children } => sanitize_layers(children, seen, next),
            _ => {}
        }
    }
}

/// Reads a shared project, unpacking embedded media into `media_dir`.
/// Media that was not embedded is looked for beside the XML file, by name only.
/// Embedded fonts go to the "fonts" folder next to `media_dir`.
pub fn read(xml: &Path, media_dir: &Path) -> Result<(Project, Report), String> {
    let fonts_dir = media_dir.with_file_name("fonts");
    let text = std::fs::read_to_string(xml).map_err(|e| format!("Dosya okunamadı: {e}"))?;
    let (tree, media) = parse(&text)?;
    let mut p: Project = serde_json::from_value(tree).map_err(|e| format!("Proje içeriği geçersiz: {e}"))?;
    drop(text);

    p.w = p.w.clamp(16, 7680);
    p.h = p.h.clamp(16, 7680);
    p.fps = if p.fps.is_finite() { p.fps.clamp(1.0, 120.0) } else { 30.0 };
    if !p.dur.is_finite() || p.dur <= 0.0 {
        p.dur = 10.0;
    }

    let mut report = Report::default();
    let mut extracted: HashMap<usize, PathBuf> = HashMap::new();
    let mut renamed_fonts: HashMap<String, String> = HashMap::new();
    for (id, name, b64) in &media {
        if *id == FONT_ENTRY {
            let compact: String = b64.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            let installed = STANDARD.decode(compact).map_err(|e| e.to_string()).and_then(|bytes| crate::media::install_font(&fonts_dir, name, &bytes));
            match installed {
                Ok(stored) => {
                    renamed_fonts.insert(name.clone(), stored);
                    report.embedded += 1;
                }
                Err(_) => report.missing.push(file_name_of(name)),
            }
            continue;
        }
        match extract(name, b64, media_dir) {
            Ok(path) => {
                extracted.insert(*id, path);
                report.embedded += 1;
            }
            Err(_) => report.missing.push(file_name_of(name)),
        }
    }
    let beside = xml.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut missing = vec![];
    for_each_media(&mut p.layers, &mut |path| {
        // Only references into this file's own media, or a bare name, are honored:
        // a path written into the XML must not make us open arbitrary local files.
        let id = path.strip_prefix(MEDIA_REF).and_then(|r| r.split('/').next()).and_then(|s| s.parse::<usize>().ok());
        if let Some(found) = id.and_then(|i| extracted.get(&i)) {
            *path = found.to_string_lossy().into_owned();
            return;
        }
        let name = file_name_of(path);
        let near = beside.join(&name);
        if safe_media_name(&name).is_some() && near.is_file() {
            *path = near.to_string_lossy().into_owned();
        } else {
            missing.push(name.clone());
            *path = name;
        }
    });
    report.missing.extend(missing);

    let mut seen = HashSet::new();
    fn max_id(v: &[Layer]) -> u64 {
        v.iter().map(|l| l.id.max(if let Content::Group { children } = &l.content { max_id(children) } else { 0 })).max().unwrap_or(0)
    }
    let mut next = max_id(&p.layers).saturating_add(1).max(p.next_id);
    sanitize_layers(&mut p.layers, &mut seen, &mut next);
    p.next_id = next;

    // Text whose font did not arrive falls back to a system font, so the recipient should know.
    let mut absent = vec![];
    for_each_font(&mut p.layers, &mut |f| {
        if let Some(stored) = renamed_fonts.get(f.as_str()) {
            *f = stored.clone();
        } else if !crate::media::font_available(&fonts_dir, f) {
            absent.push(f.clone());
        }
    });
    report.missing.extend(absent);
    report.missing.sort();
    report.missing.dedup();
    Ok((p, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cm_xml_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn wrap(project: &str) -> String {
        format!("<?xml version=\"1.0\"?><compimotion version=\"1\">{project}<media></media></compimotion>")
    }
    fn tree_of(p: &Project) -> String {
        serde_json::to_string(p).unwrap()
    }

    #[test]
    fn project_round_trips_exactly() {
        let d = dir("rt");
        let mut p = demo::project();
        // Text that would break naive XML, plus Turkish characters and a line break.
        p.name = "a&b<c>\"d'e şğİı".into();
        if let Content::Text(t) = &mut p.layers[3].content {
            t.text = "Satır 1\nSatır 2 & <etiket>\ttab".into();
        }
        let child = p.layers.remove(1);
        let g = Layer::new(50, "Grup", Content::Group { children: vec![child] }, [0.0, 0.0], 3.0);
        p.layers.push(g);
        p.next_id = 51;
        let f = d.join("p.xml");
        assert_eq!(write(&p, &f, true).unwrap().embedded, 0);
        let (back, rep) = read(&f, &d.join("m")).unwrap();
        assert!(rep.missing.is_empty());
        assert_eq!(tree_of(&back), tree_of(&p));
    }

    #[test]
    fn embedded_media_arrives_intact_and_paths_stay_private() {
        let d = dir("media");
        let src_dir = d.join("gizli_kullanici");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("resim.png");
        // Longer than one encoder chunk, and not a multiple of 3, to exercise chunk joins.
        let payload: Vec<u8> = (0..(3usize << 18) + 7).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(&src, &payload).unwrap();
        let mut p = Project::new("t", 100, 100, 30.0, [0.0; 4]);
        p.layers.push(Layer::new(1, "a", Content::Image { path: src.to_string_lossy().into() }, [0.0, 0.0], 1.0));
        p.layers.push(Layer::new(2, "b", Content::Image { path: src.to_string_lossy().into() }, [0.0, 0.0], 1.0));
        let f = d.join("p.xml");
        assert_eq!(write(&p, &f, true).unwrap().embedded, 1, "aynı dosya bir kez gömülmeli");
        let xml = std::fs::read_to_string(&f).unwrap();
        assert!(!xml.contains("gizli_kullanici"), "yerel yol sızmamalı");

        // The recipient has neither the folder nor the original file.
        std::fs::remove_dir_all(&src_dir).unwrap();
        let media = d.join("alici");
        let (back, rep) = read(&f, &media).unwrap();
        assert_eq!(rep.embedded, 1);
        assert!(rep.missing.is_empty());
        let Content::Image { path } = &back.layers[1].content else { panic!() };
        assert!(Path::new(path).starts_with(&media));
        assert_eq!(std::fs::read(path).unwrap(), payload);
    }

    #[test]
    fn without_embedding_media_is_found_beside_the_xml_by_name() {
        let d = dir("near");
        let a = d.join("a");
        std::fs::create_dir_all(&a).unwrap();
        let mut p = Project::new("t", 100, 100, 30.0, [0.0; 4]);
        p.layers.push(Layer::new(1, "a", Content::Audio { path: a.join("ses.mp3").to_string_lossy().into() }, [0.0, 0.0], 1.0));
        let f = d.join("p.xml");
        write(&p, &f, false).unwrap();
        assert!(!std::fs::read_to_string(&f).unwrap().contains("\\a\\"));
        let (_, rep) = read(&f, &d.join("m")).unwrap();
        assert_eq!(rep.missing, vec!["ses.mp3".to_string()]);
        std::fs::write(d.join("ses.mp3"), b"x").unwrap();
        let (back, rep) = read(&f, &d.join("m")).unwrap();
        assert!(rep.missing.is_empty());
        let Content::Audio { path } = &back.layers[0].content else { panic!() };
        assert_eq!(Path::new(path), d.join("ses.mp3"));
    }

    fn with_tampered(tag: &str, edit: impl Fn(String) -> String) -> (PathBuf, PathBuf) {
        let d = dir(tag);
        let mut p = Project::new("t", 100, 100, 30.0, [0.0; 4]);
        let src = d.join("r.png");
        std::fs::write(&src, b"veri").unwrap();
        p.layers.push(Layer::new(1, "a", Content::Image { path: src.to_string_lossy().into() }, [0.0, 0.0], 1.0));
        let f = d.join("p.xml");
        write(&p, &f, true).unwrap();
        std::fs::write(&f, edit(std::fs::read_to_string(&f).unwrap())).unwrap();
        (f, d)
    }

    #[test]
    fn hostile_media_names_cannot_escape_or_smuggle_programs() {
        let (f, d) = with_tampered("evil", |x| x.replace("name=\"r.png\"", "name=\"..\\..\\..\\zararli.png\""));
        let media = d.join("m");
        read(&f, &media).unwrap();
        let names: Vec<_> = std::fs::read_dir(&media).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(names.len(), 1);
        assert!(names[0].parent() == Some(media.as_path()), "medya klasöründe kalmalı");

        let (f, d) = with_tampered("exe", |x| x.replace("name=\"r.png\"", "name=\"virus.exe\""));
        let media = d.join("m");
        let (_, rep) = read(&f, &media).unwrap();
        assert_eq!(rep.embedded, 0);
        assert_eq!(std::fs::read_dir(&media).map(|r| r.count()).unwrap_or(0), 0, "exe yazılmamalı");
    }

    #[test]
    fn paths_inside_the_xml_cannot_point_at_local_files() {
        let d = dir("path");
        let secret = d.join("gizli.png");
        std::fs::write(&secret, b"x").unwrap();
        let mut p = Project::new("t", 100, 100, 30.0, [0.0; 4]);
        p.layers.push(Layer::new(1, "a", Content::Image { path: "PLACEHOLDER.png".into() }, [0.0, 0.0], 1.0));
        let f = d.join("alt").join("p.xml");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        write(&p, &f, false).unwrap();
        let x = std::fs::read_to_string(&f).unwrap().replace("PLACEHOLDER.png", &secret.to_string_lossy());
        std::fs::write(&f, x).unwrap();
        let (back, rep) = read(&f, &d.join("m")).unwrap();
        let Content::Image { path } = &back.layers[0].content else { panic!() };
        assert_eq!(path, "gizli.png", "tam yol korunmamalı");
        assert_eq!(rep.missing, vec!["gizli.png".to_string()]);
    }

    #[test]
    fn out_of_range_values_are_brought_back_in() {
        let (f, d) = with_tampered("range", |x| x.replace("<w t=\"n\" v=\"100\"/>", "<w t=\"n\" v=\"999999\"/>").replace("<fps t=\"n\" v=\"30.0\"/>", "<fps t=\"n\" v=\"0.0\"/>"));
        let (back, _) = read(&f, &d.join("m")).unwrap();
        assert_eq!(back.w, 7680);
        assert_eq!(back.fps, 1.0);

        let mut l = Layer::new(1, "a", Content::Null, [0.0, 0.0], 1.0);
        let mut e = effects::instance("repeat");
        e.p.insert("n".into(), Track::new([1e9, 0.0, 0.0, 0.0]));
        l.effects.push(e);
        let mut v = vec![l, Layer::new(1, "kopya kimlik", Content::Null, [0.0, 0.0], 1.0)];
        v[0].tf.rot.keys = vec![Key { t: 2.0, v: 0.0, ease: Ease::Linear }, Key { t: 1.0, v: 0.0, ease: Ease::Linear }];
        let (mut seen, mut next) = (HashSet::new(), 10);
        sanitize_layers(&mut v, &mut seen, &mut next);
        assert_eq!(v[0].effects[0].p["n"].base[0], 64.0, "efekt aralığı uygulanmalı");
        assert_eq!(v[1].id, 10, "aynı kimlik yenilenmeli");
        assert_eq!(v[0].tf.rot.times(), vec![1.0, 2.0], "keyframe'ler sıralanmalı");
    }

    fn text_project(font: &str) -> Project {
        let mut p = Project::new("t", 100, 100, 30.0, [0.0; 4]);
        let t = TextData { text: "Şğİ".into(), font: font.into(), size: Track::new(20.0), align: Align::Center, tracking: Track::new(0.0), leading: Track::new(0.0) };
        let child = Layer::new(2, "m", Content::Text(t), [0.0, 0.0], 1.0);
        p.layers.push(Layer::new(1, "g", Content::Group { children: vec![child] }, [0.0, 0.0], 1.0));
        p.next_id = 3;
        p
    }
    fn font_of(p: &Project) -> String {
        match &p.find(2).unwrap().content {
            Content::Text(t) => t.font.clone(),
            _ => panic!(),
        }
    }

    #[test]
    fn added_fonts_travel_inside_the_xml() {
        let d = dir("font");
        let sender = d.join("gonderen");
        let bytes = std::fs::read(crate::media::fonts_dir().join("arial.ttf")).unwrap();
        let name = crate::media::install_font(&sender, "Özel Yazı.ttf", &bytes).unwrap();
        let f = d.join("p.xml");
        let rep = write_with_fonts(&text_project(&name), &f, true, &sender).unwrap();
        assert_eq!(rep.embedded, 1);

        let media = d.join("alici").join("media");
        let (back, rep) = read(&f, &media).unwrap();
        assert!(rep.missing.is_empty(), "{:?}", rep.missing);
        assert_eq!(font_of(&back), name);
        assert_eq!(std::fs::read(d.join("alici").join("fonts").join(&name)).unwrap(), bytes);
    }

    #[test]
    fn a_name_clash_on_the_recipient_keeps_their_font_and_uses_the_new_name() {
        let d = dir("fontclash");
        let sender = d.join("gonderen");
        let arial = std::fs::read(crate::media::fonts_dir().join("arial.ttf")).unwrap();
        let tahoma = std::fs::read(crate::media::fonts_dir().join("tahoma.ttf")).unwrap();
        crate::media::install_font(&sender, "ayni.ttf", &arial).unwrap();
        let f = d.join("p.xml");
        write_with_fonts(&text_project("ayni.ttf"), &f, true, &sender).unwrap();
        let fonts = d.join("alici").join("fonts");
        crate::media::install_font(&fonts, "ayni.ttf", &tahoma).unwrap();
        let (back, _) = read(&f, &d.join("alici").join("media")).unwrap();
        assert_eq!(font_of(&back), "ayni_2.ttf");
        assert_eq!(std::fs::read(fonts.join("ayni.ttf")).unwrap(), tahoma, "alıcının yazı tipi değişmemeli");
    }

    #[test]
    fn a_missing_or_corrupt_font_is_reported_without_failing_the_import() {
        let d = dir("fontgone");
        let f = d.join("p.xml");
        write_with_fonts(&text_project("kayip.ttf"), &f, true, &d.join("yok")).unwrap();
        let (_, rep) = read(&f, &d.join("a").join("media")).unwrap();
        assert_eq!(rep.missing, vec!["kayip.ttf".to_string()]);

        let sender = d.join("gonderen");
        let bytes = std::fs::read(crate::media::fonts_dir().join("arial.ttf")).unwrap();
        crate::media::install_font(&sender, "b.ttf", &bytes).unwrap();
        write_with_fonts(&text_project("b.ttf"), &f, true, &sender).unwrap();
        let x = std::fs::read_to_string(&f).unwrap();
        let start = x.find("<font name=\"b.ttf\">").unwrap() + "<font name=\"b.ttf\">".len();
        let end = x.find("</font>").unwrap();
        std::fs::write(&f, format!("{}QUJD{}", &x[..start], &x[end..])).unwrap();
        let (_, rep) = read(&f, &d.join("c").join("media")).unwrap();
        assert!(rep.missing.contains(&"b.ttf".to_string()));
        assert!(!d.join("c").join("fonts").join("b.ttf").exists(), "bozuk yazı tipi kurulmamalı");
    }

    #[test]
    fn bad_files_give_errors_not_panics() {
        let d = dir("bad");
        let f = d.join("x.xml");
        let m = d.join("m");
        for bad in [
            "bu xml değil".to_string(),
            "<baska/>".to_string(),
            "<compimotion version=\"99\"></compimotion>".to_string(),
            "<compimotion></compimotion>".to_string(),
            wrap(""),
            wrap("<project t=\"o\"><w t=\"s\" v=\"yazı\"/></project>"),
            wrap("<project t=\"o\"><w t=\"n\" v=\"[1]\"/></project>"),
            "<!DOCTYPE x [<!ENTITY a \"b\">]><compimotion version=\"1\"></compimotion>".to_string(),
            format!("<compimotion version=\"1\">{}</compimotion>", "<project t=\"a\">".repeat(500)),
        ] {
            std::fs::write(&f, &bad).unwrap();
            assert!(read(&f, &m).is_err(), "reddedilmeli: {}", &bad[..bad.len().min(60)]);
        }
    }
}
