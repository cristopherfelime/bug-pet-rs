//! Milestone 4: speech bubbles, font loading and reminder badge rasterization.
//!
//! Ports `pet.js:50-235` (line bags / showLine), `ui/bubble-style.js`, `ui/bubble-fx.js`,
//! `ui/bubble-shape.js` and `bubble.js` (BubbleFit) to native Rust. Bubbles are rasterized with
//! `tiny-skia` + `fontdue` into premultiplied RGBA sprites (see [`RgbaSprite`]).
#![allow(dead_code)]

use crate::assets::RgbaSprite;
use crate::config::{parse_hex_color_rgba, CharacterConfig, UiPrefs};
use fontdue::{Font, FontSettings};
use rand::Rng;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tiny_skia as sk;

const PI: f64 = std::f64::consts::PI;
const TAU: f64 = std::f64::consts::PI * 2.0;

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// JS `Math.min(hi, Math.max(lo, v))`.
fn lim(v: f64, lo: f64, hi: f64) -> f64 {
    hi.min(lo.max(v))
}

/// JS `BubbleFx` pseudo random hash `F(t)` in `[0, 1)`.
pub fn fx_rand(t: f64) -> f64 {
    let e = 43758.5453 * (91.731 * t + 7.3).sin();
    e - e.floor()
}

fn rnd(t: f64) -> f32 {
    fx_rand(t) as f32
}

// ───────────────────────────── line bags & sequences ─────────────────────────────

/// `pet.js pool()`: trim, drop empty, first-seen dedupe.
pub fn pool(raw: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for s in raw {
        let t = s.trim();
        if t.is_empty() || !seen.insert(t.to_string()) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

#[derive(Debug, Default, Clone)]
struct Bag {
    bag: Vec<String>,
    key: String,
    last: String,
    idx: Option<usize>,
}

/// Shuffle-bag line picker matching `pet.js:66-89`.
#[derive(Debug, Default, Clone)]
pub struct LineBags {
    bags: HashMap<String, Bag>,
}

impl LineBags {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last(&self, pool_key: &str) -> Option<&str> {
        self.bags.get(pool_key).map(|b| b.last.as_str())
    }

    /// `pet.js pick(e, t, n, o)`; also records `last` (JS `say()` does `bags[t].last = o`).
    pub fn pick(
        &mut self,
        pool_key: &str,
        items: &[String],
        no_repeat: bool,
        in_order: bool,
    ) -> Option<String> {
        let n = items.len();
        if n == 0 {
            return None;
        }
        let bag = self.bags.entry(pool_key.to_string()).or_default();
        if n == 1 {
            bag.last = items[0].clone();
            return Some(items[0].clone());
        }
        if in_order {
            let o: isize = match bag.idx {
                Some(i) if items.get(i).map(|s| *s == bag.last).unwrap_or(false) => i as isize,
                _ => items
                    .iter()
                    .position(|s| *s == bag.last)
                    .map(|i| i as isize)
                    .unwrap_or(-1),
            };
            let next = if o < 0 { 0 } else { ((o + 1) as usize) % n };
            bag.idx = Some(next);
            bag.last = items[next].clone();
            return Some(items[next].clone());
        }
        let mut rng = rand::thread_rng();
        if !no_repeat {
            let c = items[rng.gen_range(0..n)].clone();
            bag.last = c.clone();
            return Some(c);
        }
        let key = format!("{}|{}", n, items.join("\u{1}"));
        if bag.key != key {
            bag.key = key;
            bag.bag.clear();
        }
        if bag.bag.is_empty() {
            let mut e: Vec<String> = items.to_vec();
            for t in (1..e.len()).rev() {
                let k = rng.gen_range(0..=t);
                e.swap(t, k);
            }
            let last_i = e.len() - 1;
            if e[last_i] == bag.last {
                e.swap(last_i, 0);
            }
            bag.bag = e;
        }
        let c = bag.bag.pop();
        if let Some(c) = &c {
            bag.last = c.clone();
        }
        c
    }
}

/// `BubbleFx.splitSeq`: split on `//` (not `://`), trim, drop empties, max 12 segments.
pub fn split_seq(text: &str) -> Vec<String> {
    if !text.contains("//") {
        return vec![text.to_string()];
    }
    let b = text.as_bytes();
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'/' && (i == 0 || b[i - 1] != b':') {
            parts.push(&text[start..i]);
            i += 2;
            start = i;
        } else {
            i += 1;
        }
    }
    parts.push(&text[start..]);
    let out: Vec<String> = parts
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(12)
        .collect();
    if out.is_empty() {
        vec![text.to_string()]
    } else {
        out
    }
}

/// `BubbleFx.holdMs`.
pub fn hold_ms(text: &str, style: &ResolvedBubbleStyle, bubble_ms: f64, reduced: bool) -> f64 {
    let n = text.chars().filter(|c| !c.is_whitespace()).count() as f64;
    let fx = if reduced { TextFx::None } else { style.text_fx };
    let s = if style.speed_t > 0.0 {
        style.speed_t
    } else if style.speed > 0.0 {
        style.speed
    } else {
        1.0
    };
    let l = if style.speed_a > 0.0 {
        style.speed_a
    } else if style.speed > 0.0 {
        style.speed
    } else {
        1.0
    };
    let f = if reduced {
        200.0
    } else {
        let base = match style.appear {
            Appear::None => 0.0,
            Appear::Fade => 380.0,
            Appear::Rise => 420.0,
            Appear::Pop => 450.0,
            Appear::Grow => 200.0,
        };
        base / l.max(0.3)
    };
    let c = match fx {
        TextFx::Type => (120.0 + 70.0 * n) / s,
        TextFx::Blur => (100.0 + 70.0 * n + 280.0) / s,
        _ => 0.0,
    };
    let d = 8000f64.min(900.0 + 60.0 * n);
    let base_ms = if bubble_ms.is_finite() && bubble_ms != 0.0 { bubble_ms } else { 1500.0 };
    20000f64.min(base_ms.max(f + c + d)).round()
}

// ───────────────────────────── bubble style ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Box,
    Hand,
    Pixel,
    Caption,
    Focus,
    Recall,
    Dark,
    Dream,
    Paper,
    Shout,
}

impl Shape {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "box" => Shape::Box,
            "hand" => Shape::Hand,
            "pixel" => Shape::Pixel,
            "caption" => Shape::Caption,
            "focus" => Shape::Focus,
            "recall" => Shape::Recall,
            "dark" => Shape::Dark,
            "dream" => Shape::Dream,
            "paper" => Shape::Paper,
            "shout" => Shape::Shout,
            _ => return None,
        })
    }

    /// `BubbleFx.SPECIAL`: shapes whose wobble frames animate over time.
    pub fn is_special(self) -> bool {
        matches!(
            self,
            Shape::Focus | Shape::Recall | Shape::Dark | Shape::Dream | Shape::Paper | Shape::Shout
        )
    }

    /// `BubbleFx.EXTRA` padding `(x, y)` in CSS px (before the size factor `k`).
    pub fn extra_pad(self) -> (f64, f64) {
        match self {
            Shape::Box => (0.0, 0.0),
            Shape::Hand => (18.0, 6.0),
            Shape::Pixel => (4.0, 2.0),
            Shape::Caption => (2.0, -1.0),
            Shape::Focus => (18.0, 6.0),
            Shape::Recall => (18.0, 6.0),
            Shape::Dark => (20.0, 7.0),
            Shape::Dream => (20.0, 9.0),
            Shape::Paper => (10.0, 3.0),
            Shape::Shout => (22.0, 9.0),
        }
    }

    /// `(frame period in ms, frame count)`; static shapes report `(0, 1)`.
    pub fn wobble(self) -> (f64, u32) {
        match self {
            Shape::Box | Shape::Pixel | Shape::Caption => (0.0, 1),
            Shape::Hand => (120.0, 20),
            Shape::Focus => (100.0, 24),
            Shape::Recall => (120.0, 20),
            Shape::Dark => (200.0, 12),
            Shape::Dream => (240.0, 10),
            Shape::Paper => (160.0, 15),
            Shape::Shout => (100.0, 24),
        }
    }

    pub fn wobble_frames(self) -> u32 {
        self.wobble().1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Appear {
    Grow,
    Fade,
    Pop,
    Rise,
    None,
}

impl Appear {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "grow" => Appear::Grow,
            "fade" => Appear::Fade,
            "pop" => Appear::Pop,
            "rise" => Appear::Rise,
            "none" => Appear::None,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextFx {
    None,
    Type,
    Blur,
    Wave,
    Shake,
    Glitch,
}

impl TextFx {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => TextFx::None,
            "type" => TextFx::Type,
            "blur" => TextFx::Blur,
            "wave" => TextFx::Wave,
            "shake" => TextFx::Shake,
            "glitch" => TextFx::Glitch,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Frame {
    Default,
    Solid,
    Dashed,
    Double,
    None,
}

impl Frame {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "default" => Frame::Default,
            "solid" => Frame::Solid,
            "dashed" => Frame::Dashed,
            "double" => Frame::Double,
            "none" => Frame::None,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl TextAlign {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "left" => TextAlign::Left,
            "center" => TextAlign::Center,
            "right" => TextAlign::Right,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GradDir {
    V,
    H,
    D,
}

impl GradDir {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "v" => GradDir::V,
            "h" => GradDir::H,
            "d" => GradDir::D,
            _ => return None,
        })
    }
}

/// `BubbleStyle.PRESETS`: `(id, bg, fg, line)`.
pub const PRESETS: [(&str, &str, &str, &str); 7] = [
    ("dark", "#212121", "#e8e8e8", "#e8e8e824"),
    ("light", "#ffffff", "#1c1c1c", "#1c1c1c24"),
    ("amber", "#211d18", "#ece4d4", "#dfa74880"),
    ("night", "#1c1e24", "#e6e8ec", "#9bb8ff66"),
    ("paper", "#f3ead6", "#3a2f1f", "#8b3a2a55"),
    ("cool", "#ffffff", "#1e2733", "#41668f66"),
    ("black", "#000000", "#f2f2f2", "#f2f2f233"),
];

/// Looks up `(bg, fg, line)` for a preset id in [`PRESETS`].
pub fn preset_by_id(id: &str) -> Option<(&'static str, &'static str, &'static str)> {
    PRESETS
        .iter()
        .find(|(pid, _, _, _)| *pid == id)
        .map(|(_, bg, fg, line)| (*bg, *fg, *line))
}

/// Fully resolved bubble style (`BubbleStyle.resolve` merged with `BubbleFx.resolve`).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedBubbleStyle {
    pub preset: String,
    pub bg: String,
    pub fg: String,
    pub line: String,
    pub radius: f64,
    pub font: f64,
    pub size: f64,
    pub weight: u32,
    pub shadow: bool,
    pub tail: bool,
    pub gap: f64,
    pub align: TextAlign,
    pub family: String,
    pub grad: bool,
    pub bg2: String,
    pub gdir: GradDir,
    pub frame: Frame,
    pub line_w: f64,
    pub line_c: String,
    pub shape: Shape,
    pub appear: Appear,
    pub text_fx: TextFx,
    pub speed: f64,
    pub split: bool,
    pub speed_a: f64,
    pub speed_t: f64,
    pub speed_s: f64,
}

impl Default for ResolvedBubbleStyle {
    /// `BubbleStyle.DEFAULT` + `BubbleFx.MOTION_DEFAULT`.
    fn default() -> Self {
        Self {
            preset: "dark".into(),
            bg: "#212121".into(),
            fg: "#e8e8e8".into(),
            line: "#e8e8e824".into(),
            radius: 16.0,
            font: 14.0,
            size: 100.0,
            weight: 500,
            shadow: true,
            tail: true,
            gap: 12.0,
            align: TextAlign::Left,
            family: String::new(),
            grad: false,
            bg2: "#4a4a4a".into(),
            gdir: GradDir::D,
            frame: Frame::Default,
            line_w: 2.0,
            line_c: "#e8e8e8".into(),
            shape: Shape::Box,
            appear: Appear::Grow,
            text_fx: TextFx::None,
            speed: 1.0,
            split: false,
            speed_a: 1.0,
            speed_t: 1.0,
            speed_s: 1.0,
        }
    }
}

fn valid_hex(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() == 7 || b.len() == 9)
        && b[0] == b'#'
        && b[1..].iter().all(|c| c.is_ascii_hexdigit())
}

/// JS `Number(x)` for JSON values (null excluded by callers).
fn jnum(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse::<f64>().ok()?
            }
        }
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        _ => return None,
    };
    if n.is_finite() {
        Some(n)
    } else {
        None
    }
}

/// `BubbleFx.normOverride`: validated partial style.
#[derive(Clone, Debug, Default)]
struct StyleOverride {
    shape: Option<Shape>,
    appear: Option<Appear>,
    text_fx: Option<TextFx>,
    bg: Option<String>,
    fg: Option<String>,
    line: Option<String>,
    bg2: Option<String>,
    line_c: Option<String>,
    weight: Option<u32>,
    radius: Option<f64>,
    font: Option<f64>,
    size: Option<f64>,
    line_w: Option<f64>,
    speed: Option<f64>,
    speed_a: Option<f64>,
    speed_t: Option<f64>,
    speed_s: Option<f64>,
    split: Option<bool>,
    grad: Option<bool>,
    tail: Option<bool>,
    shadow: Option<bool>,
    gdir: Option<GradDir>,
    frame: Option<Frame>,
    align: Option<TextAlign>,
    family: Option<String>,
    preset: Option<String>,
}

impl StyleOverride {
    fn builtin(id: &str) -> Option<Self> {
        let mut o = StyleOverride::default();
        match id {
            "p_shout" => {
                o.shape = Some(Shape::Shout);
                o.appear = Some(Appear::Pop);
                o.text_fx = Some(TextFx::Shake);
                o.weight = Some(700);
            }
            "p_think" => {
                o.shape = Some(Shape::Dream);
                o.appear = Some(Appear::Fade);
                o.text_fx = Some(TextFx::Blur);
            }
            "p_recall" => {
                o.shape = Some(Shape::Recall);
                o.appear = Some(Appear::Fade);
                o.text_fx = Some(TextFx::None);
            }
            _ => return None,
        }
        Some(o)
    }

    fn from_json(v: &Value) -> Self {
        let mut o = StyleOverride::default();
        let Some(m) = v.as_object() else { return o };
        let st = |k: &str| m.get(k).and_then(|x| x.as_str());
        o.shape = st("shape").and_then(Shape::parse);
        o.appear = st("appear").and_then(Appear::parse);
        o.text_fx = st("textFx").and_then(TextFx::parse);
        let hex = |k: &str| st(k).filter(|s| valid_hex(s)).map(|s| s.to_lowercase());
        o.bg = hex("bg");
        o.fg = hex("fg");
        o.line = hex("line");
        o.bg2 = hex("bg2");
        o.line_c = hex("lineC");
        if let Some(w) = m.get("weight").and_then(jnum) {
            if w == 400.0 || w == 500.0 || w == 700.0 {
                o.weight = Some(w as u32);
            }
        }
        let num = |k: &str, lo: f64, hi: f64| -> Option<f64> {
            match m.get(k) {
                Some(x) if !x.is_null() => jnum(x).map(|n| round1(lim(n, lo, hi))),
                _ => None,
            }
        };
        o.radius = num("radius", 0.0, 28.0);
        o.font = num("font", 11.0, 22.0);
        o.size = num("size", 80.0, 160.0);
        o.line_w = num("lineW", 1.0, 8.0);
        o.speed = num("speed", 0.3, 2.0);
        o.speed_a = num("speedA", 0.3, 2.0);
        o.speed_t = num("speedT", 0.3, 2.0);
        o.speed_s = num("speedS", 0.3, 2.0);
        let bl = |k: &str| m.get(k).and_then(|x| x.as_bool());
        o.split = bl("split");
        o.grad = bl("grad");
        o.tail = bl("tail");
        o.shadow = bl("shadow");
        o.gdir = st("gdir").and_then(GradDir::parse);
        o.frame = st("frame").and_then(Frame::parse);
        o.align = st("align").and_then(TextAlign::parse);
        if let Some(f) = st("family") {
            let f = f.trim();
            let ok = f == "@file"
                || (!f.is_empty()
                    && f.chars().count() <= 80
                    && !f.chars().any(|c| matches!(c, '"' | '\'' | '`' | ';' | '{' | '}' | '<' | '>' | '\\')));
            if ok {
                o.family = Some(f.to_string());
            }
        }
        if let Some(p) = st("preset") {
            if !p.is_empty() && p.len() <= 20 && p.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
                o.preset = Some(p.to_string());
            }
        }
        o
    }

    fn apply(&self, s: &mut ResolvedBubbleStyle) {
        macro_rules! set {
            ($f:ident) => {
                if let Some(v) = &self.$f {
                    s.$f = v.clone();
                }
            };
        }
        set!(shape);
        set!(appear);
        set!(text_fx);
        set!(bg);
        set!(fg);
        set!(line);
        set!(bg2);
        set!(line_c);
        set!(weight);
        set!(radius);
        set!(font);
        set!(size);
        set!(line_w);
        set!(speed);
        set!(speed_a);
        set!(speed_t);
        set!(speed_s);
        set!(split);
        set!(grad);
        set!(tail);
        set!(shadow);
        set!(gdir);
        set!(frame);
        set!(align);
        set!(family);
        set!(preset);
    }
}

/// `BubbleFx.styleById` custom entries (`cfg.bubble_styles[].{id,name,o}`), validated like `normStyles`.
fn custom_override(cfg: &CharacterConfig, id: &str) -> Option<StyleOverride> {
    let valid_id = |s: &str| {
        s.len() >= 4
            && s.len() <= 18
            && s.starts_with("u_")
            && s[2..].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    };
    let mut seen: HashSet<&str> = HashSet::new();
    for entry in &cfg.bubble_styles {
        if seen.len() >= 24 {
            break;
        }
        let Some(eid) = entry.get("id").and_then(|x| x.as_str()) else { continue };
        if !valid_id(eid) || !seen.insert(eid) {
            continue;
        }
        if eid == id {
            return Some(StyleOverride::from_json(entry.get("o").unwrap_or(&Value::Null)));
        }
    }
    None
}

/// `cfg[pool_key]` raw line list for the known pools.
fn cfg_pool_lines<'a>(cfg: &'a CharacterConfig, pool_key: &str) -> Option<&'a [String]> {
    match pool_key {
        "lines" => Some(&cfg.lines),
        "idleLines" => Some(&cfg.idle_lines),
        "patLines" => Some(&cfg.pat_lines),
        _ => None,
    }
}

/// `BubbleFx.styleOfLine`: style id attached to a line in `raw_lines` (matched by trimmed text).
pub fn style_of_line_in(
    cfg: &CharacterConfig,
    pool_key: &str,
    raw_lines: &[String],
    text: &str,
) -> String {
    let ids = cfg.line_styles.get(pool_key);
    let want = text.trim();
    for (i, l) in raw_lines.iter().enumerate() {
        if l.trim() == want {
            return ids.and_then(|v| v.get(i)).cloned().unwrap_or_default();
        }
    }
    String::new()
}

/// `BubbleFx.styleOfLine`: style id attached to a line (matched by trimmed text and raw index).
pub fn style_of_line(cfg: &CharacterConfig, pool_key: &str, text: &str) -> String {
    let Some(raw) = cfg_pool_lines(cfg, pool_key) else { return String::new() };
    style_of_line_in(cfg, pool_key, raw, text)
}

impl ResolvedBubbleStyle {
    /// `BubbleStyle.resolve(cfg.bubbleStyle, ui.bubbleTheme)` then `BubbleFx.resolve` overrides.
    pub fn resolve(cfg: &CharacterConfig, style_id: &str, ui_theme: &str) -> Self {
        let mut s = ResolvedBubbleStyle::default();
        let mut base_ov: Option<StyleOverride> = None;
        if cfg.bubble_style.is_object() {
            let base = StyleOverride::from_json(&cfg.bubble_style);
            base.apply(&mut s);
            if let Some(g) = cfg
                .bubble_style
                .get("gap")
                .filter(|x| !x.is_null())
                .and_then(jnum)
            {
                s.gap = lim(g, 0.0, 60.0);
            }
            base_ov = Some(base);
        } else if ui_theme == "light" {
            s.bg = PRESETS[1].1.into();
            s.fg = PRESETS[1].2.into();
            s.line = PRESETS[1].3.into();
            s.preset = "light".into();
        }
        let mut line_ov: Option<StyleOverride> = None;
        if !style_id.is_empty() {
            let ov = StyleOverride::builtin(style_id).or_else(|| custom_override(cfg, style_id));
            if let Some(ov) = ov {
                ov.apply(&mut s);
                line_ov = Some(ov);
            }
        }
        // `BubbleFx.normMotion`
        let has_sa = base_ov.as_ref().is_some_and(|o| o.speed_a.is_some())
            || line_ov.as_ref().is_some_and(|o| o.speed_a.is_some());
        let has_st = base_ov.as_ref().is_some_and(|o| o.speed_t.is_some())
            || line_ov.as_ref().is_some_and(|o| o.speed_t.is_some());
        let has_ss = base_ov.as_ref().is_some_and(|o| o.speed_s.is_some())
            || line_ov.as_ref().is_some_and(|o| o.speed_s.is_some());
        s.speed = round1(lim(s.speed, 0.3, 2.0));
        s.speed_a = if !s.split || !has_sa { s.speed } else { round1(lim(s.speed_a, 0.3, 2.0)) };
        s.speed_t = if !s.split || !has_st { s.speed } else { round1(lim(s.speed_t, 0.3, 2.0)) };
        s.speed_s = if !s.split || !has_ss { s.speed } else { round1(lim(s.speed_s, 0.3, 2.0)) };
        s.gap = lim(s.gap, 0.0, 60.0);
        s.radius = lim(s.radius, 0.0, 28.0);
        s.font = lim(s.font, 11.0, 22.0);
        s.size = lim(s.size, 80.0, 160.0);
        s.line_w = lim(s.line_w, 1.0, 8.0);
        s
    }

    /// `BubbleStyle.k`: size factor `0.8..=1.6`.
    pub fn k(&self) -> f64 {
        lim(self.size, 80.0, 160.0) / 100.0
    }

    /// `BubbleStyle.fontPx`.
    pub fn font_px(&self) -> f64 {
        (self.font * self.k() * 2.0).round() / 2.0
    }

    /// `--bb-radius` in px.
    pub fn radius_px(&self) -> f64 {
        (self.radius * self.k()).round()
    }

    /// `BubbleFx.tailExtra`: additional gap needed for tails drawn outside the box.
    pub fn tail_extra(&self) -> f64 {
        if !self.tail {
            return 0.0;
        }
        match self.shape {
            Shape::Hand => 7.0,
            Shape::Dream => 24.0,
            Shape::Pixel => 2.0,
            _ => 0.0,
        }
    }

    pub fn is_bold(&self) -> bool {
        self.weight >= 700
    }
}

/// `BubbleFx.reduced(prefs)` (no OS query available natively).
pub fn reduced_motion(ui: &UiPrefs) -> bool {
    ui.reduce_motion
}

// ───────────────────────────── fonts, wrapping, box measuring ─────────────────────────────

struct LazyFont {
    path: PathBuf,
    cell: OnceLock<Option<Arc<Font>>>,
}

impl LazyFont {
    fn new(path: PathBuf) -> Self {
        Self { path, cell: OnceLock::new() }
    }

    fn get(&self) -> Option<&Font> {
        self.cell.get_or_init(|| load_font_file(&self.path)).as_deref()
    }
}

struct SharedFonts {
    regular: Vec<LazyFont>,
    bold: Vec<LazyFont>,
}

static SHARED_FONTS: OnceLock<SharedFonts> = OnceLock::new();

fn windows_fonts_dir() -> PathBuf {
    let base = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    base.join("Fonts")
}

fn load_font_file(path: &Path) -> Option<Arc<Font>> {
    let data = std::fs::read(path).ok()?;
    // `.ttc` collections: face 0 (Microsoft YaHei / Malgun regular).
    let settings = FontSettings { collection_index: 0, ..FontSettings::default() };
    Font::from_bytes(data, settings).ok().map(Arc::new)
}

fn shared_fonts() -> &'static SharedFonts {
    SHARED_FONTS.get_or_init(|| {
        let dir = windows_fonts_dir();
        let mk = |names: &[&str]| names.iter().map(|n| LazyFont::new(dir.join(n))).collect::<Vec<_>>();
        SharedFonts {
            regular: mk(&["segoeui.ttf", "malgun.ttf", "msyh.ttc"]),
            bold: mk(&["segoeuib.ttf", "malgunbd.ttf", "msyhbd.ttc"]),
        }
    })
}

/// Per-character font fallback stack: character font (optional) → shared Windows fonts.
///
/// Shared fonts are process-wide `OnceLock`s that are parsed lazily (the big CJK faces are only
/// loaded when a glyph is not found in an earlier face).
#[derive(Clone, Default)]
pub struct FontStack {
    custom: Option<Arc<LazyFont>>,
}

/// Which line breaking mode `BubbleFx.breakFor` selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakMode {
    /// CSS `word-break: keep-all`.
    KeepAll,
    /// CSS `word-break: normal` (CJK may wrap between ideographs).
    Normal,
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0xff00..=0xffef | 0x3000..=0x303f)
}

fn is_closing(c: char) -> bool {
    "、。，．！？：；」』）】〉》・…ー〜,.!?:;)]".contains(c)
        || matches!(c as u32, 0x3001 | 0x3002 | 0xff0c | 0xff01 | 0xff1f | 0xff1a | 0xff1b | 0xff09)
}

/// `BubbleFx.breakFor`.
pub fn break_for(text: &str) -> BreakMode {
    if text.chars().any(|c| matches!(c as u32, 0xac00..=0xd7af)) {
        BreakMode::KeepAll
    } else if text.chars().any(is_cjk) {
        BreakMode::Normal
    } else {
        BreakMode::KeepAll
    }
}

/// `[...text.trim()].length <= 3 && !text.includes("\n")` (`is-short` class).
pub fn is_short(text: &str) -> bool {
    text.trim().chars().count() <= 3 && !text.contains('\n')
}

/// CSS box metrics of the bubble element for a style/font size.
#[derive(Clone, Copy, Debug)]
pub struct BoxMetrics {
    pub pad_x: f64,
    pub pad_t: f64,
    pub pad_b: f64,
    pub border: f64,
    pub line_h: f64,
    pub min_w: f64,
    pub min_h: f64,
    pub radius: f64,
}

pub fn box_metrics(style: &ResolvedBubbleStyle, font_px: f64) -> BoxMetrics {
    let k = style.k();
    let (ex, ey) = style.shape.extra_pad();
    let radius = style.radius_px();
    BoxMetrics {
        pad_x: round1((13.0 + ex) * k),
        pad_t: round1((8.0 + ey) * k),
        pad_b: round1((9.0 + ey) * k),
        border: 1.0,
        line_h: font_px * 1.5,
        min_w: radius * 2.0 + 26.0,
        min_h: radius * 2.0 + 4.0,
        radius,
    }
}

/// Result of laying out text in a bubble box.
#[derive(Clone, Debug)]
pub struct BoxLayout {
    pub width: f64,
    pub height: f64,
    pub lines: Vec<String>,
    pub line_widths: Vec<f32>,
    pub metrics: BoxMetrics,
}

struct Unit {
    s: String,
    w: f32,
    space: bool,
}

const LETTER_SPACING_EM: f32 = -0.005;

impl FontStack {
    /// System fonts only.
    pub fn system() -> Self {
        Self { custom: None }
    }

    /// Loads `<char_dir>/<font_file>` lazily (only stats the file here; parsed on first glyph use).
    pub fn new(char_dir: &Path, font_file: &str) -> Self {
        let mut s = Self::system();
        let f = font_file.trim();
        if !f.is_empty() {
            let p = char_dir.join(f);
            if p.is_file() {
                s.custom = Some(Arc::new(LazyFont::new(p)));
            }
        }
        s
    }

    pub fn has_custom(&self) -> bool {
        self.custom.is_some()
    }

    pub fn is_custom_loaded(&self) -> bool {
        self.custom.as_ref().is_some_and(|l| l.cell.get().is_some())
    }

    /// First font in the stack that has a glyph for `ch`.
    pub fn font_for(&self, ch: char, bold: bool) -> Option<&Font> {
        if let Some(f) = self.custom.as_deref().and_then(LazyFont::get) {
            if f.lookup_glyph_index(ch) != 0 {
                return Some(f);
            }
        }
        let sh = shared_fonts();
        if bold {
            for l in &sh.bold {
                if let Some(f) = l.get() {
                    if f.lookup_glyph_index(ch) != 0 {
                        return Some(f);
                    }
                }
            }
        }
        for l in &sh.regular {
            if let Some(f) = l.get() {
                if f.lookup_glyph_index(ch) != 0 {
                    return Some(f);
                }
            }
        }
        None
    }

    fn primary(&self, bold: bool) -> Option<&Font> {
        if let Some(f) = self.custom.as_deref().and_then(LazyFont::get) {
            return Some(f);
        }
        let sh = shared_fonts();
        if bold {
            for l in &sh.bold {
                if let Some(f) = l.get() {
                    return Some(f);
                }
            }
        }
        for l in &sh.regular {
            if let Some(f) = l.get() {
                return Some(f);
            }
        }
        None
    }

    /// `(ascent, descent)` in px (descent is negative).
    pub fn line_metrics(&self, px: f32, bold: bool) -> (f32, f32) {
        self.primary(bold)
            .and_then(|f| f.horizontal_line_metrics(px))
            .map(|m| (m.ascent, m.descent))
            .unwrap_or((px * 0.9, -px * 0.25))
    }

    /// Horizontal advance of one character including CSS letter spacing.
    pub fn advance(&self, ch: char, px: f32, bold: bool) -> f32 {
        let ls = LETTER_SPACING_EM * px;
        if ch == '\t' {
            return self.advance(' ', px, bold) * 4.0;
        }
        let a = match self.font_for(ch, bold) {
            Some(f) => f.metrics(ch, px).advance_width,
            None => {
                if is_cjk(ch) || matches!(ch as u32, 0xac00..=0xd7af) {
                    px
                } else {
                    px * 0.5
                }
            }
        };
        a + ls
    }

    pub fn text_width(&self, s: &str, px: f32, bold: bool) -> f32 {
        s.chars().map(|c| self.advance(c, px, bold)).sum()
    }

    fn units(&self, line: &str, px: f32, bold: bool, mode: BreakMode) -> Vec<Unit> {
        let mut out: Vec<Unit> = Vec::new();
        let mut in_word = false;
        for c in line.chars() {
            let w = self.advance(c, px, bold);
            if c.is_whitespace() {
                out.push(Unit { s: c.to_string(), w, space: true });
                in_word = false;
                continue;
            }
            let last_non_space = matches!(out.last(), Some(u) if !u.space);
            if mode == BreakMode::Normal && last_non_space && is_closing(c) {
                let u = out.last_mut().unwrap();
                u.s.push(c);
                u.w += w;
            } else if mode == BreakMode::Normal && is_cjk(c) {
                out.push(Unit { s: c.to_string(), w, space: false });
                in_word = false;
            } else if in_word && last_non_space {
                let u = out.last_mut().unwrap();
                u.s.push(c);
                u.w += w;
            } else {
                out.push(Unit { s: c.to_string(), w, space: false });
                in_word = true;
            }
        }
        out
    }

    /// Greedy `white-space: pre-wrap` wrapping with `overflow-wrap: anywhere`.
    pub fn wrap(&self, text: &str, max_w: f32, px: f32, bold: bool, mode: BreakMode) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for hard in text.split('\n') {
            let units = self.units(hard, px, bold, mode);
            let mut cur = String::new();
            let mut total = 0f32;
            for u in units {
                if u.space {
                    cur.push_str(&u.s);
                    total += u.w;
                    continue;
                }
                if !cur.trim().is_empty() && total + u.w > max_w {
                    lines.push(cur.trim_end().to_string());
                    cur.clear();
                    total = 0.0;
                }
                if u.w > max_w {
                    for ch in u.s.chars() {
                        let cw = self.advance(ch, px, bold);
                        if !cur.is_empty() && total + cw > max_w {
                            lines.push(cur.trim_end().to_string());
                            cur.clear();
                            total = 0.0;
                        }
                        cur.push(ch);
                        total += cw;
                    }
                } else {
                    cur.push_str(&u.s);
                    total += u.w;
                }
            }
            lines.push(cur.trim_end().to_string());
        }
        lines
    }

    /// Lays text out inside a bubble whose CSS `max-width` is `max_w_dip` (border-box).
    pub fn layout_box(&self, text: &str, max_w_dip: f64, font_px: f64, style: &ResolvedBubbleStyle) -> BoxLayout {
        let m = box_metrics(style, font_px);
        let px = font_px as f32;
        let bold = style.is_bold();
        let max_w = max_w_dip.floor().max(1.0);
        let chrome_w = 2.0 * m.border + 2.0 * m.pad_x;
        let hard: Vec<&str> = text.split('\n').collect();
        let hard_w: Vec<f32> = hard.iter().map(|l| self.text_width(l.trim_end(), px, bold)).collect();
        let natural = hard_w.iter().cloned().fold(0f32, f32::max) as f64;
        let (lines, mut width) = if natural + chrome_w <= max_w {
            (hard.iter().map(|l| l.trim_end().to_string()).collect::<Vec<_>>(), natural + chrome_w)
        } else {
            let content = (max_w - chrome_w).max(10.0) as f32;
            (self.wrap(text, content, px, bold, break_for(text)), max_w)
        };
        width = width.max(m.min_w);
        let line_widths: Vec<f32> = lines.iter().map(|l| self.text_width(l, px, bold)).collect();
        let mut height = lines.len() as f64 * m.line_h + m.pad_t + m.pad_b + 2.0 * m.border;
        if is_short(text) {
            height = height.max(m.min_h);
        }
        BoxLayout { width: width.ceil(), height: height.ceil(), lines, line_widths, metrics: m }
    }

    /// `pet.js measure()`: `(width, height, lines)` of the bubble for `max_w_dip` / `font_px`.
    pub fn measure_box(
        &self,
        text: &str,
        max_w_dip: f64,
        font_px: f64,
        style: &ResolvedBubbleStyle,
    ) -> (f64, f64, Vec<String>) {
        let l = self.layout_box(text, max_w_dip, font_px, style);
        (l.width, l.height, l.lines)
    }
}

// ───────────────────────────── BubbleFit ─────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CharBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Head center X (`cx`); falls back to `x + w / 2`.
    pub cx: Option<f64>,
}

impl CharBox {
    /// Applies `cfg.bubble_x` / `cfg.bubble_y` (`pet.js placeBubble`), `pet_size` in DIP.
    pub fn with_bubble_offset(&self, bubble_x: f64, bubble_y: f64, pet_size: f64, flip: bool) -> CharBox {
        let e = pet_size / 200.0;
        let t = bubble_x * e * if flip { -1.0 } else { 1.0 };
        let n = bubble_y * e;
        CharBox {
            x: self.x,
            y: self.y + n,
            w: self.w,
            h: (self.h - n).max(1.0),
            cx: Some(self.cx.unwrap_or(self.x + self.w / 2.0) + t),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DipRect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TailSide {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BubblePlacement {
    pub left_dip: f64,
    pub top_dip: f64,
    pub width_dip: f64,
    pub height_dip: f64,
    pub font_px: f64,
    pub tail_side: TailSide,
    pub tail_at_dip: f64,
    pub clamp_lines: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct FitOptions {
    pub font: f64,
    pub fonts: Option<Vec<f64>>,
    pub gap: Option<f64>,
    pub avoid: Vec<DipRect>,
    pub avoid_soft: Vec<DipRect>,
}

#[derive(Clone, Copy)]
struct Region {
    l: f64,
    r: f64,
    t: f64,
    b: f64,
}

impl Region {
    fn size(&self) -> (f64, f64) {
        ((self.r - self.l).floor(), (self.b - self.t).floor())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Above,
    Right,
    Left,
    Below,
}

pub struct BubbleFit;

impl BubbleFit {
    pub const GAP: f64 = 12.0;
    pub const MARGIN: f64 = 8.0;
    pub const TAIL_INSET: f64 = 18.0;
    pub const WIDTHS: [f64; 3] = [260.0, 360.0, 480.0];
    pub const LINE_CAP: usize = 7;
    const MIN_W: f64 = 80.0;
    const MIN_H: f64 = 34.0;

    fn regions(c: &CharBox, a: &DipRect, gap: Option<f64>) -> [(Place, Region); 4] {
        let g = gap.unwrap_or(Self::GAP);
        let (l, r) = (a.left + Self::MARGIN, a.right - Self::MARGIN);
        let (t, b) = (a.top + Self::MARGIN, a.bottom - Self::MARGIN);
        [
            (Place::Above, Region { l, r, t, b: b.min(c.y - g) }),
            (Place::Right, Region { l: l.max(c.x + c.w + g), r, t, b }),
            (Place::Left, Region { l, r: r.min(c.x - g), t, b }),
            (Place::Below, Region { l, r, t: t.max(c.y + c.h + g), b }),
        ]
    }

    fn candidates(place: Place, n: (f64, f64), reg: &Region, c: &CharBox) -> Vec<BubblePlacement> {
        let cl = |v: f64, lo: f64, hi: f64| lo.max(hi.min(v));
        let i = c.cx.filter(|v| v.is_finite()).unwrap_or(c.x + c.w / 2.0);
        let r = c.y + (0.3 * c.h).min(48.0);
        let o = Self::TAIL_INSET;
        let mut out = Vec::new();
        let mut push = |left: f64, top: f64, side: TailSide, at: f64| {
            out.push(BubblePlacement {
                left_dip: left.round(),
                top_dip: top.round(),
                width_dip: n.0.round(),
                height_dip: n.1.round(),
                font_px: 0.0,
                tail_side: side,
                tail_at_dip: at.round(),
                clamp_lines: None,
            });
        };
        if place == Place::Above || place == Place::Below {
            let above = place == Place::Above;
            let top = if above { reg.b - n.1 } else { reg.t };
            let side = if above { TailSide::Bottom } else { TailSide::Top };
            for x in [i - n.0 / 2.0, i - n.0 + o + 6.0, i - o - 6.0] {
                let a = cl(x, reg.l, reg.r - n.0);
                let f = o.min(n.0 / 2.0);
                push(a, top, side, cl(i - a, f, n.0 - f));
            }
        } else {
            let right = place == Place::Right;
            let left = if right { reg.l } else { reg.r - n.0 };
            let side = if right { TailSide::Left } else { TailSide::Right };
            for y in [r - n.1 / 2.0, r - n.1 + o + 6.0, r - o - 6.0] {
                let a = cl(y, reg.t, reg.b - n.1);
                let f = o.min(n.1 / 2.0);
                push(left, a, side, cl(r - a, f, n.1 - f));
            }
        }
        out
    }

    fn overlaps(p: &BubblePlacement, avoid: &[DipRect]) -> bool {
        let o = 6.0;
        avoid.iter().any(|n| {
            p.left_dip < n.right + o
                && p.left_dip + p.width_dip > n.left - o
                && p.top_dip < n.bottom + o
                && p.top_dip + p.height_dip > n.top - o
        })
    }

    /// Exact port of `BubbleFit.plan`. `measure(max_width, font_px) -> (width, height)`.
    pub fn plan(
        char_box: &CharBox,
        allowed: &DipRect,
        widths: &[f64],
        line_cap: usize,
        opts: &FitOptions,
        measure: &mut dyn FnMut(f64, f64) -> (f64, f64),
    ) -> BubblePlacement {
        let s = if opts.font > 0.0 { opts.font } else { 14.0 };
        let fonts: Vec<f64> = opts
            .fonts
            .clone()
            .unwrap_or_else(|| vec![s, (s - 1.0).max(11.0), (s - 2.0).max(11.0)]);
        let regs = Self::regions(char_box, allowed, opts.gap);
        let reg_of = |p: Place| regs.iter().find(|(q, _)| *q == p).map(|(_, r)| *r).unwrap();
        let right_first = reg_of(Place::Right).size().0 >= reg_of(Place::Left).size().0;
        let mut order = vec![Place::Above];
        if right_first {
            order.extend([Place::Right, Place::Left]);
        } else {
            order.extend([Place::Left, Place::Right]);
        }
        order.push(Place::Below);

        let hard = &opts.avoid;
        let soft = &opts.avoid_soft;
        let mut sets: Vec<Vec<DipRect>> = Vec::new();
        if !hard.is_empty() || !soft.is_empty() {
            sets.push(hard.iter().chain(soft.iter()).cloned().collect());
        }
        if !hard.is_empty() && !soft.is_empty() {
            sets.push(hard.clone());
        }
        sets.push(Vec::new());

        for avoid in &sets {
            for &font in &fonts {
                let limit_h = (1.5 * font * line_cap as f64 + 18.0).ceil();
                let mut best: Option<BubblePlacement> = None;
                for &place in &order {
                    let reg = reg_of(place);
                    let sz = reg.size();
                    if sz.0 < Self::MIN_W || sz.1 < Self::MIN_H {
                        continue;
                    }
                    let mut ws: Vec<f64> = widths.iter().cloned().filter(|w| *w < sz.0).collect();
                    ws.push(sz.0);
                    for w in ws {
                        let e = measure(w, font);
                        if e.1 > sz.1 || e.0 > sz.0 {
                            continue;
                        }
                        let found = Self::candidates(place, e, &reg, char_box)
                            .into_iter()
                            .find(|p| !Self::overlaps(p, avoid));
                        if let Some(mut p) = found {
                            p.font_px = font;
                            if e.1 <= limit_h {
                                return p;
                            }
                            if best.as_ref().map(|b| p.height_dip < b.height_dip).unwrap_or(true) {
                                best = Some(p);
                            }
                        }
                    }
                }
                if let Some(p) = best {
                    return p;
                }
            }
        }

        // Fallback: largest region with the smallest font and a clamped line count.
        let y = *fonts.last().unwrap();
        let mut v: Option<(Place, f64, (f64, f64))> = None;
        for &place in &order {
            let sz = reg_of(place).size();
            if sz.0 < Self::MIN_W || sz.1 < Self::MIN_H {
                continue;
            }
            let area = sz.0 * sz.1;
            if v.as_ref().map(|b| area > b.1).unwrap_or(true) {
                v = Some((place, area, sz));
            }
        }
        let clamp_for = |height: f64, font: f64, avail: f64| -> Option<usize> {
            if height > avail {
                Some((((avail - 18.0) / (1.5 * font)).floor().max(1.0)) as usize)
            } else {
                None
            }
        };
        match v {
            None => {
                let h = Region {
                    l: allowed.left + 2.0,
                    r: allowed.right - 2.0,
                    t: allowed.top + 2.0,
                    b: allowed.bottom - 2.0,
                };
                let e = h.size();
                let i = measure(40f64.max(e.0), y);
                let size = (i.0.min(e.0).max(1.0), i.1.min(e.1).max(1.0));
                let mut r = Self::candidates(Place::Above, size, &h, char_box).remove(0);
                r.font_px = y;
                r.clamp_lines = clamp_for(i.1, y, e.1);
                r
            }
            Some((place, _, sz)) => {
                let a = reg_of(place);
                let i = measure(sz.0, y);
                let size = (i.0.min(sz.0), i.1.min(sz.1));
                let mut r = Self::candidates(place, size, &a, char_box).remove(0);
                r.font_px = y;
                r.clamp_lines = clamp_for(i.1, y, sz.1);
                r
            }
        }
    }

    /// `plan` with the default widths / line cap.
    pub fn plan_default(
        char_box: &CharBox,
        allowed: &DipRect,
        opts: &FitOptions,
        measure: &mut dyn FnMut(f64, f64) -> (f64, f64),
    ) -> BubblePlacement {
        Self::plan(char_box, allowed, &Self::WIDTHS, Self::LINE_CAP, opts, measure)
    }
}

// ───────────────────────────── runtime ─────────────────────────────

#[derive(Clone, Debug)]
pub struct BubbleFrameState {
    pub visible: bool,
    pub text: String,
    pub placement: BubblePlacement,
    pub style: ResolvedBubbleStyle,
    pub phase_idx: u32,
    pub appear_scale: f32,
    pub appear_alpha: f32,
    /// Extra vertical offset (DIP) of the `rise` appear animation to add to the sprite position.
    pub appear_dy_dip: f32,
    /// `Some(elapsed seconds)` while a text effect is animating (`None` when reduced / no effect).
    pub fx_elapsed_s: Option<f64>,
    pub shake_tick: u32,
    /// Hash of everything that changes the rasterized pixels (not the sprite position).
    pub dirty_key: u64,
}

struct Active {
    pool_key: String,
    line: String,
    segs: Vec<String>,
    seg_idx: usize,
    seg_start_s: f64,
    first_start_s: f64,
    hold_s: f64,
    style: ResolvedBubbleStyle,
    placement: BubblePlacement,
    inputs: (CharBox, DipRect),
    reduced: bool,
    hide_at_s: Option<f64>,
    ms_override: Option<f64>,
    bubble_ms: f64,
    extra_until_s: f64,
    needs_plan: bool,
    serial: u64,
}

pub struct BubbleRuntime {
    bags: LineBags,
    active: Option<Active>,
    idle_next_s: Option<f64>,
    idle_initialized: bool,
    serial: u64,
}

const FADE_OUT_S: f64 = 0.12;

impl Default for BubbleRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn kf(frames: &[(f64, f64)], p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    for w in frames.windows(2) {
        let (a, b) = (w[0], w[1]);
        if p <= b.0 {
            let t = if b.0 > a.0 { (p - a.0) / (b.0 - a.0) } else { 1.0 };
            let t = t * t * (3.0 - 2.0 * t);
            return a.1 + (b.1 - a.1) * t;
        }
    }
    frames.last().map(|f| f.1).unwrap_or(1.0)
}

/// `(scale, alpha, dy_dip)` of the appear animation at `t` seconds after a segment started.
fn appear_state(appear: Appear, reduced: bool, speed_a: f64, t: f64, first: bool) -> (f32, f32, f32) {
    let eff = if reduced {
        if appear == Appear::None {
            Appear::None
        } else {
            Appear::Fade
        }
    } else {
        appear
    };
    let r = 1.0 / (if reduced { 2.0 } else { speed_a }).max(0.3);
    let (s, a, dy) = match eff {
        Appear::None => (1.0, 1.0, 0.0),
        Appear::Grow => {
            let p = t / 0.2;
            let s = kf(&[(0.0, 0.9), (0.6, 1.03), (1.0, 1.0)], p);
            let a = if first { (t / 0.12).min(1.0) } else { 1.0 };
            (s, a, 0.0)
        }
        Appear::Fade => (1.0, (t / (0.38 * r)).clamp(0.0, 1.0), 0.0),
        Appear::Rise => {
            let p = (t / (0.42 * r)).clamp(0.0, 1.0);
            let a = kf(&[(0.0, 0.0), (0.6, 1.0), (1.0, 1.0)], p);
            (1.0, a, 14.0 * (1.0 - p).powi(3))
        }
        Appear::Pop => {
            let p = (t / (0.45 * r)).clamp(0.0, 1.0);
            let s = kf(&[(0.0, 0.3), (0.55, 1.12), (0.8, 0.96), (1.0, 1.0)], p);
            let a = kf(&[(0.0, 0.0), (0.55, 1.0), (1.0, 1.0)], p);
            (s, a, 0.0)
        }
    };
    (s as f32, a.clamp(0.0, 1.0) as f32, dy as f32)
}

impl BubbleRuntime {
    pub fn new() -> Self {
        Self {
            bags: LineBags::new(),
            active: None,
            idle_next_s: None,
            idle_initialized: false,
            serial: 0,
        }
    }

    pub fn bags(&self) -> &LineBags {
        &self.bags
    }

    /// `pet.js SAY_KEY`: pool key of the bubble currently shown (`None` once hidden).
    pub fn active_key(&self) -> Option<&str> {
        self.active
            .as_ref()
            .filter(|a| a.hide_at_s.is_none())
            .map(|a| a.pool_key.as_str())
    }

    /// Full text of the line currently shown (all `//` segments).
    pub fn active_line(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.line.as_str())
    }

    /// `true` while the bubble (including its 0.12 s fade-out) still needs drawing.
    pub fn is_visible(&self, now_s: f64) -> bool {
        match &self.active {
            None => false,
            Some(a) => match a.hide_at_s {
                Some(h) => now_s - h < FADE_OUT_S,
                None => true,
            },
        }
    }

    /// `pet.js hideBubble()` (fades out for 0.12 s).
    pub fn hide(&mut self, now_s: f64) {
        if let Some(a) = &mut self.active {
            if a.hide_at_s.is_none() {
                a.hide_at_s = Some(now_s);
            }
        }
    }

    /// Drops the bubble immediately (no fade).
    pub fn clear(&mut self) {
        self.active = None;
    }

    /// Keeps the final segment up until `until_s` + 0.3 s (`lineSound` duration).
    pub fn extend_hold_until(&mut self, until_s: f64) {
        if let Some(a) = &mut self.active {
            a.extra_until_s = until_s;
            if a.seg_idx + 1 == a.segs.len() {
                a.hold_s = Self::seg_hold_s(a);
            }
        }
    }

    fn seg_hold_s(a: &Active) -> f64 {
        let text = &a.segs[a.seg_idx];
        let last = a.seg_idx + 1 == a.segs.len();
        let mut m = hold_ms(text, &a.style, a.bubble_ms, a.reduced);
        if let Some(ms) = a.ms_override {
            if ms > 0.0 {
                m = m.max(ms);
            }
        }
        let mut h = if last {
            m
        } else {
            let n = text.chars().count() as f64;
            m.min(900f64.max(2400f64.min(600.0 + 90.0 * n)))
        };
        if last && a.extra_until_s > 0.0 {
            h = h.max((a.extra_until_s - a.seg_start_s) * 1000.0 + 300.0);
        }
        h / 1000.0
    }

    fn plan_for(
        text: &str,
        style: &ResolvedBubbleStyle,
        char_box: &CharBox,
        allowed: &DipRect,
        fonts: &FontStack,
    ) -> BubblePlacement {
        let font = style.font_px();
        let opts = FitOptions {
            font,
            fonts: None,
            gap: Some(style.gap + style.tail_extra()),
            avoid: Vec::new(),
            avoid_soft: Vec::new(),
        };
        let mut m = |w: f64, f: f64| {
            let (bw, bh, _) = fonts.measure_box(text, w, f, style);
            (bw, bh)
        };
        let mut p = BubbleFit::plan_default(char_box, allowed, &opts, &mut m);
        // `plan` clamps by the line cap formula when the box did not fit; keep that value.
        if p.clamp_lines.is_none() {
            let (_, h, _) = fonts.measure_box(text, p.width_dip, p.font_px, style);
            if h > p.height_dip + 0.5 {
                let lines = (((p.height_dip - 18.0) / (1.5 * p.font_px)).floor().max(1.0)) as usize;
                p.clamp_lines = Some(lines);
            }
        }
        p
    }

    #[allow(clippy::too_many_arguments)]
    fn begin(
        &mut self,
        pool_key: &str,
        line: &str,
        style: ResolvedBubbleStyle,
        reduced: bool,
        ms_override: Option<f64>,
        cfg: &CharacterConfig,
        char_box: &CharBox,
        allowed: &DipRect,
        fonts: &FontStack,
        now_s: f64,
    ) {
        self.serial += 1;
        let segs = split_seq(line);
        let placement = Self::plan_for(&segs[0], &style, char_box, allowed, fonts);
        let mut a = Active {
            pool_key: pool_key.to_string(),
            line: line.to_string(),
            segs,
            seg_idx: 0,
            seg_start_s: now_s,
            first_start_s: now_s,
            hold_s: 0.0,
            style,
            placement,
            inputs: (*char_box, *allowed),
            reduced,
            hide_at_s: None,
            ms_override,
            bubble_ms: cfg.bubble_ms,
            extra_until_s: 0.0,
            needs_plan: false,
            serial: self.serial,
        };
        a.hold_s = Self::seg_hold_s(&a);
        self.active = Some(a);
    }

    /// `pet.js say(poolKey)`: picks a line from `cfg`'s pool (`"lines"`, `"idleLines"`,
    /// `"patLines"`) and shows it.
    ///
    /// `char_box` must already include the `bubbleX/Y` offsets ([`CharBox::with_bubble_offset`]).
    /// Returns the chosen line (so callers can trigger line sounds), or `None` when nothing is shown.
    #[allow(clippy::too_many_arguments)]
    pub fn say(
        &mut self,
        pool_key: &str,
        cfg: &CharacterConfig,
        ui_prefs: &UiPrefs,
        char_box: &CharBox,
        allowed_box: &DipRect,
        fonts: &FontStack,
        now_s: f64,
    ) -> Option<String> {
        let lines = cfg_pool_lines(cfg, pool_key).unwrap_or(&[]);
        self.say_from_pool(pool_key, cfg, lines, ui_prefs, char_box, allowed_box, fonts, now_s)
    }

    /// Picks a line from an explicit `lines` slice (e.g. `sp_*` pools) and shows it.
    #[allow(clippy::too_many_arguments)]
    pub fn say_from_pool(
        &mut self,
        pool_key: &str,
        cfg: &CharacterConfig,
        lines: &[String],
        ui_prefs: &UiPrefs,
        char_box: &CharBox,
        allowed_box: &DipRect,
        fonts: &FontStack,
        now_s: f64,
    ) -> Option<String> {
        if !cfg.bubble {
            self.hide(now_s);
            return None;
        }
        let items = pool(lines);
        if items.is_empty() {
            self.hide(now_s);
            return None;
        }
        let line = self.bags.pick(pool_key, &items, cfg.no_repeat, cfg.in_order)?;
        let raw: &[String] = cfg_pool_lines(cfg, pool_key).unwrap_or(lines);
        let style_id = style_of_line_in(cfg, pool_key, raw, &line);
        let style = ResolvedBubbleStyle::resolve(cfg, &style_id, &ui_prefs.bubble_theme);
        let reduced = reduced_motion(ui_prefs);
        self.begin(pool_key, &line, style, reduced, None, cfg, char_box, allowed_box, fonts, now_s);
        Some(line)
    }

    /// `pet.js sayText` (reminders etc.): shows `text` with optional minimum `ms` and the style
    /// attached to `src` (defaults to `text`) in `style_pool_key`.
    #[allow(clippy::too_many_arguments)]
    pub fn say_text(
        &mut self,
        text: &str,
        ms: Option<f64>,
        style_pool_key: Option<&str>,
        style_src: Option<&str>,
        cfg: &CharacterConfig,
        ui_prefs: &UiPrefs,
        char_box: &CharBox,
        allowed_box: &DipRect,
        fonts: &FontStack,
        now_s: f64,
    ) {
        if !cfg.bubble {
            self.hide(now_s);
            return;
        }
        let style_id = match style_pool_key {
            Some(k) if !k.is_empty() => style_of_line(cfg, k, style_src.unwrap_or(text)),
            _ => String::new(),
        };
        let style = ResolvedBubbleStyle::resolve(cfg, &style_id, &ui_prefs.bubble_theme);
        let reduced = reduced_motion(ui_prefs);
        self.begin("__raw", text, style, reduced, ms, cfg, char_box, allowed_box, fonts, now_s);
    }

    /// `pet.js scheduleIdle()`: `idle_ms * (0.6 + 0.8 * random)`, only if `idle_ms >= 1000` and
    /// there are idle lines. Call after a character loads and after each idle bubble.
    pub fn reschedule_idle(&mut self, cfg: &CharacterConfig, now_s: f64) {
        self.idle_initialized = true;
        if cfg.idle_ms < 1000.0 || pool(&cfg.idle_lines).is_empty() {
            self.idle_next_s = None;
            return;
        }
        let f = 0.6 + 0.8 * rand::thread_rng().gen::<f64>();
        self.idle_next_s = Some(now_s + cfg.idle_ms * f / 1000.0);
    }

    /// Polls the idle timer: returns `true` when the caller should `say("idleLines", ..)`
    /// (`busy` = pet is being pressed/resting). Auto-initializes on first poll.
    pub fn poll_idle(&mut self, cfg: &CharacterConfig, now_s: f64, busy: bool) -> bool {
        if !self.idle_initialized {
            self.reschedule_idle(cfg, now_s);
            return false;
        }
        match self.idle_next_s {
            Some(t) if now_s >= t => {
                self.reschedule_idle(cfg, now_s);
                !busy
            }
            _ => false,
        }
    }

    /// Advances the segment sequence / fade-out and returns the frame to draw.
    ///
    /// Re-plans the placement when `char_box` / `allowed_box` changed. Re-rasterize and upload the
    /// texture only when `dirty_key` differs from the previous frame; `placement.left_dip/top_dip`
    /// only move the sprite.
    pub fn step(
        &mut self,
        now_s: f64,
        char_box: &CharBox,
        allowed_box: &DipRect,
        fonts: &FontStack,
    ) -> Option<BubbleFrameState> {
        let a = self.active.as_mut()?;
        if let Some(h) = a.hide_at_s {
            if now_s - h >= FADE_OUT_S {
                self.active = None;
                return None;
            }
        } else if now_s >= a.seg_start_s + a.hold_s {
            if a.seg_idx + 1 < a.segs.len() {
                a.seg_idx += 1;
                a.seg_start_s += a.hold_s;
                if now_s - a.seg_start_s > 2.0 {
                    a.seg_start_s = now_s;
                }
                a.hold_s = Self::seg_hold_s(a);
                a.needs_plan = true;
                self.serial += 1;
                a.serial = self.serial;
            } else {
                a.hide_at_s = Some(now_s);
            }
        }
        let text = a.segs[a.seg_idx].clone();
        let changed = a.needs_plan || a.inputs.0 != *char_box || a.inputs.1 != *allowed_box;
        if changed {
            a.placement = Self::plan_for(&text, &a.style, char_box, allowed_box, fonts);
            a.inputs = (*char_box, *allowed_box);
            a.needs_plan = false;
        }
        let t = (now_s - a.seg_start_s).max(0.0);
        let first = a.seg_idx == 0;
        let seg_t = if first { now_s - a.first_start_s } else { t };
        let (mut scale, mut alpha, dy) =
            appear_state(a.style.appear, a.reduced, a.style.speed_a, seg_t.max(0.0), first);
        if let Some(h) = a.hide_at_s {
            let u = ((now_s - h) / FADE_OUT_S).clamp(0.0, 1.0);
            alpha = (1.0 - u) as f32;
            scale = (1.0 - 0.06 * u) as f32;
        }

        let style = &a.style;
        let (period, frames) = style.shape.wobble();
        let phase_idx = if a.reduced || !style.shape.is_special() || period <= 0.0 {
            0
        } else {
            ((t * 1000.0 * style.speed_s / period).floor() as u64 % frames as u64) as u32
        };
        let fx = if a.reduced { TextFx::None } else { style.text_fx };
        let r_ms = t * 1000.0 * style.speed_t;
        let n_chars = text.chars().filter(|c| !c.is_whitespace()).count() as f64;
        let (fx_elapsed_s, shake_tick, fx_state) = match fx {
            TextFx::None => (None, 0u32, 0u64),
            TextFx::Type => {
                let shown = ((r_ms - 120.0).max(0.0) / 70.0).floor() as u64;
                let max_s = n_chars as u64 + 41;
                let blink = if shown < max_s {
                    ((t * 1000.0 / 450.0).floor() as u64) & 1
                } else {
                    2
                };
                (Some(t), 0, (shown.min(max_s) << 2) | blink)
            }
            TextFx::Blur => {
                let done = r_ms > 100.0 + 70.0 * n_chars + 300.0;
                (Some(t), 0, if done { u64::MAX } else { (r_ms / 33.0).floor() as u64 })
            }
            TextFx::Wave => (Some(t), 0, (r_ms / 33.0).floor() as u64),
            TextFx::Shake => {
                let tick = (r_ms / 50.0).floor() as u32;
                (Some(t), tick, tick as u64)
            }
            TextFx::Glitch => (Some(t), 0, (r_ms / 40.0).floor() as u64),
        };

        let p = &a.placement;
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        a.serial.hash(&mut h);
        (p.width_dip.round() as i64, p.height_dip.round() as i64).hash(&mut h);
        ((p.font_px * 2.0).round() as i64, p.tail_side, p.tail_at_dip.round() as i64, p.clamp_lines).hash(&mut h);
        (phase_idx, (alpha * 32.0).round() as i32, (scale * 64.0).round() as i32, fx_state, shake_tick).hash(&mut h);

        Some(BubbleFrameState {
            visible: true,
            text,
            placement: a.placement.clone(),
            style: a.style.clone(),
            phase_idx,
            appear_scale: scale,
            appear_alpha: alpha,
            appear_dy_dip: dy,
            fx_elapsed_s,
            shake_tick,
            dirty_key: h.finish(),
        })
    }
}

// ───────────────────────────── rasterizer ─────────────────────────────

/// Transparent margin (DIP) around the bubble box in every sprite so shadows, wobbling strokes
/// and tails never clip. `rasterize_*` return this as `(pad_left_dip, pad_top_dip)`.
pub const SPRITE_PAD_DIP: f64 = 48.0;
const REMIND_PAD_DIP: f32 = 40.0;
const PI32: f32 = std::f32::consts::PI;
const TAU32: f32 = std::f32::consts::PI * 2.0;

type Rgba = [f32; 4];
type Pt = (f32, f32);

fn parse_col(s: &str, fb: Rgba) -> Rgba {
    if !s.is_ascii() {
        return fb;
    }
    parse_hex_color_rgba(s).unwrap_or(fb)
}

fn to_color(c: Rgba, opacity: f32) -> sk::Color {
    sk::Color::from_rgba(
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
        (c[3] * opacity).clamp(0.0, 1.0),
    )
    .unwrap_or(sk::Color::BLACK)
}

fn solid_paint(c: Rgba, opacity: f32, aa: bool) -> sk::Paint<'static> {
    let mut p = sk::Paint::default();
    p.set_color(to_color(c, opacity));
    p.anti_alias = aa;
    p
}

fn mk_stroke(w: f32, join: sk::LineJoin) -> sk::Stroke {
    let mut s = sk::Stroke::default();
    s.width = w.max(0.05);
    s.line_join = join;
    s.line_cap = sk::LineCap::Round;
    s
}

#[derive(Clone)]
enum FillSpec {
    Solid(Rgba),
    Grad { a: Rgba, b: Rgba, dir: GradDir, w: f32, h: f32 },
}

fn fill_paint(spec: &FillSpec, opacity: f32, aa: bool) -> sk::Paint<'static> {
    match spec {
        FillSpec::Solid(c) => solid_paint(*c, opacity, aa),
        FillSpec::Grad { a, b, dir, w, h } => {
            let (p0, p1) = match dir {
                GradDir::V => ((0.0, 0.0), (0.0, *h)),
                GradDir::H => ((0.0, 0.0), (*w, 0.0)),
                GradDir::D => ((0.0, 0.0), (*w, *h)),
            };
            let stops = vec![
                sk::GradientStop::new(0.0, to_color(*a, opacity)),
                sk::GradientStop::new(1.0, to_color(*b, opacity)),
            ];
            let shader = sk::LinearGradient::new(
                sk::Point::from_xy(p0.0, p0.1),
                sk::Point::from_xy(p1.0, p1.1),
                stops,
                sk::SpreadMode::Pad,
                sk::Transform::identity(),
            );
            match shader {
                Some(sh) => {
                    let mut p = sk::Paint::default();
                    p.shader = sh;
                    p.anti_alias = aa;
                    p
                }
                None => solid_paint(*a, opacity, aa),
            }
        }
    }
}

fn poly_path(pts: &[Pt], close: bool) -> Option<sk::Path> {
    if pts.len() < 2 {
        return None;
    }
    let mut pb = sk::PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

fn ellipse_pts(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<Pt> {
    let (rx, ry) = (rx.max(0.5), ry.max(0.5));
    let per = PI32 * (3.0 * (rx + ry) - ((3.0 * rx + ry) * (rx + 3.0 * ry)).sqrt());
    let n = ((per / 1.5).ceil() as usize).clamp(32, 720);
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * TAU32;
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect()
}

fn subdivide(pts: &[Pt], closed: bool, step: f32) -> Vec<Pt> {
    let mut out = Vec::with_capacity(pts.len() * 2);
    let n = pts.len();
    if n == 0 {
        return out;
    }
    let edges = if closed { n } else { n - 1 };
    for i in 0..edges {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let k = ((len / step).ceil() as usize).max(1);
        for j in 0..k {
            let t = j as f32 / k as f32;
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
        }
    }
    if !closed {
        out.push(pts[n - 1]);
    }
    out
}

fn rect_pts(x: f32, y: f32, w: f32, h: f32) -> Vec<Pt> {
    subdivide(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], true, 1.5)
}

fn hash2(ix: i32, iy: i32, seed: u32) -> f32 {
    let mut h = (ix as u32).wrapping_mul(0x9E37_79B1)
        ^ (iy as u32).wrapping_mul(0x85EB_CA77)
        ^ seed.wrapping_mul(0xC2B2_AE3D).wrapping_add(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h as f32 / u32::MAX as f32
}

fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash2(ix, iy, seed);
    let b = hash2(ix + 1, iy, seed);
    let c = hash2(ix, iy + 1, seed);
    let d = hash2(ix + 1, iy + 1, seed);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 0.5, 0.0, 1.0);
    for o in 0..octaves.max(1) {
        sum += amp * vnoise(x * f, y * f, seed.wrapping_add(o * 101));
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

/// Emulates `feTurbulence` + `feDisplacementMap` by moving densely sampled geometry points.
fn displace(pts: &[Pt], closed: bool, freq: f32, scale: f32, octaves: u32, seed: u32) -> Vec<Pt> {
    subdivide(pts, closed, 3.0)
        .into_iter()
        .map(|(x, y)| {
            let nx = fbm(x * freq, y * freq, seed.wrapping_mul(2).wrapping_add(1), octaves);
            let ny = fbm(x * freq, y * freq, seed.wrapping_mul(2).wrapping_add(2), octaves);
            (x + scale * (nx - 0.5) * 1.5, y + scale * (ny - 0.5) * 1.5)
        })
        .collect()
}

fn quad_pts(p0: Pt, c: Pt, p1: Pt, n: usize, out: &mut Vec<Pt>) {
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        out.push((
            u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
            u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
        ));
    }
}

fn cubic_pts(p0: Pt, c1: Pt, c2: Pt, p1: Pt, n: usize, out: &mut Vec<Pt>) {
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        out.push((
            u * u * u * p0.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * p1.0,
            u * u * u * p0.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * p1.1,
        ));
    }
}

fn arc_pts(cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32, n: usize, out: &mut Vec<Pt>) {
    for i in 0..=n {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        out.push((cx + rx * a.cos(), cy + ry * a.sin()));
    }
}

/// `BubbleFx` `T()`: `(x, y, dx, dy)` of the tail anchor at distance `n` outside the `side` edge.
fn tail_vec(side: TailSide, w: f32, h: f32, a: f32, n: f32) -> (f32, f32, f32, f32) {
    match side {
        TailSide::Top => (a, -n, 0.0, -1.0),
        TailSide::Left => (-n, a, -1.0, 0.0),
        TailSide::Right => (w + n, a, 1.0, 0.0),
        TailSide::Bottom => (a, h + n, 0.0, 1.0),
    }
}

/// `BubbleShape.pathOf`: rounded rect with a triangular tail; `inset` shrinks for strokes.
fn box_path(w: f32, h: f32, r: f32, side: TailSide, at: f32, tail: bool, inset: f32) -> Option<sk::Path> {
    let l = inset;
    let (d, c) = (l, l);
    let (p, u) = (w - l, h - l);
    if p <= d + 1.0 || u <= c + 1.0 {
        return None;
    }
    let hh = 1f32.max((r - l).min((p - d) / 2.0).min((u - c) / 2.0));
    let f = 9.0f32;
    let b = 9.0 / 9f32.hypot(f);
    let m = 9.0 + l - l / b;
    let g = 1.5f32.max(m * f / 9.0);
    let y = tail && m > 1.0;
    let wf = |t: f32, e: f32, o: f32| (e + hh + g).max((o - hh - g).min(t));
    let mut pb = sk::PathBuilder::new();
    pb.move_to(d + hh, c);
    if y && side == TailSide::Top {
        let t = wf(at, d, p);
        pb.line_to(t - g, c);
        pb.line_to(t, c - m);
        pb.line_to(t + g, c);
    }
    pb.line_to(p - hh, c);
    pb.quad_to(p, c, p, c + hh);
    if y && side == TailSide::Right {
        let t = wf(at, c, u);
        pb.line_to(p, t - g);
        pb.line_to(p + m, t);
        pb.line_to(p, t + g);
    }
    pb.line_to(p, u - hh);
    pb.quad_to(p, u, p - hh, u);
    if y && side == TailSide::Bottom {
        let t = wf(at, d, p);
        pb.line_to(t + g, u);
        pb.line_to(t, u + m);
        pb.line_to(t - g, u);
    }
    pb.line_to(d + hh, u);
    pb.quad_to(d, u, d, u - hh);
    if y && side == TailSide::Left {
        let t = wf(at, c, u);
        pb.line_to(d, t + g);
        pb.line_to(d - m, t);
        pb.line_to(d, t - g);
    }
    pb.line_to(d, c + hh);
    pb.quad_to(d, c, d + hh, c);
    pb.close();
    pb.finish()
}

fn rr_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<sk::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = sk::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// `BubbleShape.I()`: faint (< 0x99 alpha) line colors become opaque for freeform shapes.
fn opaque_if_faint(line: &str, fg: &str) -> Rgba {
    let c = parse_col(line, parse_col(fg, [0.91, 0.91, 0.91, 1.0]));
    if line.len() == 9 && c[3] < 153.0 / 255.0 {
        [c[0], c[1], c[2], 1.0]
    } else {
        c
    }
}

struct ShapeCtx<'a> {
    style: &'a ResolvedBubbleStyle,
    w: f32,
    h: f32,
    side: TailSide,
    at: f32,
    phase: u32,
    fill: FillSpec,
    line: Rgba,
    cm: f32,
    ts: sk::Transform,
}

impl ShapeCtx<'_> {
    fn fill_poly(&self, pm: &mut sk::Pixmap, pts: &[Pt], close: bool, opacity: f32, aa: bool) {
        if let Some(path) = poly_path(pts, close) {
            pm.fill_path(&path, &fill_paint(&self.fill, opacity, aa), sk::FillRule::Winding, self.ts, None);
        }
    }

    fn stroke_poly(
        &self,
        pm: &mut sk::Pixmap,
        pts: &[Pt],
        close: bool,
        color: Rgba,
        opacity: f32,
        width: f32,
        join: sk::LineJoin,
        aa: bool,
    ) {
        if let Some(path) = poly_path(pts, close) {
            pm.stroke_path(&path, &solid_paint(color, opacity, aa), &mk_stroke(width, join), self.ts, None);
        }
    }

    fn fill_solid_poly(&self, pm: &mut sk::Pixmap, pts: &[Pt], color: Rgba, opacity: f32) {
        if let Some(path) = poly_path(pts, true) {
            pm.fill_path(&path, &solid_paint(color, opacity, true), sk::FillRule::Winding, self.ts, None);
        }
    }

    fn frames(&self) -> f32 {
        self.phase.min(self.style.shape.wobble_frames().saturating_sub(1)) as f32
    }

    fn draw(&self, pm: &mut sk::Pixmap) {
        match self.style.shape {
            Shape::Box => self.draw_box(pm),
            Shape::Hand => self.draw_hand(pm),
            Shape::Pixel => self.draw_pixel(pm),
            Shape::Caption => self.draw_caption(pm),
            Shape::Focus => self.draw_focus(pm),
            Shape::Recall => self.draw_recall(pm),
            Shape::Dark => self.draw_dark(pm),
            Shape::Dream => self.draw_dream(pm),
            Shape::Paper => self.draw_paper(pm),
            Shape::Shout => self.draw_shout(pm),
        }
    }

    fn draw_box(&self, pm: &mut sk::Pixmap) {
        let s = self.style;
        let r = s.radius_px() as f32;
        let k = s.line_w.clamp(1.0, 8.0) as f32;
        let (w, h) = (self.w, self.h);
        if let Some(path) = box_path(w, h, r, self.side, self.at, s.tail, 0.0) {
            pm.fill_path(&path, &fill_paint(&self.fill, 1.0, true), sk::FillRule::Winding, self.ts, None);
        }
        let paint_line = |pm: &mut sk::Pixmap, inset: f32, width: f32, dash: bool, col: Rgba| {
            if let Some(path) = box_path(w, h, r, self.side, self.at, s.tail, inset) {
                let mut st = mk_stroke(width, sk::LineJoin::Round);
                if dash {
                    st.dash = sk::StrokeDash::new(vec![5.0 * k, 3.0 * k], 0.0);
                }
                pm.stroke_path(&path, &solid_paint(col, 1.0, true), &st, self.ts, None);
            }
        };
        let fg = parse_col(&s.fg, [0.91, 0.91, 0.91, 1.0]);
        let line_c = parse_col(&s.line_c, fg);
        match s.frame {
            Frame::Default => {
                let c = parse_col(&s.line, [0.91, 0.91, 0.91, 0.14]);
                paint_line(pm, 0.5, 1.0, false, c);
            }
            Frame::Solid => paint_line(pm, 0.0, k, false, line_c),
            Frame::Dashed => paint_line(pm, 0.0, k, true, line_c),
            Frame::Double => {
                let t = (0.6 * k).max(1.0);
                paint_line(pm, 0.0, t, false, line_c);
                paint_line(pm, 4.0 + t, t, false, line_c);
            }
            Frame::None => {}
        }
    }

    fn draw_hand(&self, pm: &mut sk::Pixmap) {
        let t = self.frames();
        let (seed1, seed2) = ((3.0 + 7.0 * t) as u32, (9.0 + 5.0 * t) as u32);
        let (w, h) = (self.w, self.h);
        let (p, u) = (w / 2.0, h / 2.0);
        let (hw, hh) = (w / 2.0, h / 2.0);
        let cm = self.cm;
        if self.style.tail {
            let (tx, ty, dx, dy) = tail_vec(self.side, w, h, self.at, 13.0);
            let x = if dx != 0.0 { p + dx * (hw - 4.0) } else { (p - 0.6 * hw).max((p + 0.6 * hw).min(self.at)) };
            let y = if dy != 0.0 { u + dy * (hh - 3.0) } else { (u - 0.5 * hh).max((u + 0.5 * hh).min(self.at)) };
            let b = if dy != 0.0 { 6.0 } else { 0.0 };
            let g = if dx != 0.0 { 6.0 } else { 0.0 };
            let p0 = (x - b, y - g);
            let tip = (tx, ty);
            let mut pts = vec![p0];
            quad_pts(p0, (x + 0.4 * (tx - x) - 0.3 * g, y + 0.5 * (ty - y) - 0.3 * b), tip, 10, &mut pts);
            quad_pts(
                tip,
                (x + 0.3 * (tx - x) + 0.6 * b, y + 0.4 * (ty - y) + 0.6 * g),
                (x + 1.2 * b, y + 1.2 * g),
                10,
                &mut pts,
            );
            let pts = displace(&pts, false, 0.035, 3.5, 2, seed1);
            self.fill_poly(pm, &pts, true, 1.0, true);
            self.stroke_poly(pm, &pts, false, self.line, 1.0, 1.3 * cm, sk::LineJoin::Round, true);
        }
        let main = displace(&ellipse_pts(p, u, hw, hh), true, 0.035, 3.5, 2, seed1);
        self.fill_poly(pm, &main, true, 1.0, true);
        self.stroke_poly(pm, &main, true, self.line, 1.0, 1.4 * cm, sk::LineJoin::Round, true);
        let inner = displace(&ellipse_pts(p + 1.2, u - 0.8, hw - 0.5, hh - 0.5), true, 0.05, 4.0, 2, seed2);
        self.stroke_poly(pm, &inner, true, self.line, 0.4, 0.9 * cm, sk::LineJoin::Round, true);
    }

    fn draw_pixel(&self, pm: &mut sk::Pixmap) {
        let (e, o) = (self.w, self.h);
        let t = 4.0f32;
        let (r, i, s, l) = (0.5, 0.5, e - 0.5, o - 0.5);
        let mut p: Vec<Pt> = vec![
            (r + 2.0 * t, i),
            (s - 2.0 * t, i),
            (s - 2.0 * t, i + t),
            (s - t, i + t),
            (s - t, i + 2.0 * t),
            (s, i + 2.0 * t),
            (s, l - 2.0 * t),
            (s - t, l - 2.0 * t),
            (s - t, l - t),
            (s - 2.0 * t, l - t),
            (s - 2.0 * t, l),
        ];
        if self.style.tail && self.side == TailSide::Bottom {
            let ex = (r + 4.0 * t).max((s - 4.0 * t).min(self.at));
            p.extend([
                (ex + 2.0 * t, l),
                (ex + 2.0 * t, l + t),
                (ex + t, l + t),
                (ex + t, l + 2.0 * t),
                (ex, l + 2.0 * t),
                (ex, l + t),
                (ex - t, l + t),
                (ex - t, l),
            ]);
        }
        p.extend([
            (r + 2.0 * t, l),
            (r + 2.0 * t, l - t),
            (r + t, l - t),
            (r + t, l - 2.0 * t),
            (r, l - 2.0 * t),
            (r, i + 2.0 * t),
            (r + t, i + 2.0 * t),
            (r + t, i + t),
            (r + 2.0 * t, i + t),
        ]);
        self.fill_poly(pm, &p, true, 1.0, false);
        self.stroke_poly(pm, &p, true, self.line, 1.0, self.cm, sk::LineJoin::Miter, false);
    }

    fn draw_caption(&self, pm: &mut sk::Pixmap) {
        if let Some(path) = rr_path(0.5, 0.5, self.w - 1.0, self.h - 1.0, 7.0) {
            pm.fill_path(&path, &fill_paint(&self.fill, 0.9, true), sk::FillRule::Winding, self.ts, None);
            pm.stroke_path(
                &path,
                &solid_paint(self.line, 1.0, true),
                &mk_stroke(self.cm, sk::LineJoin::Round),
                self.ts,
                None,
            );
        }
    }

    fn draw_focus(&self, pm: &mut sk::Pixmap) {
        let t = 1000.0 * self.frames();
        let (p, u) = (self.w / 2.0, self.h / 2.0);
        let (r, n) = (self.w / 2.0, self.h / 2.0);
        self.fill_poly(pm, &ellipse_pts(p, u, r, n), true, 1.0, true);
        let a = (PI32 * (r + n) / 2.4).round() as i32;
        for e in 0..a.max(1) {
            let ef = e as f64;
            let tf = t as f64;
            let o = e as f32 / a as f32 * TAU32 + 0.02 * rnd(ef + tf);
            let (i, s) = (o.cos(), o.sin());
            let (l, d) = (i / r, s / n);
            let hy = l.hypot(d);
            let (m, x) = (l / hy, d / hy);
            let y = 2.0 + 2.5 * rnd(ef + 300.0 + tf);
            let b = 4.0 + 6.0 * rnd(ef + 600.0 + tf);
            let (gx, gy) = (p + i * r + m * y, u + s * n + x * y);
            let op = 0.3 + 0.5 * rnd(ef + 900.0 + tf);
            let wd = (0.5 + 0.6 * rnd(ef + 1200.0)) * self.cm;
            self.stroke_poly(pm, &[(gx, gy), (gx + m * b, gy + x * b)], false, self.line, op, wd, sk::LineJoin::Round, true);
        }
    }

    fn draw_recall(&self, pm: &mut sk::Pixmap) {
        let frames = 20.0;
        let a = self.frames() / frames * TAU32;
        let (p, u) = (self.w / 2.0, self.h / 2.0);
        let (s, h) = (self.w / 2.0 - 3.0, self.h / 2.0 - 2.0);
        self.fill_poly(pm, &ellipse_pts(p, u, s, h), true, 1.0, true);
        let seed = (9.0 + 3.0 * self.frames()) as u32;
        for t in 0..12 {
            let tf = t as f64;
            let e = s + 7.0 * rnd(tf);
            let o = h + 5.0 * rnd(tf + 40.0);
            let n = rnd(tf + 80.0) * TAU32 + a * if t % 2 == 1 { 1.0 } else { -1.0 } * 0.5;
            let i = PI32 * (1.1 + 0.85 * rnd(tf + 120.0));
            let mut pts = Vec::new();
            arc_pts(p, u, e, o, n, n + i, 28, &mut pts);
            let pts = displace(&pts, false, 0.05, 4.0, 2, seed);
            let op = 0.3 + 0.5 * rnd(tf + 160.0);
            let wd = (0.5 + 0.8 * rnd(tf + 200.0)) * self.cm;
            self.stroke_poly(pm, &pts, false, self.line, op, wd, sk::LineJoin::Round, true);
        }
    }

    fn draw_dark(&self, pm: &mut sk::Pixmap) {
        let s = self.style;
        let t = self.frames();
        let seed = (2.0 + 5.0 * t) as u32;
        let (p, u) = (self.w / 2.0, self.h / 2.0);
        let (a, i) = (self.w / 2.0, self.h / 2.0);
        let ring = if s.frame != Frame::Default {
            parse_col(&s.line_c, [0.87, 0.65, 0.28, 1.0])
        } else {
            [58.0 / 255.0, 58.0 / 255.0, 58.0 / 255.0, 1.0]
        };
        let black: Rgba = [5.0 / 255.0, 5.0 / 255.0, 5.0 / 255.0, 1.0];
        let mut tmp = match sk::Pixmap::new(pm.width(), pm.height()) {
            Some(t) => t,
            None => return,
        };
        let outer = displace(&ellipse_pts(p, u, a + 3.0, i + 2.0), true, 0.13, 11.0, 4, seed);
        self.fill_solid_poly(&mut tmp, &outer, ring, 0.8);
        let mid = displace(&ellipse_pts(p, u, a - 1.0, i - 1.0), true, 0.13, 11.0, 4, seed);
        self.fill_solid_poly(&mut tmp, &mid, black, 1.0);
        if s.frame == Frame::Default {
            let l_low = s.line.trim().to_ascii_lowercase();
            let is_neutral = l_low.is_empty()
                || matches!(l_low.as_str(), "#e8e8e824" | "#1c1c1c24" | "#f2f2f233");
            if !is_neutral {
                let accent = parse_col(&s.line, ring);
                self.stroke_poly(&mut tmp, &mid, true, accent, 1.0, 1.2 * self.cm, sk::LineJoin::Round, true);
            }
        }
        let sigma = 0.5 * self.ts.sx;
        if sigma >= 0.4 {
            blur_pixmap(&mut tmp, sigma);
        }
        pm.draw_pixmap(0, 0, tmp.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), None);
        let core = ellipse_pts(p, u, 4f32.max(a - 9.0), 4f32.max(i - 7.0));
        self.fill_solid_poly(pm, &core, black, 1.0);
    }

    fn draw_dream(&self, pm: &mut sk::Pixmap) {
        let t0 = self.frames();
        let seed = (3.0 + 4.0 * t0) as u32;
        let (p, u) = (self.w / 2.0, self.h / 2.0);
        let (rx, ry) = (0.97 * (self.w / 2.0), 0.92 * (self.h / 2.0));
        let a = ((PI32 * (rx + ry) / 22.0).round() as usize).max(9);
        let cp: Vec<Pt> = (0..a)
            .map(|s| {
                let l = s as f32 / a as f32 * TAU32;
                let f = 1.0 + 0.16 * (rnd(s as f64 + 11.0) - 0.5) + if s % 2 == 1 { 0.05 } else { -0.02 };
                (p + l.cos() * rx * f, u + l.sin() * ry * f)
            })
            .collect();
        let mid = |a: Pt, b: Pt| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let mut pts: Vec<Pt> = Vec::new();
        let mut cur = mid(cp[a - 1], cp[0]);
        pts.push(cur);
        for i in 0..a {
            let next = mid(cp[i], cp[(i + 1) % a]);
            quad_pts(cur, cp[i], next, 8, &mut pts);
            cur = next;
        }
        let cloud = displace(&pts, true, 0.035, 3.5, 2, seed);
        self.fill_poly(pm, &cloud, true, 1.0, true);
        self.stroke_poly(pm, &cloud, true, self.line, 1.0, 1.4 * self.cm, sk::LineJoin::Round, true);
        if self.style.tail {
            let t = t0 / 10.0 * TAU32;
            let (x0, y0, dx, dy) = tail_vec(self.side, self.w, self.h, self.at, 0.0);
            let m = |d: f32| (x0 + dx * d, y0 + dy * d);
            let g = |e: f32| 1.6 * (t - e).sin();
            let (x, b) = (m(11.8), m(22.6 + 2.8));
            let e1 = ellipse_pts(
                x.0 + if dx != 0.0 { g(0.0) } else { 0.0 },
                x.1 + if dy != 0.0 { g(0.0) } else { 0.0 },
                6.5,
                4.8,
            );
            let e1 = displace(&e1, true, 0.035, 3.5, 2, seed);
            self.fill_poly(pm, &e1, true, 1.0, true);
            self.stroke_poly(pm, &e1, true, self.line, 1.0, 1.2 * self.cm, sk::LineJoin::Round, true);
            let e2 = ellipse_pts(
                b.0 + if dx != 0.0 { g(1.2) } else { 0.0 },
                b.1 + if dy != 0.0 { g(1.2) } else { 0.0 },
                3.8,
                2.8,
            );
            let e2 = displace(&e2, true, 0.035, 3.5, 2, seed);
            self.fill_poly(pm, &e2, true, 1.0, true);
            self.stroke_poly(pm, &e2, true, self.line, 1.0, self.cm, sk::LineJoin::Round, true);
        }
    }

    fn draw_paper(&self, pm: &mut sk::Pixmap) {
        let t = self.frames();
        let seed = (7.0 + 11.0 * t) as u32;
        let pts = displace(&rect_pts(1.0, 1.0, self.w - 2.0, self.h - 2.0), true, 0.25, 4.5, 2, seed);
        self.fill_poly(pm, &pts, true, 1.0, true);
        self.stroke_poly(pm, &pts, true, self.line, 0.8, 1.2 * self.cm, sk::LineJoin::Round, true);
    }

    fn draw_shout(&self, pm: &mut sk::Pixmap) {
        let t = self.frames();
        let (seed1, seed2) = ((3.0 + 3.0 * t) as u32, (9.0 + 2.0 * t) as u32);
        let (p, u) = (self.w / 2.0, self.h / 2.0);
        let (a, i) = ((self.w / 2.0 - 10.0).max(4.0), (self.h / 2.0 - 4.0).max(4.0));
        let per = PI32 * (3.0 * (a + i) - ((3.0 * a + i) * (a + 3.0 * i)).sqrt());
        let y = ((per / 16.0).round() as usize).max(12);
        let jx = 2.2 * (rnd(t as f64 + 1.0) - 0.5);
        let jy = 2.2 * (rnd(t as f64 + 7.0) - 0.5);
        let mut poly: Vec<Pt> = Vec::with_capacity(2 * y);
        for k in 0..2 * y {
            let e = k as f32 / (2.0 * y as f32) * TAU32;
            let (o, r) = (e.cos(), e.sin());
            let (n, s) = (o / a, r / i);
            let l = n.hypot(s);
            let f = if k % 2 == 1 { 0.0 } else { 9.0 + 5.0 * rnd(k as f64) };
            poly.push((p + o * a + n / l * f + jx, u + r * i + s / l * f + jy));
        }
        let main = displace(&poly, true, 0.035, 3.5, 2, seed1);
        self.fill_poly(pm, &main, true, 1.0, true);
        self.stroke_poly(pm, &main, true, self.line, 1.0, 1.4 * self.cm, sk::LineJoin::Miter, true);
        let shifted: Vec<Pt> = poly.iter().map(|q| (q.0 + 1.5, q.1 - 1.0)).collect();
        let dup = displace(&shifted, true, 0.05, 4.0, 2, seed2);
        self.stroke_poly(pm, &dup, true, self.line, 0.4, 0.8 * self.cm, sk::LineJoin::Miter, true);
    }
}

// ── blur / shadow helpers ──

fn box_blur_pass(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let n = (2 * r + 1) as f32;
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut acc: f32 = row[..=r.min(w - 1)].iter().sum();
        for x in 0..w {
            tmp[y * w + x] = acc / n;
            if x + r + 1 < w {
                acc += row[x + r + 1];
            }
            if x >= r {
                acc -= row[x - r];
            }
        }
    }
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        let mut acc = 0f32;
        for y in 0..=r.min(h - 1) {
            acc += tmp[y * w + x];
        }
        for y in 0..h {
            out[y * w + x] = acc / n;
            if y + r + 1 < h {
                acc += tmp[(y + r + 1) * w + x];
            }
            if y >= r {
                acc -= tmp[(y - r) * w + x];
            }
        }
    }
    out
}

fn gauss_blur_plane(plane: &mut Vec<f32>, w: usize, h: usize, sigma: f32) {
    if sigma < 0.35 || w == 0 || h == 0 {
        return;
    }
    let bw = (4.0 * sigma * sigma + 1.0).sqrt().round() as usize;
    let r = (bw.max(1) - 1) / 2;
    let r = r.max(1);
    for _ in 0..3 {
        *plane = box_blur_pass(plane, w, h, r);
    }
}

fn blur_pixmap(pm: &mut sk::Pixmap, sigma: f32) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let data = pm.data_mut();
    for ch in 0..4 {
        let mut plane: Vec<f32> = (0..w * h).map(|i| data[i * 4 + ch] as f32).collect();
        gauss_blur_plane(&mut plane, w, h, sigma);
        for i in 0..w * h {
            data[i * 4 + ch] = plane[i].round().clamp(0.0, 255.0) as u8;
        }
    }
    for px in data.chunks_exact_mut(4) {
        let a = px[3];
        px[0] = px[0].min(a);
        px[1] = px[1].min(a);
        px[2] = px[2].min(a);
    }
}

/// Black drop shadow built from `src`'s alpha, shifted by `(dx, dy)` px and blurred by `sigma` px.
fn drop_shadow(src: &sk::Pixmap, dx: i32, dy: i32, sigma: f32, alpha: f32) -> Option<sk::Pixmap> {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let sd = src.data();
    let mut plane = vec![0f32; w * h];
    for y in 0..h as i32 {
        let ty = y + dy;
        if ty < 0 || ty >= h as i32 {
            continue;
        }
        for x in 0..w as i32 {
            let tx = x + dx;
            if tx < 0 || tx >= w as i32 {
                continue;
            }
            plane[ty as usize * w + tx as usize] = sd[(y as usize * w + x as usize) * 4 + 3] as f32;
        }
    }
    gauss_blur_plane(&mut plane, w, h, sigma);
    let mut out = sk::Pixmap::new(w as u32, h as u32)?;
    let od = out.data_mut();
    for i in 0..w * h {
        od[i * 4 + 3] = (plane[i] * alpha).round().clamp(0.0, 255.0) as u8;
    }
    Some(out)
}

fn compose(dst: &mut sk::Pixmap, src: &sk::Pixmap) {
    dst.draw_pixmap(0, 0, src.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), None);
}

fn blit_cov(
    pm: &mut sk::Pixmap,
    cov: &[u8],
    gw: usize,
    gh: usize,
    x: i32,
    y: i32,
    rgb: [u8; 3],
    alpha: f32,
    clip: Option<(i32, i32)>,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for gy in 0..gh {
        let py = y + gy as i32;
        if py < 0 || py >= ph {
            continue;
        }
        if let Some((c0, c1)) = clip {
            if py < c0 || py >= c1 {
                continue;
            }
        }
        for gx in 0..gw {
            let px = x + gx as i32;
            if px < 0 || px >= pw {
                continue;
            }
            let c = cov[gy * gw + gx];
            if c == 0 {
                continue;
            }
            let a = c as f32 / 255.0 * alpha;
            let inv = 1.0 - a;
            let i = ((py * pw + px) * 4) as usize;
            let out_a = (a * 255.0 + data[i + 3] as f32 * inv).round().clamp(0.0, 255.0) as u8;
            data[i + 3] = out_a;
            for k in 0..3 {
                data[i + k] = ((rgb[k] as f32 * a + data[i + k] as f32 * inv).round().clamp(0.0, 255.0) as u8).min(out_a);
            }
        }
    }
}

/// Rasterizes one glyph at device-space pen `(x_dev, base_dev)`.
fn draw_glyph_at(
    pm: &mut sk::Pixmap,
    fonts: &FontStack,
    ch: char,
    px_dev: f32,
    bold: bool,
    x_dev: f32,
    base_dev: f32,
    rgb: [u8; 3],
    alpha: f32,
    clip: Option<(i32, i32)>,
) {
    if ch.is_whitespace() || alpha <= 0.0 {
        return;
    }
    let Some(font) = fonts.font_for(ch, bold) else { return };
    let (m, cov) = font.rasterize(ch, px_dev);
    if m.width == 0 || m.height == 0 {
        return;
    }
    let gx = x_dev.round() as i32 + m.xmin;
    let gy = base_dev.round() as i32 - (m.ymin + m.height as i32);
    blit_cov(pm, &cov, m.width, m.height, gx, gy, rgb, alpha, clip);
}

fn rgb8(c: Rgba) -> [u8; 3] {
    [(c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8]
}

struct GlyphPlace {
    ch: char,
    x: f32,
    base: f32,
    idx: usize,
}

fn draw_bubble_text(
    pm: &mut sk::Pixmap,
    text: &str,
    place: &BubblePlacement,
    style: &ResolvedBubbleStyle,
    fonts: &FontStack,
    dpr: f32,
    pad: f32,
    fx_elapsed_s: Option<f64>,
    shake_tick: u32,
) {
    let lay = fonts.layout_box(text, place.width_dip, place.font_px, style);
    let m = lay.metrics;
    let px = place.font_px as f32;
    let bold = style.is_bold();
    let w = place.width_dip as f32;
    let h = place.height_dip as f32;
    let line_h = m.line_h as f32;
    let content_w = w - 2.0 * m.border as f32 - 2.0 * m.pad_x as f32;
    let mut lines = lay.lines.clone();
    if let Some(cap) = place.clamp_lines {
        if lines.len() > cap.max(1) {
            lines.truncate(cap.max(1));
            if let Some(last) = lines.last_mut() {
                while !last.is_empty() && fonts.text_width(&format!("{last}…"), px, bold) > content_w {
                    last.pop();
                }
                last.push('…');
            }
        }
    }
    let widths: Vec<f32> = lines.iter().map(|l| fonts.text_width(l, px, bold)).collect();
    let block_w = widths.iter().cloned().fold(0f32, f32::max);
    let x0 = m.border as f32 + m.pad_x as f32 + ((content_w - block_w) / 2.0).max(0.0);
    let top0 = if is_short(text) {
        (h - lines.len() as f32 * line_h) / 2.0
    } else {
        m.border as f32 + m.pad_t as f32
    };
    let (asc, desc) = fonts.line_metrics(px, bold);
    let base_off = (line_h - (asc - desc)) / 2.0 + asc;

    let mut glyphs: Vec<GlyphPlace> = Vec::new();
    let mut idx = 0usize;
    let eff_align = if is_short(text) { TextAlign::Center } else { style.align };
    for (i, line) in lines.iter().enumerate() {
        let ax = match eff_align {
            TextAlign::Left => 0.0,
            TextAlign::Center => (block_w - widths[i]) / 2.0,
            TextAlign::Right => block_w - widths[i],
        };
        let mut pen = x0 + ax;
        for ch in line.chars() {
            let adv = fonts.advance(ch, px, bold);
            let id = if ch.is_whitespace() {
                usize::MAX
            } else {
                idx += 1;
                idx - 1
            };
            glyphs.push(GlyphPlace { ch, x: pen, base: top0 + i as f32 * line_h + base_off, idx: id });
            pen += adv;
        }
    }

    let fg = if style.shape == Shape::Dark {
        [242.0 / 255.0, 242.0 / 255.0, 242.0 / 255.0, 1.0]
    } else {
        parse_col(&style.fg, [0.91, 0.91, 0.91, 1.0])
    };
    let rgb = fg_rgb(fg);
    let fx = if fx_elapsed_s.is_some() {
        style.text_fx
    } else if style.text_fx == TextFx::Shake && shake_tick > 0 {
        TextFx::Shake
    } else {
        TextFx::None
    };
    let elapsed_s = fx_elapsed_s.unwrap_or(0.0);
    let r_ms = elapsed_s * 1000.0 * style.speed_t;
    let px_dev = px * dpr;
    let off = pad * dpr;
    let put = |pm: &mut sk::Pixmap, g: &GlyphPlace, dx: f32, dy: f32, alpha: f32, c: [u8; 3], clip: Option<(i32, i32)>| {
        draw_glyph_at(pm, fonts, g.ch, px_dev, bold, off + (g.x + dx) * dpr, off + (g.base + dy) * dpr, c, alpha * fg[3], clip);
    };
    match fx {
        TextFx::Glitch => {
            let t = (r_ms / 40.0).floor();
            let e = 9.0 * (1.0 - r_ms / 700.0).max(0.0) + if t % 20.0 < 3.0 && r_ms > 900.0 { 6.0 } else { 0.0 };
            let base_alpha = if e > 0.0 { 0.45 } else { 1.0 };
            for g in &glyphs {
                put(pm, g, 0.0, 0.0, base_alpha, rgb, None);
            }
            if e > 0.0 {
                let e = e as f32;
                let n = (55.0 * fx_rand(t + 9.0)).floor() as f32;
                let a2 = (100.0 - n - 25.0 - (25.0 * fx_rand(t + 19.0)).floor() as f32).max(0.0);
                let top = (pad + top0) * dpr;
                let hh = lines.len() as f32 * line_h * dpr;
                let rows = |a: f32, b: f32| Some(((top + hh * a / 100.0).round() as i32, (top + hh * b / 100.0).round() as i32));
                let layers: [(f32, [u8; 3], Option<(i32, i32)>); 3] = [
                    ((rnd(t) - 0.5) * e, [0x7a, 0x7a, 0x7a], None),
                    ((rnd(t + 50.0) - 0.5) * e, rgb, rows(0.0, n)),
                    ((rnd(t + 70.0) - 0.5) * e * 1.2, rgb, rows(n, 100.0 - a2)),
                ];
                for (dx, c, clip) in layers {
                    for g in &glyphs {
                        put(pm, g, dx, 0.0, 1.0, c, clip);
                    }
                }
            }
        }
        _ => {
            for g in &glyphs {
                if g.idx == usize::MAX {
                    continue;
                }
                let i = g.idx as f64;
                match fx {
                    TextFx::None => put(pm, g, 0.0, 0.0, 1.0, rgb, None),
                    TextFx::Type => {
                        let shown = ((r_ms - 120.0).max(0.0) / 70.0).floor();
                        if i < shown {
                            put(pm, g, 0.0, 0.0, 1.0, rgb, None);
                        }
                    }
                    TextFx::Blur => {
                        let e = ((r_ms - 100.0 - 70.0 * i) / 280.0).clamp(0.0, 1.0) as f32;
                        if e >= 1.0 {
                            put(pm, g, 0.0, 0.0, 1.0, rgb, None);
                        } else if e > 0.0 {
                            let s = 4.0 * (1.0 - e) * 0.5;
                            for (dx, dy) in [(0.0, 0.0), (s, 0.0), (-s, 0.0), (0.0, s), (0.0, -s)] {
                                put(pm, g, dx, dy, e / 5.0, rgb, None);
                            }
                        }
                    }
                    TextFx::Wave => {
                        let dy = 2.4 * (r_ms / 1200.0 * PI * 2.0 - 0.55 * i).sin() as f32;
                        put(pm, g, 0.0, dy, 1.0, rgb, None);
                    }
                    TextFx::Shake => {
                        let t = shake_tick as f64;
                        let dx = 2.4 * (fx_rand(13.0 * t + i) as f32 - 0.5);
                        let dy = 2.4 * (fx_rand(17.0 * t + i + 5.0) as f32 - 0.5);
                        put(pm, g, dx, dy, 1.0, rgb, None);
                    }
                    TextFx::Glitch => {}
                }
            }
            if fx == TextFx::Type && idx > 0 {
                let shown = ((r_ms - 120.0).max(0.0) / 70.0).floor() as usize;
                let elapsed_ms = elapsed_s * 1000.0;
                if shown <= idx + 40 && (shown < idx || ((elapsed_ms / 450.0).floor() as u64) % 2 == 0) {
                    let anchor = if shown > 0 {
                        glyphs.iter().find(|g| g.idx == shown.min(idx) - 1)
                    } else {
                        glyphs.iter().find(|g| g.idx == 0)
                    };
                    if let Some(g) = anchor {
                        let cur_x = if shown > 0 {
                            g.x + fonts.advance(g.ch, px, bold) + 1.5
                        } else {
                            g.x - 1.0
                        };
                        let cur_y = g.base - asc + 0.08 * px;
                        let (x0_d, y0_d) = ((pad + cur_x) * dpr, (pad + cur_y) * dpr);
                        let (cw_d, ch_d) = (1.5 * dpr, 1.05 * px * dpr);
                        let rect = [(x0_d, y0_d), (x0_d + cw_d, y0_d), (x0_d + cw_d, y0_d + ch_d), (x0_d, y0_d + ch_d)];
                        if let Some(path) = poly_path(&rect, true) {
                            pm.fill_path(&path, &solid_paint(fg, 1.0, true), sk::FillRule::Winding, sk::Transform::identity(), None);
                        }
                    }
                }
            }
        }
    }
}

fn fg_rgb(c: Rgba) -> [u8; 3] {
    rgb8(c)
}

/// Rasterizes one speech bubble to a premultiplied RGBA sprite.
///
/// Returns `(sprite, pad_left_dip, pad_top_dip)`; place the sprite's top-left at
/// `(placement.left_dip - pad_left_dip, placement.top_dip - pad_top_dip)` (+ appear dy).
///
/// * `phase_idx`: wobble frame (`BubbleState::phase_idx`), taken modulo the shape's frame count.
/// * `appear_scale` / `appear_alpha`: applied around the tail tip (CSS `transform-origin`).
/// * `blur_elapsed_s`: `Some(seconds)` enables the style's text effect (type/blur/wave/glitch);
///   `shake_tick` drives the `shake` effect. `None` draws plain text.
pub fn rasterize_bubble(
    text: &str,
    place: &BubblePlacement,
    style: &ResolvedBubbleStyle,
    fonts: &FontStack,
    dpr: f64,
    phase_idx: u32,
    appear_scale: f32,
    appear_alpha: f32,
    blur_elapsed_s: Option<f64>,
    shake_tick: u32,
) -> (RgbaSprite, f32, f32) {
    let dpr = dpr.clamp(0.5, 4.0) as f32;
    let pad = SPRITE_PAD_DIP as f32;
    let w = place.width_dip.max(1.0) as f32;
    let h = place.height_dip.max(1.0) as f32;
    let cw = ((w + 2.0 * pad) * dpr).ceil().max(1.0) as u32;
    let ch = ((h + 2.0 * pad) * dpr).ceil().max(1.0) as u32;
    let mk = |w: u32, h: u32| sk::Pixmap::new(w, h).or_else(|| sk::Pixmap::new(1, 1)).unwrap();
    let mut out = mk(cw, ch);
    let ts = sk::Transform::from_row(dpr, 0.0, 0.0, dpr, dpr * pad, dpr * pad);

    let frames = style.shape.wobble_frames();
    let phase = phase_idx % frames.max(1);
    let bg = parse_col(&style.bg, [0.13, 0.13, 0.13, 1.0]);
    let fill = if style.grad && style.shape != Shape::Dark {
        FillSpec::Grad { a: bg, b: parse_col(&style.bg2, bg), dir: style.gdir, w, h }
    } else {
        FillSpec::Solid(bg)
    };
    let line = if style.frame != Frame::Default {
        parse_col(&style.line_c, [0.91, 0.91, 0.91, 1.0])
    } else {
        opaque_if_faint(&style.line, &style.fg)
    };
    let cm = if matches!(style.frame, Frame::Solid | Frame::Dashed | Frame::Double) {
        ((style.line_w as f32) / 1.6).max(1.0)
    } else {
        1.0
    };
    let ctx = ShapeCtx {
        style,
        w,
        h,
        side: place.tail_side,
        at: place.tail_at_dip as f32,
        phase,
        fill,
        line,
        cm,
        ts,
    };
    let mut layer = mk(cw, ch);
    ctx.draw(&mut layer);

    if style.shadow && style.shape != Shape::Dark {
        let native_box = style.shape == Shape::Box && style.frame == Frame::Default && !style.grad;
        let shadows: &[(f32, f32, f32)] = if native_box {
            &[(8.0, 11.0, 0.32), (1.0, 1.5, 0.26)]
        } else {
            &[(6.0, 5.0, 0.32)]
        };
        for (dy, sigma, a) in shadows {
            if let Some(sh) = drop_shadow(&layer, 0, (dy * dpr).round() as i32, sigma * dpr, *a) {
                compose(&mut out, &sh);
            }
        }
    }
    compose(&mut out, &layer);
    draw_bubble_text(&mut out, text, place, style, fonts, dpr, pad, blur_elapsed_s, shake_tick);

    if (appear_scale - 1.0).abs() > 0.001 || appear_alpha < 0.999 {
        let (tx, ty) = match place.tail_side {
            TailSide::Bottom => (place.tail_at_dip as f32, h),
            TailSide::Top => (place.tail_at_dip as f32, 0.0),
            TailSide::Left => (0.0, place.tail_at_dip as f32),
            TailSide::Right => (w, place.tail_at_dip as f32),
        };
        let (px, py) = ((pad + tx) * dpr, (pad + ty) * dpr);
        let s = appear_scale.max(0.01);
        let mut fin = mk(out.width(), out.height());
        let mut pp = sk::PixmapPaint::default();
        pp.opacity = appear_alpha.clamp(0.0, 1.0);
        pp.quality = sk::FilterQuality::Bilinear;
        fin.draw_pixmap(0, 0, out.as_ref(), &pp, sk::Transform::from_row(s, 0.0, 0.0, s, px * (1.0 - s), py * (1.0 - s)), None);
        out = fin;
    }
    let (width, height) = (out.width(), out.height());
    (RgbaSprite { width, height, pixels: out.take() }, pad, pad)
}

// ───────────────────────────── reminder badge ─────────────────────────────

fn kind_color(kind: &str) -> Rgba {
    let c = match kind {
        "pill" => [0x6F, 0xD3, 0x9B],
        "water" => [0x7F, 0xB2, 0xFF],
        "todo" => [0xC9, 0xA6, 0xFF],
        _ => [0xDF, 0xA7, 0x48],
    };
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]
}

/// Polylines (24x24 viewBox, closed?) of the `RM_ICON` vector icons.
fn icon_shapes(kind: &str) -> Vec<(Vec<Pt>, bool)> {
    let mut v: Vec<(Vec<Pt>, bool)> = Vec::new();
    let deg = |d: f32| d.to_radians();
    match kind {
        "pill" => {
            let mut p: Vec<Pt> = vec![(10.5, 20.5), (3.5, 13.5)];
            arc_pts(7.0, 10.0, 4.95, 4.95, deg(135.0), deg(315.0), 16, &mut p);
            p.push((17.5, 13.5));
            arc_pts(14.0, 17.0, 4.95, 4.95, deg(315.0), deg(495.0), 16, &mut p);
            v.push((p, true));
            v.push((vec![(8.5, 8.5), (15.5, 15.5)], false));
        }
        "water" => {
            let mut p: Vec<Pt> = vec![(12.0, 2.7)];
            cubic_pts((12.0, 2.7), (12.0, 2.7), (18.0, 9.3), (18.0, 14.0), 14, &mut p);
            arc_pts(12.0, 14.0, 6.0, 6.0, 0.0, PI32, 16, &mut p);
            cubic_pts((6.0, 14.0), (6.0, 9.3), (12.0, 2.7), (12.0, 2.7), 14, &mut p);
            v.push((p, true));
        }
        "todo" => {
            let mut p: Vec<Pt> = Vec::new();
            for (cx, cy, a0) in [(16.0, 8.0, -90.0f32), (16.0, 16.0, 0.0), (8.0, 16.0, 90.0), (8.0, 8.0, 180.0)] {
                arc_pts(cx, cy, 4.0, 4.0, deg(a0), deg(a0 + 90.0), 6, &mut p);
            }
            v.push((p, true));
            v.push((vec![(8.5, 12.2), (10.9, 14.6), (15.5, 9.6)], false));
        }
        _ => {
            let mut p: Vec<Pt> = Vec::new();
            arc_pts(12.0, 8.0, 6.0, 6.0, deg(180.0), deg(360.0), 18, &mut p);
            cubic_pts((18.0, 8.0), (18.0, 15.0), (21.0, 17.0), (21.0, 17.0), 10, &mut p);
            p.push((3.0, 17.0));
            cubic_pts((3.0, 17.0), (3.0, 17.0), (6.0, 15.0), (6.0, 8.0), 10, &mut p);
            v.push((p, true));
            let mut c: Vec<Pt> = Vec::new();
            arc_pts(12.0, 20.066, 1.94, 1.94, deg(151.2), deg(28.8), 10, &mut c);
            v.push((c, false));
        }
    }
    v
}

fn draw_icon(pm: &mut sk::Pixmap, kind: &str, x: f32, y: f32, size: f32, dpr: f32, pad: f32, color: Rgba) {
    let s = size / 24.0;
    let ts = sk::Transform::from_row(dpr * s, 0.0, 0.0, dpr * s, dpr * (pad + x), dpr * (pad + y));
    for (pts, closed) in icon_shapes(kind) {
        if let Some(path) = poly_path(&pts, closed) {
            pm.stroke_path(&path, &solid_paint(color, 1.0, true), &mk_stroke(2.0, sk::LineJoin::Round), ts, None);
        }
    }
}

fn draw_run(
    pm: &mut sk::Pixmap,
    fonts: &FontStack,
    s: &str,
    px: f32,
    bold: bool,
    x: f32,
    top: f32,
    line_h: f32,
    color: Rgba,
    alpha: f32,
    pad: f32,
    dpr: f32,
) {
    let (asc, desc) = fonts.line_metrics(px, bold);
    let base = top + (line_h - (asc - desc)) / 2.0 + asc;
    let mut pen = x;
    let rgb = rgb8(color);
    for ch in s.chars() {
        draw_glyph_at(pm, fonts, ch, px * dpr, bold, (pad + pen) * dpr, (pad + base) * dpr, rgb, alpha * color[3], None);
        pen += fonts.advance(ch, px, bold);
    }
}

/// Rasterizes the reminder badge (`#remind`): a 58x44 icon badge, or a 190..280 px bubble with
/// icon, header, body text, footer and a 13 px tail. Returns `(sprite, pad_left_dip, pad_top_dip)`.
pub fn rasterize_remind_badge(
    kind: &str,
    header: &str,
    body_line: &str,
    footer_left: &str,
    time_str: &str,
    has_bubble: bool,
    custom_style: Option<&ResolvedBubbleStyle>,
    fonts: &FontStack,
    dpr: f64,
) -> (RgbaSprite, f32, f32) {
    let dpr = dpr.clamp(0.5, 4.0) as f32;
    let pad = REMIND_PAD_DIP;
    let kc = kind_color(kind);
    let mk = |w: u32, h: u32| sk::Pixmap::new(w, h).or_else(|| sk::Pixmap::new(1, 1)).unwrap();
    let ts = sk::Transform::from_row(dpr, 0.0, 0.0, dpr, dpr * pad, dpr * pad);

    let styled = custom_style;
    let bg = styled.map(|s| parse_col(&s.bg, [0.11, 0.11, 0.11, 1.0])).unwrap_or([29.0 / 255.0, 29.0 / 255.0, 29.0 / 255.0, 1.0]);
    let fg = styled.map(|s| parse_col(&s.fg, [0.93, 0.93, 0.93, 1.0])).unwrap_or([236.0 / 255.0, 236.0 / 255.0, 236.0 / 255.0, 1.0]);
    let border = match styled {
        Some(s) if matches!(s.frame, Frame::Solid | Frame::Dashed | Frame::Double) => parse_col(&s.line_c, fg),
        _ => kc,
    };
    let tail_on = styled.map(|s| s.tail).unwrap_or(true);

    let (w, h, radius, lines): (f32, f32, f32, Vec<String>);
    let body_px = 15.0f32;
    let (hd_px, ft_px) = (12.0f32, 11.0f32);
    if has_bubble {
        let content_max: f32 = 280.0 - 3.2 - 30.0;
        let hd_w = 15.0 + 6.0 + fonts.text_width(header, hd_px, true);
        let ft_w = fonts.text_width(footer_left, ft_px, false) + 16.0 + fonts.text_width(time_str, ft_px, false);
        let mode = break_for(body_line);
        let body_nat = body_line
            .split('\n')
            .map(|l| fonts.text_width(l.trim_end(), body_px, true))
            .fold(0f32, f32::max);
        let natural = hd_w.max(ft_w).max(body_nat);
        let bw = (natural + 33.2).clamp(190.0, 280.0);
        let ln = if body_nat <= bw - 33.2 {
            body_line.split('\n').map(|l| l.trim_end().to_string()).collect()
        } else {
            fonts.wrap(body_line, (bw - 33.2).min(content_max.max(150.0)), body_px, true, mode)
        };
        let bh = 1.6 + 11.0 + 16.0 + 5.0 + ln.len() as f32 * body_px * 1.45 + 7.0 + 1.0 + 6.0 + 14.0 + 9.0 + 1.6;
        w = bw;
        h = bh;
        lines = ln;
        radius = styled
            .map(|s| ((if s.radius > 0.0 { s.radius } else { 14.0 }) * s.k()).round() as f32)
            .unwrap_or(14.0);
    } else {
        w = 58.0;
        h = 44.0;
        radius = 13.0;
        lines = Vec::new();
    }
    let tail_extra = if tail_on { 12.0 } else { 0.0 };
    let cw = ((w + 2.0 * pad) * dpr).ceil() as u32;
    let ch = ((h + tail_extra + 2.0 * pad) * dpr).ceil() as u32;
    let mut out = mk(cw, ch);

    // glow: mid-point of `rmGlow` (ring 4.5px @ .2 + blur 20px spread 5px @ .42)
    let mut glow = mk(cw, ch);
    if let Some(p) = rr_path(-5.0, -5.0, w + 10.0, h + 10.0, radius + 5.0) {
        glow.fill_path(&p, &solid_paint(kc, 0.42, true), sk::FillRule::Winding, ts, None);
    }
    blur_pixmap(&mut glow, 10.0 * dpr);
    compose(&mut out, &glow);
    if let Some(p) = rr_path(-4.5, -4.5, w + 9.0, h + 9.0, radius + 4.5) {
        out.fill_path(&p, &solid_paint(kc, 0.2, true), sk::FillRule::Winding, ts, None);
    }
    if let Some(p) = rr_path(0.0, 0.0, w, h, radius) {
        out.fill_path(&p, &solid_paint(bg, 1.0, true), sk::FillRule::Winding, ts, None);
    }
    if let Some(p) = rr_path(0.8, 0.8, w - 1.6, h - 1.6, radius - 0.8) {
        let mut st = mk_stroke(1.6, sk::LineJoin::Round);
        if let Some(s) = styled {
            if s.frame == Frame::Dashed {
                st.dash = sk::StrokeDash::new(vec![8.0, 5.0], 0.0);
            }
        }
        out.stroke_path(&p, &solid_paint(border, 1.0, true), &st, ts, None);
    }

    // tail: 13px rotated square, bottom -8px, centered
    if tail_on {
        let cx = w / 2.0;
        let top_y = h + 0.1;
        let hd = 9.19f32;
        let tri = [(cx - hd, top_y), (cx, top_y + hd), (cx + hd, top_y)];
        if let Some(p) = poly_path(&[(cx - hd, top_y), (cx, top_y - hd), (cx + hd, top_y), (cx, top_y + hd)], true) {
            out.fill_path(&p, &solid_paint(bg, 1.0, true), sk::FillRule::Winding, ts, None);
        }
        if let Some(p) = poly_path(&tri, false) {
            out.stroke_path(&p, &solid_paint(border, 1.0, true), &mk_stroke(1.6, sk::LineJoin::Round), ts, None);
        }
    }

    if !has_bubble {
        draw_icon(&mut out, kind, 17.0, 10.0, 24.0, dpr, pad, kc);
    } else {
        let cx = 1.6 + 15.0;
        let mut y = 1.6 + 11.0;
        draw_icon(&mut out, kind, cx, y + 0.5, 15.0, dpr, pad, kc);
        draw_run(&mut out, fonts, header, hd_px, true, cx + 21.0, y, 16.0, kc, 1.0, pad, dpr);
        y += 16.0 + 5.0;
        for l in &lines {
            draw_run(&mut out, fonts, l, body_px, true, cx, y, body_px * 1.45, fg, 1.0, pad, dpr);
            y += body_px * 1.45;
        }
        y += 7.0;
        let sep = styled.map(|_| [fg[0], fg[1], fg[2], 0.14]).unwrap_or([1.0, 1.0, 1.0, 0.08]);
        if let Some(p) = poly_path(&[(cx, y + 0.5), (w - cx, y + 0.5)], false) {
            out.stroke_path(&p, &solid_paint(sep, 1.0, true), &mk_stroke(1.0, sk::LineJoin::Round), ts, None);
        }
        y += 1.0 + 6.0;
        let ft_col = styled.map(|_| [fg[0], fg[1], fg[2], 0.62]).unwrap_or([154.0 / 255.0, 154.0 / 255.0, 154.0 / 255.0, 1.0]);
        draw_run(&mut out, fonts, footer_left, ft_px, false, cx, y, 14.0, ft_col, 1.0, pad, dpr);
        let tw = fonts.text_width(time_str, ft_px, false);
        draw_run(&mut out, fonts, time_str, ft_px, false, w - cx - tw, y, 14.0, ft_col, 1.0, pad, dpr);
    }
    let (width, height) = (out.width(), out.height());
    (RgbaSprite { width, height, pixels: out.take() }, pad, pad)
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn premultiplied_nonempty(s: &RgbaSprite) -> bool {
        assert_eq!(s.pixels.len(), (s.width * s.height * 4) as usize);
        let mut any = false;
        for px in s.pixels.chunks_exact(4) {
            assert!(px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3], "not premultiplied: {px:?}");
            any |= px[3] > 0;
        }
        any
    }

    #[test]
    fn line_bags_no_repeat_and_order() {
        let items: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let mut bags = LineBags::new();
        let mut last = String::new();
        for _ in 0..60 {
            let p = bags.pick("lines", &items, true, false).unwrap();
            assert_ne!(p, last, "immediate repeat");
            last = p;
        }
        let mut seq = Vec::new();
        for _ in 0..4 {
            seq.push(bags.pick("ordered", &items, true, true).unwrap());
        }
        assert_eq!(seq, ["a", "b", "c", "a"]);
        assert_eq!(pool(&[" x ".into(), "".into(), "x".into(), "y".into()]), ["x", "y"]);
        assert!(bags.pick("none", &[], true, false).is_none());
    }

    #[test]
    fn split_seq_keeps_urls() {
        assert_eq!(split_seq("hello"), ["hello"]);
        assert_eq!(split_seq("a // b //  // c"), ["a", "b", "c"]);
        assert_eq!(split_seq("see https://x.com//next"), ["see https://x.com", "next"]);
        assert_eq!(split_seq("https://example.com"), ["https://example.com"]);
        assert_eq!(split_seq("//").len(), 1);
        let many = (0..20).map(|i| i.to_string()).collect::<Vec<_>>().join("//");
        assert_eq!(split_seq(&many).len(), 12);
    }

    #[test]
    fn hold_ms_formula() {
        let mut st = ResolvedBubbleStyle::default();
        // grow: 200 + 0 + min(8000, 900 + 60*5) = 1400 -> max(1500, 1400)
        assert_eq!(hold_ms("hello", &st, 1500.0, false), 1500.0);
        st.text_fx = TextFx::Type;
        assert_eq!(hold_ms("hello", &st, 1500.0, false), (200.0 + 120.0 + 350.0 + 1200.0f64).max(1500.0));
        assert_eq!(hold_ms("hello", &st, 0.0, true), 1500.0f64.max(200.0 + 1200.0));
    }

    #[test]
    fn style_resolution_and_builtins() {
        let mut cfg = CharacterConfig::default();
        let s = ResolvedBubbleStyle::resolve(&cfg, "", "dark");
        assert_eq!(s.shape, Shape::Box);
        assert_eq!(ResolvedBubbleStyle::resolve(&cfg, "", "light").bg, "#ffffff");
        assert_eq!(preset_by_id("amber").map(|p| p.0), Some("#211d18"));
        let shout = ResolvedBubbleStyle::resolve(&cfg, "p_shout", "dark");
        assert_eq!((shout.shape, shout.appear, shout.text_fx, shout.weight), (Shape::Shout, Appear::Pop, TextFx::Shake, 700));
        cfg.bubble_styles = vec![serde_json::json!({"id":"u_abc","name":"x","o":{"shape":"hand","radius":99,"weight":600,"bg":"#112233"}})];
        let c = ResolvedBubbleStyle::resolve(&cfg, "u_abc", "dark");
        assert_eq!((c.shape, c.radius, c.weight, c.bg.as_str()), (Shape::Hand, 28.0, 500, "#112233"));
        cfg.bubble_style = serde_json::json!({"size": 300, "font": 5, "gap": 90, "split": true, "speed": 1.5});
        let c = ResolvedBubbleStyle::resolve(&cfg, "", "dark");
        assert_eq!((c.size, c.font, c.gap, c.speed_a), (160.0, 11.0, 60.0, 1.5));
        cfg.bubble_style = serde_json::json!({"gap": 24});
        assert_eq!(ResolvedBubbleStyle::resolve(&cfg, "", "dark").gap, 24.0);
        cfg.lines = vec!["hello".into(), "boom".into()];
        cfg.line_styles.insert("lines".into(), vec!["".into(), "p_shout".into()]);
        assert_eq!(style_of_line(&cfg, "lines", " boom "), "p_shout");
    }

    #[test]
    fn bubble_fit_places_above_pet() {
        let cb = CharBox { x: 300.0, y: 300.0, w: 200.0, h: 200.0, cx: None };
        let allowed = DipRect { left: 0.0, top: 0.0, right: 1000.0, bottom: 900.0 };
        let mut m = |w: f64, _f: f64| (w.min(120.0), 60.0);
        let p = BubbleFit::plan(&cb, &allowed, &BubbleFit::WIDTHS, 7, &FitOptions { font: 14.0, ..Default::default() }, &mut m);
        assert_eq!(p.tail_side, TailSide::Bottom);
        assert!(p.top_dip + p.height_dip <= cb.y - BubbleFit::GAP + 0.5);
        assert_eq!((p.left_dip, p.top_dip, p.width_dip, p.height_dip), (340.0, 228.0, 120.0, 60.0));
        assert_eq!((p.tail_at_dip, p.font_px, p.clamp_lines), (60.0, 14.0, None));
        // no room anywhere -> clamped fallback still returns a placement
        let tiny = DipRect { left: 0.0, top: 0.0, right: 130.0, bottom: 60.0 };
        let mut big = |_w: f64, _f: f64| (100.0, 400.0);
        let q = BubbleFit::plan(&cb, &tiny, &BubbleFit::WIDTHS, 7, &FitOptions { font: 14.0, ..Default::default() }, &mut big);
        assert!(q.clamp_lines.is_some());
    }

    #[test]
    fn wraps_cjk_and_korean() {
        let fonts = FontStack::system();
        let st = ResolvedBubbleStyle::default();
        let (w, h, lines) = fonts.measure_box("别一直盯着屏幕。休息一下吧，眼睛会累的。", 120.0, 14.0, &st);
        assert!(w <= 120.0 && h > 40.0 && lines.len() >= 2);
        let (_, _, ko) = fonts.measure_box("말풍선 테스트 입니다 길게 길게 길게 길게", 100.0, 14.0, &st);
        assert!(ko.len() >= 2);
        assert!(ko.iter().all(|l| !l.starts_with(' ')));
        if let Ok(dir) = crate::config::discover_data_dir(None) {
            if let Ok(entries) = std::fs::read_dir(dir.join("characters")) {
                if let Some(cdir) = entries
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| p.is_dir() && p.join("font.ttf").is_file())
                {
                    let fs = FontStack::new(&cdir, "font.ttf");
                    assert!(fs.has_custom());
                    assert!(!fs.is_custom_loaded());
                    let _ = fs.measure_box("别一直盯着屏幕。", 200.0, 14.0, &st);
                    assert!(fs.is_custom_loaded());
                }
            }
        }
    }

    #[test]
    fn runtime_say_and_step() {
        let mut cfg = CharacterConfig::default();
        cfg.lines = vec!["Hello there// second line".into()];
        cfg.idle_ms = 2000.0;
        cfg.idle_lines = vec!["idle line".into()];
        let ui = UiPrefs::default();
        let fonts = FontStack::system();
        let cb = CharBox { x: 300.0, y: 300.0, w: 200.0, h: 200.0, cx: None };
        let allowed = DipRect { left: 0.0, top: 0.0, right: 1000.0, bottom: 900.0 };
        let mut rt = BubbleRuntime::new();
        assert!(!rt.poll_idle(&cfg, 0.0, false));
        assert!(rt.poll_idle(&cfg, 3.5, false));
        let line = rt.say("lines", &cfg, &ui, &cb, &allowed, &fonts, 0.0).unwrap();
        assert!(line.contains("//"));
        assert_eq!(rt.active_key(), Some("lines"));
        let a = rt.step(0.05, &cb, &allowed, &fonts).unwrap();
        assert_eq!(a.text, "Hello there");
        let b = rt.step(0.5, &cb, &allowed, &fonts).unwrap();
        assert_eq!(a.dirty_key == b.dirty_key, a.appear_alpha == b.appear_alpha && a.appear_scale == b.appear_scale);
        let c = rt.step(1.65, &cb, &allowed, &fonts).unwrap();
        assert_eq!(c.text, "second line");
        assert!(rt.is_visible(1.65));
        rt.hide(3.0);
        assert!(rt.active_key().is_none());
        assert!(rt.is_visible(3.05));
        assert!(!rt.is_visible(3.2));
        assert!(rt.step(3.2, &cb, &allowed, &fonts).is_none());
    }

    fn sample_place(w: f64, h: f64) -> BubblePlacement {
        BubblePlacement {
            left_dip: 100.0,
            top_dip: 100.0,
            width_dip: w,
            height_dip: h,
            font_px: 14.0,
            tail_side: TailSide::Bottom,
            tail_at_dip: 60.0,
            clamp_lines: None,
        }
    }

    #[test]
    fn rasterizes_all_shapes_premultiplied() {
        let fonts = FontStack::system();
        for shape in [
            Shape::Box, Shape::Hand, Shape::Pixel, Shape::Caption, Shape::Focus,
            Shape::Recall, Shape::Dark, Shape::Dream, Shape::Paper, Shape::Shout,
        ] {
            let mut st = ResolvedBubbleStyle::default();
            st.shape = shape;
            let lay = fonts.layout_box("Hello 말풍선 别一直盯着屏幕。", 260.0, 14.0, &st);
            let place = sample_place(lay.width, lay.height);
            let (sp, pl, pt) = rasterize_bubble("Hello 말풍선 别一直盯着屏幕。", &place, &st, &fonts, 1.5, 3, 1.0, 1.0, None, 0);
            assert_eq!((pl, pt), (SPRITE_PAD_DIP as f32, SPRITE_PAD_DIP as f32));
            assert!(premultiplied_nonempty(&sp), "{shape:?} empty");
        }
    }

    #[test]
    fn rasterizes_hand_and_dark_presets() {
        let fonts = FontStack::system();
        let mut hand = ResolvedBubbleStyle::default();
        hand.shape = Shape::Hand;
        hand.bg = "#ff8ad2".into();
        hand.fg = "#ffffff".into();
        hand.line = "#5fc3ff".into();
        let mut dark = ResolvedBubbleStyle::default();
        dark.shape = Shape::Dark;
        dark.bg = "#252525".into();
        dark.fg = "#ece4d4".into();
        dark.frame = Frame::Default;
        dark.line = "#dfa748a0".into();
        dark.text_fx = TextFx::Blur;
        for (st, fx) in [(hand, None), (dark, Some(0.4))] {
            let lay = fonts.layout_box("Meow~ 말풍선", 260.0, 14.0, &st);
            let place = sample_place(lay.width, lay.height);
            let (sp, _, _) = rasterize_bubble("Meow~ 말풍선", &place, &st, &fonts, 2.0, 7, 0.9, 0.8, fx, 0);
            assert!(premultiplied_nonempty(&sp));
            assert!(sp.width as f64 >= (place.width_dip + 2.0 * SPRITE_PAD_DIP) * 2.0 - 1.0);
        }
    }

    #[test]
    fn rasterizes_remind_badges() {
        let fonts = FontStack::system();
        for kind in ["alarm", "pill", "water", "todo"] {
            let (icon, p1, _) = rasterize_remind_badge(kind, "", "", "", "", false, None, &fonts, 1.0);
            assert!(premultiplied_nonempty(&icon) && p1 > 0.0);
            let (bub, _, _) = rasterize_remind_badge(kind, "알림 · 회의", "10분 뒤 회의가 있어요!", "눌러서 닫기", "14:30", true, None, &fonts, 1.5);
            assert!(premultiplied_nonempty(&bub));
            assert!(bub.height > icon.height);
        }
        let mut cs = ResolvedBubbleStyle::default();
        cs.frame = Frame::Solid;
        cs.tail = false;
        let (s, _, _) = rasterize_remind_badge("todo", "할 일", "하나 끝!", "눌러서 닫기", "09:00", true, Some(&cs), &fonts, 1.0);
        assert!(premultiplied_nonempty(&s));
    }
}
