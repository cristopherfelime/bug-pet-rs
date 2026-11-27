//! Milestone 4: petting detection, head-pat sway/expression state and glow particles.
//!
//! Ports `ui/pet-area.js`, `ui/pet-fx.js` and the petting parts of `pet/pet.js:780-805`.
//! All times are seconds (`f64`), all lengths are DIPs unless stated otherwise.
#![allow(dead_code)]

use crate::assets::RgbaSprite;
use crate::config::{parse_hex_color_rgba, CharacterConfig};
use crate::renderer::{GpuContext, GpuTexture};
use std::collections::HashMap;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

// ───────────────────────────── Pet area mask ─────────────────────────────

const GRID: usize = 96;

/// `PetArea.fit`: normalized placement of a sprite inside the square (landscape bottom-aligned,
/// portrait horizontally centred).
fn fit_rect(w: u32, h: u32) -> (f64, f64, f64, f64) {
    if w == 0 || h == 0 {
        return (0.0, 0.0, 1.0, 1.0);
    }
    if w >= h {
        let t = h as f64 / w as f64;
        (0.0, 1.0 - t, 1.0, t)
    } else {
        let r = w as f64 / h as f64;
        ((1.0 - r) / 2.0, 0.0, r, 1.0)
    }
}

/// Draws the sprite alpha into a 96x96 grid (box-filtered).
fn alpha_grid(s: &RgbaSprite) -> Option<Vec<u8>> {
    let (w, h) = (s.width as usize, s.height as usize);
    if w == 0 || h == 0 || s.pixels.len() < w * h * 4 {
        return None;
    }
    let (fx, fy, fw, fh) = fit_rect(s.width, s.height);
    let n = GRID as f64;
    let (x0, y0, x1, y1) = (fx * n, fy * n, (fx + fw) * n, (fy + fh) * n);
    let mut out = vec![0u8; GRID * GRID];
    for py in 0..GRID {
        let (cy0, cy1) = ((py as f64).max(y0), ((py + 1) as f64).min(y1));
        if cy1 <= cy0 {
            continue;
        }
        let v0 = (cy0 - y0) / (y1 - y0) * h as f64;
        let v1 = (cy1 - y0) / (y1 - y0) * h as f64;
        let sy0 = (v0.floor() as usize).min(h - 1);
        let sy1 = (v1.ceil() as usize).clamp(sy0 + 1, h);
        for px in 0..GRID {
            let (cx0, cx1) = ((px as f64).max(x0), ((px + 1) as f64).min(x1));
            if cx1 <= cx0 {
                continue;
            }
            let u0 = (cx0 - x0) / (x1 - x0) * w as f64;
            let u1 = (cx1 - x0) / (x1 - x0) * w as f64;
            let sx0 = (u0.floor() as usize).min(w - 1);
            let sx1 = (u1.ceil() as usize).clamp(sx0 + 1, w);
            let mut sum = 0u64;
            let mut cnt = 0u64;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    sum += s.pixels[(sy * w + sx) * 4 + 3] as u64;
                    cnt += 1;
                }
            }
            let avg = sum as f64 / cnt.max(1) as f64;
            let cov = (cx1 - cx0) * (cy1 - cy0);
            out[py * GRID + px] = (avg * cov).round().clamp(0.0, 255.0) as u8;
        }
    }
    Some(out)
}

/// First row (0..1) with at least 3 pixels of alpha > 40.
fn first_body_row(grid: &[u8]) -> Option<f64> {
    for row in 0..GRID {
        let c = grid[row * GRID..(row + 1) * GRID].iter().filter(|&&a| a > 40).count();
        if c >= 3 {
            return Some(row as f64 / GRID as f64);
        }
    }
    None
}

/// 96x96 "head" hit mask used to restrict petting to the head area.
#[derive(Debug, Clone)]
pub struct PetAreaMask {
    mask: Vec<u8>,
    any: bool,
    no_head: bool,
    bbox: (f64, f64, f64, f64),
    cut: f64,
}

impl PetAreaMask {
    pub fn build(head1: Option<&RgbaSprite>, body: Option<&RgbaSprite>, pet_cut: f64) -> Self {
        let auto = body
            .and_then(alpha_grid)
            .and_then(|g| first_body_row(&g))
            .map(|r| (r + 0.03).min(1.0))
            .unwrap_or(1.0);
        let cut = if pet_cut > 0.0 { (pet_cut / 100.0).clamp(0.1, 1.0) } else { auto };

        let grid = head1.and_then(alpha_grid);
        let (fx, fy, fw, fh) = head1.map(|s| fit_rect(s.width, s.height)).unwrap_or((0.0, 0.0, 1.0, 1.0));
        let bbox = (fx, fy, fx + fw, (fy + fh).min(cut));
        let mut mask = vec![0u8; GRID * GRID];
        let mut any = false;
        if let Some(g) = &grid {
            let rows = GRID.min((cut * GRID as f64).floor() as usize + 1);
            for r in 0..rows {
                for c in 0..GRID {
                    if g[r * GRID + c] > 12 {
                        mask[r * GRID + c] = 1;
                        any = true;
                    }
                }
            }
        }
        Self { mask, any, no_head: grid.is_none(), bbox, cut }
    }

    /// Resolved cut line (0..1 of the sprite height).
    pub fn cut(&self) -> f64 {
        self.cut
    }

    /// `nx`, `ny` are normalized (flip already applied by the caller) coordinates in the pet square.
    pub fn hit(&self, nx: f64, ny: f64) -> bool {
        if self.no_head {
            return true;
        }
        if !self.any {
            let (x0, y0, x1, y1) = self.bbox;
            return nx >= x0 && nx <= x1 && ny >= y0 && ny <= y1;
        }
        let ox = (nx * GRID as f64).floor() as i64;
        let oy = (ny * GRID as f64).floor() as i64;
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                let (a, b) = (ox + dx, oy + dy);
                if a >= 0 && b >= 0 && (a as usize) < GRID && (b as usize) < GRID && self.mask[b as usize * GRID + a as usize] != 0 {
                    return true;
                }
            }
        }
        false
    }
}

// ───────────────────────────── Basic types ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RectDip {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub cx: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticleKind {
    Heart,
    Note,
    Anger,
    Sweat,
    Sparkle,
    Sleepy,
}

impl ParticleKind {
    pub const ALL: [ParticleKind; 6] = [
        ParticleKind::Heart,
        ParticleKind::Note,
        ParticleKind::Anger,
        ParticleKind::Sweat,
        ParticleKind::Sparkle,
        ParticleKind::Sleepy,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ParticleKind::Heart => "heart",
            ParticleKind::Note => "note",
            ParticleKind::Anger => "anger",
            ParticleKind::Sweat => "sweat",
            ParticleKind::Sparkle => "sparkle",
            ParticleKind::Sleepy => "sleepy",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.name() == s)
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&k| k == self).unwrap_or(0)
    }

    /// Number of SVG variants (only `note` has several).
    pub fn variant_count(self) -> usize {
        if self == ParticleKind::Note {
            4
        } else {
            1
        }
    }

    fn life_ms(self) -> f64 {
        match self {
            ParticleKind::Anger => 1000.0,
            ParticleKind::Sweat => 1150.0,
            ParticleKind::Sparkle => 950.0,
            ParticleKind::Sleepy => 1500.0,
            ParticleKind::Note => 1250.0,
            ParticleKind::Heart => 1200.0,
        }
    }

    fn anchors(self) -> &'static [[f64; 2]] {
        match self {
            ParticleKind::Heart | ParticleKind::Note => &[[0.05, 0.04], [0.11, 0.2], [0.07, 0.36]],
            ParticleKind::Sparkle => &[[-0.06, 0.05], [0.02, 0.25], [-0.02, 0.45]],
            ParticleKind::Sweat => &[[0.04, 0.28], [0.07, 0.4]],
            ParticleKind::Anger => &[[0.14, 0.05], [0.19, 0.13]],
            ParticleKind::Sleepy => &[[0.0, 0.04], [0.04, 0.13]],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PetEdge {
    Glow,
    Line,
    None,
}

/// Fully-resolved raster identity of one particle texture. Bounded cardinality:
/// kind(6) x variant(<=4) x colour(<=7 candy | 24 rainbow hue buckets | fixed) x edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParticleTexKey {
    pub kind: ParticleKind,
    pub variant: u8,
    pub fill: [u8; 3],
    pub line: [u8; 3],
    pub edge: PetEdge,
    /// Line width in half units (only for `PetEdge::Line`, else 0).
    pub line_w_q: u8,
    /// Glow sigma (viewBox units) in half units (only for `PetEdge::Glow`, else 0).
    pub glow_q: u8,
}

const CANDY: [&str; 7] = ["#ff8fb1", "#ffb36b", "#ffe066", "#8ee6c4", "#7cc8ff", "#b9a2ff", "#ff9ee8"];

fn hex_rgb(s: &str) -> Option<[u8; 3]> {
    let [r, g, b, _] = parse_hex_color_rgba(s)?;
    Some([(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8])
}

fn hsl_rgb(h: f64, s: f64, l: f64) -> [u8; 3] {
    let n = l / 100.0;
    let a = (s / 100.0) * n.min(1.0 - n);
    let f = |c: f64| {
        let k = (c + h / 30.0) % 12.0;
        let v = n - a * (-1.0f64).max((k - 3.0).min(9.0 - k).min(1.0));
        (255.0 * v).round().clamp(0.0, 255.0) as u8
    };
    [f(0.0), f(8.0), f(4.0)]
}

pub fn resolve_edge(cfg: &CharacterConfig) -> PetEdge {
    match cfg.pet_edge.as_str() {
        "glow" => PetEdge::Glow,
        "line" => PetEdge::Line,
        "none" => PetEdge::None,
        _ => {
            if cfg.pet_glow {
                PetEdge::Glow
            } else if cfg.pet_line_w > 0.0 {
                PetEdge::Line
            } else {
                PetEdge::None
            }
        }
    }
}

fn glow_size_unclamped(cfg: &CharacterConfig) -> f64 {
    if cfg.pet_glow_size == 0.0 || cfg.pet_glow_size.is_nan() {
        5.0
    } else {
        cfg.pet_glow_size
    }
}

impl ParticleTexKey {
    /// `color_rand` in `[0,1)` picks the candy index / rainbow hue bucket.
    pub fn resolve(cfg: &CharacterConfig, kind: ParticleKind, variant: usize, color_rand: f64) -> Self {
        let mode = match cfg.pet_color_mode.as_str() {
            m @ ("one" | "each" | "candy" | "rainbow") => m,
            _ => "one",
        };
        let base_hex = if cfg.pet_fill.trim().is_empty() { "#ffffff" } else { cfg.pet_fill.as_str() };
        let mut fill = hex_rgb(base_hex).unwrap_or([255, 255, 255]);
        match mode {
            "each" => {
                if let Some(c) = cfg.pet_kind_colors.get(kind.name()).filter(|c| !c.trim().is_empty()) {
                    fill = hex_rgb(c).unwrap_or(fill);
                }
            }
            "candy" => {
                let i = ((color_rand * 7.0) as usize).min(6);
                fill = hex_rgb(CANDY[i]).unwrap_or(fill);
            }
            "rainbow" => {
                let b = ((color_rand * 24.0) as usize).min(23);
                fill = hsl_rgb(b as f64 * 15.0, 88.0, 62.0);
            }
            _ => {}
        }
        let line = if cfg.pet_line_follow && mode != "one" {
            fill
        } else {
            let h = if cfg.pet_line.trim().is_empty() { "#ffffff" } else { cfg.pet_line.as_str() };
            hex_rgb(h).unwrap_or([255, 255, 255])
        };
        let edge = resolve_edge(cfg);
        let (line_rgb, line_w_q, glow_q) = match edge {
            PetEdge::Glow => {
                let d = glow_size_unclamped(cfg).clamp(1.0, 12.0);
                (line, 0u8, (d * 2.0).round() as u8)
            }
            PetEdge::Line => {
                let lw = if cfg.pet_line_w == 0.0 || cfg.pet_line_w.is_nan() { 2.0 } else { cfg.pet_line_w };
                (line, (lw.max(1.0) * 2.0).round().min(255.0) as u8, 0u8)
            }
            PetEdge::None => ([0, 0, 0], 0, 0),
        };
        Self {
            kind,
            variant: (variant % kind.variant_count()) as u8,
            fill,
            line: line_rgb,
            edge,
            line_w_q,
            glow_q,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderParticle {
    pub tex_key: ParticleTexKey,
    pub center_x_dip: f32,
    pub center_y_dip: f32,
    pub size_dip: f32,
    pub rot_rad: f32,
    pub alpha: f32,
}

// ───────────────────────────── RNG / helpers ─────────────────────────────

#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng(if z == 0 { 0x9E37_79B9_7F4A_7C15 } else { z })
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// `C(key, n)`: random index in `0..n`, never a third identical pick in a row; keeps last 4.
fn pick_no_triple(hist: &mut Vec<usize>, n: usize, rng: &mut Rng) -> usize {
    let n = n.max(1);
    let mut cands: Vec<usize> = (0..n).collect();
    if n > 1 && hist.len() >= 2 && hist[hist.len() - 1] == hist[hist.len() - 2] {
        let last = hist[hist.len() - 1];
        cands.retain(|&c| c != last);
    }
    let i = cands[((rng.f64() * cands.len() as f64) as usize).min(cands.len() - 1)];
    hist.push(i);
    if hist.len() > 4 {
        hist.remove(0);
    }
    i
}

fn bezier_ease(c: [f64; 4], x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let [x1, y1, x2, y2] = c;
    let bez = |t: f64, a: f64, b: f64| 3.0 * (1.0 - t).powi(2) * t * a + 3.0 * (1.0 - t) * t * t * b + t * t * t;
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if bez(mid, x1, x2) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(0.5 * (lo + hi), y1, y2)
}

const EASE_FOLLOW: [f64; 4] = [0.25, 0.8, 0.35, 1.0];
const EASE_REST: [f64; 4] = [0.4, 0.0, 0.2, 1.0];
const FOLLOW_S: f64 = 0.220;
const REST_S: f64 = 0.720;
const REST_DELAY_S: f64 = 0.480;
const EXPR_TAIL_S: f64 = 0.420;
const MAX_PARTICLES: usize = 24;
const LOCK_S: f64 = 0.420;
const PAIR_DELAY_S: f64 = 0.260;

/// Keyframes `[tx, ty, scale, rot_deg, opacity, offset]` (linear), `e` = side (+-1), `t` = size scale.
fn keyframes(kind: ParticleKind, e: f64, t: f64) -> Vec<[f64; 6]> {
    match kind {
        ParticleKind::Heart => vec![
            [0.0, 0.0, 0.6, 0.0, 0.0, 0.0],
            [2.0 * e * t, -16.0 * t, 1.0, 3.0 * e, 1.0, 0.16],
            [4.0 * e * t, -32.0 * t, 1.12, 6.0 * e, 1.0, 0.3],
            [6.0 * e * t, -48.0 * t, 0.96, 3.0 * e, 1.0, 0.42],
            [8.0 * e * t, -66.0 * t, 1.0, 0.0, 0.9, 0.58],
            [9.0 * e * t, -88.0 * t, 0.95, -3.0 * e, 0.45, 0.8],
            [10.0 * e * t, -112.0 * t, 0.85, -5.0 * e, 0.0, 1.0],
        ],
        ParticleKind::Note => vec![
            [0.0, 0.0, 0.6, -10.0 * e, 0.0, 0.0],
            [4.0 * e * t, -16.0 * t, 1.0, -12.0 * e, 1.0, 0.14],
            [8.0 * e * t, -28.0 * t, 1.0, 10.0 * e, 1.0, 0.3],
            [12.0 * e * t, -46.0 * t, 1.0, -12.0 * e, 0.95, 0.48],
            [16.0 * e * t, -60.0 * t, 1.0, 10.0 * e, 0.7, 0.64],
            [20.0 * e * t, -80.0 * t, 0.95, -8.0 * e, 0.35, 0.82],
            [23.0 * e * t, -98.0 * t, 0.9, 0.0, 0.0, 1.0],
        ],
        ParticleKind::Anger => vec![
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, -3.0 * t, 1.3, 0.0, 1.0, 0.1],
            [2.0 * e * t, -7.0 * t, 0.88, 0.0, 1.0, 0.22],
            [-2.0 * e * t, -11.0 * t, 1.2, 0.0, 1.0, 0.34],
            [2.0 * e * t, -15.0 * t, 0.9, 0.0, 0.95, 0.48],
            [0.0, -20.0 * t, 1.12, 0.0, 0.7, 0.64],
            [0.0, -27.0 * t, 0.95, 0.0, 0.35, 0.82],
            [0.0, -34.0 * t, 0.75, 0.0, 0.0, 1.0],
        ],
        ParticleKind::Sweat => vec![
            [0.0, -6.0 * t, 0.3, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0, 1.0, 0.14],
            [1.0 * e * t, 12.0 * t, 1.0, 0.0, 1.0, 0.4],
            [2.0 * e * t, 28.0 * t, 1.04, 0.0, 0.7, 0.7],
            [3.0 * e * t, 46.0 * t, 0.9, 0.0, 0.0, 1.0],
        ],
        ParticleKind::Sparkle => vec![
            [0.0, 0.0, 0.3, 0.0, 0.0, 0.0],
            [2.0 * e * t, -8.0 * t, 0.75, 0.0, 1.0, 0.2],
            [5.0 * e * t, -22.0 * t, 1.0, 0.0, 0.9, 0.55],
            [8.0 * e * t, -38.0 * t, 1.3, 0.0, 0.0, 1.0],
        ],
        ParticleKind::Sleepy => vec![
            [0.0, 0.0, 0.45, 8.0 * e, 0.0, 0.0],
            [6.0 * e * t, -12.0 * t, 0.65, 8.0 * e, 1.0, 0.16],
            [22.0 * e * t, -38.0 * t, 0.9, -6.0 * e, 1.0, 0.42],
            [14.0 * e * t, -66.0 * t, 1.1, 8.0 * e, 0.6, 0.7],
            [30.0 * e * t, -100.0 * t, 1.3, -4.0 * e, 0.0, 1.0],
        ],
    }
}

/// Returns `[tx, ty, scale, rot_deg, opacity]` at progress `p` in `[0,1]`.
fn sample_frames(f: &[[f64; 6]], p: f64) -> [f64; 5] {
    let p = p.clamp(0.0, 1.0);
    for i in 1..f.len() {
        let (a, b) = (&f[i - 1], &f[i]);
        if p <= b[5] {
            let span = b[5] - a[5];
            let u = if span <= 0.0 { 1.0 } else { ((p - a[5]) / span).clamp(0.0, 1.0) };
            let mut o = [0.0; 5];
            for k in 0..5 {
                o[k] = a[k] + (b[k] - a[k]) * u;
            }
            return o;
        }
    }
    let l = f[f.len() - 1];
    [l[0], l[1], l[2], l[3], l[4]]
}

// ───────────────────────────── State ─────────────────────────────

#[derive(Debug, Clone)]
struct Particle {
    born: f64,
    life: f64,
    ax: f64,
    ay: f64,
    k: f64,
    frames: Vec<[f64; 6]>,
    key: ParticleTexKey,
}

#[derive(Debug, Clone)]
pub struct PetFxState {
    // movement detection (`pet-fx.js` move/leave)
    sample: Option<(f64, f64, f64)>,
    dir_x: i8,
    dir_y: i8,
    last_rev: f64,
    pub active_until_s: f64,
    // particles
    spawn_timer: Option<f64>,
    pairs: Vec<(f64, i8)>,
    particles: Vec<Particle>,
    rng: Rng,
    side: i8,
    side_idx: [u32; 2],
    lock_until: [f64; 2],
    hist_kind: Vec<usize>,
    hist_var: [Vec<usize>; 6],
    // pat tilt / sway
    seg_from: f64,
    seg_to: f64,
    seg_start: f64,
    seg_dur: f64,
    seg_curve: [f64; 4],
    rest_at: Option<f64>,
    expr: bool,
    expr_off_at: f64,
    sway: f64,
    tilt: f64,
    // pat_say
    say_from: f64,
    say_seen: f64,
    say_last: Option<f64>,
}

impl Default for PetFxState {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x1234_5678);
        Self::with_seed(seed)
    }
}

fn active_kinds(cfg: &CharacterConfig) -> Vec<ParticleKind> {
    cfg.pet_kinds.iter().filter_map(|s| ParticleKind::from_name(s)).collect()
}

fn spawn_gap_s(cfg: &CharacterConfig) -> f64 {
    let amount = if cfg.pet_amount == 0.0 || cfg.pet_amount.is_nan() { 4.0 } else { cfg.pet_amount };
    (1800.0 / amount.clamp(1.0, 10.0).powf(0.8)).round() / 1000.0
}

impl PetFxState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_seed(seed: u64) -> Self {
        Self {
            sample: None,
            dir_x: 0,
            dir_y: 0,
            last_rev: f64::NEG_INFINITY,
            active_until_s: f64::NEG_INFINITY,
            spawn_timer: None,
            pairs: Vec::new(),
            particles: Vec::new(),
            rng: Rng::new(seed),
            side: 1,
            side_idx: [0; 2],
            lock_until: [f64::NEG_INFINITY; 2],
            hist_kind: Vec::new(),
            hist_var: Default::default(),
            seg_from: 0.0,
            seg_to: 0.0,
            seg_start: 0.0,
            seg_dur: FOLLOW_S,
            seg_curve: EASE_FOLLOW,
            rest_at: None,
            expr: false,
            expr_off_at: f64::NEG_INFINITY,
            sway: 3.0,
            tilt: 3.0,
            say_from: f64::NEG_INFINITY,
            say_seen: f64::NEG_INFINITY,
            say_last: None,
        }
    }

    pub fn is_active(&self, now_s: f64) -> bool {
        now_s <= self.active_until_s
    }

    /// Number of live particles (for diagnostics / tests).
    pub fn particle_count(&self) -> usize {
        self.particles.len()
    }

    /// Drops all live particles and pending spawns.
    pub fn clear_particles(&mut self) {
        self.particles.clear();
        self.pairs.clear();
        self.spawn_timer = None;
    }

    // ── pat tilt/sway ──

    fn seg_value(&self, t: f64) -> f64 {
        let p = ((t - self.seg_start) / self.seg_dur).clamp(0.0, 1.0);
        self.seg_from + (self.seg_to - self.seg_from) * bezier_ease(self.seg_curve, p)
    }

    fn o_at(&self, now: f64) -> f64 {
        match self.rest_at {
            Some(r) if now >= r => {
                let f = self.seg_value(r);
                f * (1.0 - bezier_ease(EASE_REST, ((now - r) / REST_S).clamp(0.0, 1.0)))
            }
            _ => self.seg_value(now),
        }
    }

    /// Folds an elapsed 480ms rest countdown into a plain ease-back segment.
    fn commit_rest(&mut self, now: f64) {
        if let Some(r) = self.rest_at {
            if now >= r {
                let f = self.seg_value(r);
                self.seg_from = f;
                self.seg_to = 0.0;
                self.seg_start = r;
                self.seg_dur = REST_S;
                self.seg_curve = EASE_REST;
                self.expr_off_at = r + EXPR_TAIL_S;
                self.rest_at = None;
            }
        }
    }

    fn expr_on(&self, now: f64) -> bool {
        self.expr
            && match self.rest_at {
                Some(r) => now < r + EXPR_TAIL_S,
                None => now < self.expr_off_at,
            }
    }

    /// `(tx_800, rot_rad, expr_on)`: translation in 800-unit rig space, head rotation, and whether the
    /// `head4`/`head3` petting expression is shown. With `flip == true` the values are negated, i.e. they
    /// are expressed in the mirrored (un-flipped sprite) frame so the screen-space motion stays correct.
    pub fn pat_transform_800(&self, now_s: f64, flip: bool) -> (f32, f32, bool) {
        let o = self.o_at(now_s);
        let mut tx = o * self.sway * 4.0;
        let mut rot = (o * self.tilt).to_radians();
        if flip {
            tx = -tx;
            rot = -rot;
        }
        (tx as f32, rot as f32, self.expr_on(now_s))
    }

    fn pat_follow(&mut self, now: f64, o: f64, cfg: &CharacterConfig) {
        self.commit_rest(now);
        let cur = self.o_at(now);
        self.seg_from = cur;
        self.seg_to = o;
        self.seg_start = now;
        self.seg_dur = FOLLOW_S;
        self.seg_curve = EASE_FOLLOW;
        self.sway = cfg.pet_sway;
        self.tilt = cfg.pet_tilt;
        self.expr = !cfg.head4.is_empty() || !cfg.head3.is_empty();
        self.expr_off_at = f64::INFINITY;
        self.rest_at = Some(now + REST_DELAY_S);
    }

    fn pat_say(&mut self, now: f64, cfg: &CharacterConfig) -> bool {
        if !cfg.pat_lines.iter().any(|s| !s.trim().is_empty()) {
            return false;
        }
        if now - self.say_seen > 0.7 {
            self.say_from = now;
        }
        self.say_seen = now;
        if now - self.say_from < 0.5 {
            return false;
        }
        let gap = if cfg.pat_line_gap == 0.0 || cfg.pat_line_gap.is_nan() { 8.0 } else { cfg.pat_line_gap };
        let gap = gap.max(3.0);
        if let Some(l) = self.say_last {
            if now - l < gap {
                return false;
            }
        }
        self.say_last = Some(now);
        true
    }

    // ── movement ──

    fn register_move(&mut self, x: f64, y: f64, now: f64) {
        let prev = match self.sample {
            Some(p) if now - p.2 <= 0.3 => p,
            _ => {
                self.sample = Some((x, y, now));
                self.dir_x = 0;
                self.dir_y = 0;
                return;
            }
        };
        let (dx, dy) = (x - prev.0, y - prev.1);
        if dx.hypot(dy) < 0.5 {
            return;
        }
        let sx: i8 = if dx.abs() > 1.5 { dx.signum() as i8 } else { 0 };
        let sy: i8 = if dy.abs() > 1.5 { dy.signum() as i8 } else { 0 };
        if (sx != 0 && self.dir_x != 0 && sx != self.dir_x) || (sy != 0 && self.dir_y != 0 && sy != self.dir_y) {
            self.last_rev = now;
        }
        if sx != 0 {
            self.dir_x = sx;
        }
        if sy != 0 {
            self.dir_y = sy;
        }
        self.sample = Some((x, y, now));
        if now - self.last_rev > 0.900 {
            return;
        }
        let was_active = self.active_until_s > now;
        self.active_until_s = now + 0.450;
        if !was_active && self.spawn_timer.is_none() {
            self.spawn_timer = Some(now + 0.120);
        }
    }

    /// Feed one pointer-move (coordinates local to the pet window, DIP). Returns `true` when the
    /// caller should fire `say("patLines")` (the caller still applies its own bubble-busy check).
    pub fn on_move(
        &mut self,
        local_x_dip: f64,
        local_y_dip: f64,
        now_s: f64,
        cfg: &CharacterConfig,
        head_box_dip: RectDip,
        _size_dip: f64,
    ) -> bool {
        if !cfg.pet_on {
            return false;
        }
        self.register_move(local_x_dip, local_y_dip, now_s);
        if !self.is_active(now_s) {
            return false;
        }
        let half = (head_box_dip.w * 0.5).max(1.0);
        let o = ((local_x_dip - (head_box_dip.x + head_box_dip.w * 0.5)) / half).clamp(-1.0, 1.0);
        self.pat_follow(now_s, o, cfg);
        self.pat_say(now_s, cfg)
    }

    /// Pointer left the pettable area.
    pub fn leave(&mut self, now_s: f64) {
        self.sample = None;
        self.dir_x = 0;
        self.dir_y = 0;
        self.active_until_s = f64::NEG_INFINITY;
        if let Some(r) = self.rest_at {
            self.rest_at = Some(r.min(now_s + REST_DELAY_S));
        }
    }

    /// Instant reset (press / drag / menu): no ease-back, no expression tail.
    pub fn reset_immediate(&mut self) {
        self.sample = None;
        self.dir_x = 0;
        self.dir_y = 0;
        self.active_until_s = f64::NEG_INFINITY;
        self.seg_from = 0.0;
        self.seg_to = 0.0;
        self.rest_at = None;
        self.expr = false;
        self.expr_off_at = f64::NEG_INFINITY;
    }

    // ── particles ──

    fn spawn_one(
        &mut self,
        t: f64,
        cfg: &CharacterConfig,
        hb: RectDip,
        size_dip: f64,
        kind: ParticleKind,
        check_lock: bool,
    ) -> bool {
        self.particles.retain(|p| t < p.born + p.life);
        if self.particles.len() >= MAX_PARTICLES {
            return false;
        }
        self.side = -self.side;
        let l = self.side;
        let si = if l < 0 { 0 } else { 1 };
        if check_lock && self.lock_until[si] > t {
            return false;
        }
        self.lock_until[si] = self.lock_until[si].max(t + LOCK_S);
        let v = self.side_idx[si] as usize;
        self.side_idx[si] += 1;
        let f = (size_dip / 200.0).max(0.3);

        let mut k = (if cfg.pet_size == 0.0 || cfg.pet_size.is_nan() { 26.0 } else { cfg.pet_size }) * f;
        if cfg.pet_size_mode != "fixed" {
            k *= 0.7 + 0.6 * self.rng.f64();
        }

        let variant = pick_no_triple(&mut self.hist_var[kind.index()], kind.variant_count(), &mut self.rng);
        let key = ParticleTexKey::resolve(cfg, kind, variant, self.rng.f64());
        if key.edge == PetEdge::Glow {
            let s_gs = (3.2 * glow_size_unclamped(cfg)).round();
            let lw = 0.0;
            k *= (100.0 + 2.0 * (6.0 + lw + s_gs)) / (100.0 + 2.0 * (6.0 + lw));
        }

        let table = kind.anchors();
        let a = table[v % table.len()];
        let fx = a[0] + 0.03 * self.rng.f64();
        let fy = a[1] + 0.05 * self.rng.f64();
        let ax = if l < 0 { hb.x + hb.w * fx } else { hb.x + hb.w * (1.0 - fx) };
        let ay = hb.y + hb.h * fy;

        let speed = if cfg.pet_speed == 0.0 || cfg.pet_speed.is_nan() { 1100.0 } else { cfg.pet_speed };
        let life = kind.life_ms() * (speed.max(400.0) / 1100.0) * (0.9 + 0.2 * self.rng.f64()) / 1000.0;

        self.particles.push(Particle {
            born: t,
            life,
            ax,
            ay,
            k,
            frames: keyframes(kind, l as f64, f),
            key,
        });
        true
    }

    /// Outer `T()`: pick kind (history advances even when blocked), spawn, queue note pair.
    fn spawn_auto(&mut self, t: f64, cfg: &CharacterConfig, hb: RectDip, size_dip: f64, kinds: &[ParticleKind]) {
        if self.particles.len() > MAX_PARTICLES {
            return;
        }
        let i = pick_no_triple(&mut self.hist_kind, kinds.len(), &mut self.rng);
        let kind = kinds[i];
        if self.spawn_one(t, cfg, hb, size_dip, kind, true) && kind == ParticleKind::Note {
            self.pairs.push((t + PAIR_DELAY_S, self.side));
        }
    }

    /// Advances spawning and returns the live particles at `now_s`.
    pub fn step(&mut self, now_s: f64, cfg: &CharacterConfig, char_box_dip: RectDip, size_dip: f64) -> Vec<RenderParticle> {
        self.commit_rest(now_s);
        let hb = RectDip { h: char_box_dip.h * 0.55, ..char_box_dip };
        let kinds = active_kinds(cfg);
        let gap = spawn_gap_s(cfg);

        for _ in 0..4096 {
            let timer = self.spawn_timer.filter(|&t| t <= now_s);
            let pair = self
                .pairs
                .iter()
                .enumerate()
                .filter(|(_, p)| p.0 <= now_s)
                .min_by(|a, b| a.1 .0.partial_cmp(&b.1 .0).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, p)| (i, p.0));
            match (timer, pair) {
                (None, None) => break,
                (Some(t), p) if p.map_or(true, |(_, pt)| t <= pt) => {
                    self.spawn_timer = None;
                    if !cfg.pet_on || t > self.active_until_s {
                        continue;
                    }
                    if !kinds.is_empty() {
                        self.spawn_auto(t, cfg, hb, size_dip, &kinds);
                    }
                    self.spawn_timer = Some(t + gap * (0.85 + 0.3 * self.rng.f64()));
                }
                (_, Some((i, pt))) => {
                    let (_, e) = self.pairs.remove(i);
                    self.side = -e;
                    self.spawn_one(pt, cfg, hb, size_dip, ParticleKind::Note, false);
                }
                _ => break,
            }
        }

        self.particles.retain(|p| now_s < p.born + p.life);
        self.particles
            .iter()
            .map(|p| {
                let prog = (now_s - p.born) / p.life;
                let [tx, ty, sc, rot, op] = sample_frames(&p.frames, prog);
                RenderParticle {
                    tex_key: p.key,
                    center_x_dip: (p.ax + tx) as f32,
                    center_y_dip: (p.ay + ty) as f32,
                    size_dip: (p.k * sc) as f32,
                    rot_rad: rot.to_radians() as f32,
                    alpha: op.clamp(0.0, 1.0) as f32,
                }
            })
            .collect()
    }
}

// ───────────────────────────── Vector rasterizer ─────────────────────────────

/// Side length of every particle texture.
pub const TEX_SIZE: u32 = 128;

#[derive(Clone, Copy)]
enum Part {
    Fill(&'static str),
    /// path, stroke width, rotation (deg) about (50,50)
    Stroke(&'static str, f32, f32),
    /// cx, cy, rx, ry, rotation (deg) about (cx,cy)
    Ellipse(f32, f32, f32, f32, f32),
}

fn particle_parts(kind: ParticleKind, variant: u8) -> Vec<Part> {
    match kind {
        ParticleKind::Heart => vec![Part::Fill("M50 86 C 22 66 9 51 9 34 C 9 21 19 11 32 11 C 40 11 46 15 50 22 C 54 15 60 11 68 11 C 81 11 91 21 91 34 C 91 51 78 66 50 86 Z")],
        ParticleKind::Note => match variant % 4 {
            0 => vec![Part::Stroke("M58 72 V 12", 7.0, 0.0), Part::Ellipse(46.0, 74.0, 16.0, 12.0, -18.0)],
            1 => vec![
                Part::Stroke("M50 72 V 12 C 54 24 66 28 72 38 C 77 46 74 56 68 62 C 71 52 68 44 58 40", 7.0, 0.0),
                Part::Ellipse(38.0, 74.0, 16.0, 12.0, -18.0),
            ],
            2 => vec![
                Part::Stroke("M36 74 V 24 L 80 15 V 64", 7.0, 0.0),
                Part::Fill("M36 22 L 80 13 L 80 29 L 36 38 Z"),
                Part::Ellipse(26.0, 76.0, 14.0, 11.0, -18.0),
                Part::Ellipse(70.0, 67.0, 14.0, 11.0, -18.0),
            ],
            _ => vec![
                Part::Stroke("M30 76 V 26 L 82 17 V 66", 7.0, 0.0),
                Part::Fill("M30 24 L 82 15 L 82 27 L 30 36 Z M30 40 L 82 31 L 82 43 L 30 52 Z"),
                Part::Ellipse(20.0, 78.0, 13.0, 10.0, -18.0),
                Part::Ellipse(72.0, 69.0, 13.0, 10.0, -18.0),
            ],
        },
        ParticleKind::Anger => [0.0, 90.0, 180.0, 270.0]
            .iter()
            .map(|&r| Part::Stroke("M34 8 Q 34 34 8 34", 13.0, r))
            .collect(),
        ParticleKind::Sweat => vec![Part::Fill("M52 8 C 63 28 80 44 80 63 C 80 80 67 92 52 92 C 37 92 24 80 24 63 C 24 44 41 28 52 8 Z")],
        ParticleKind::Sparkle => vec![Part::Fill("M50 6 C 53 32 58 42 86 50 C 58 58 53 68 50 94 C 47 68 42 58 14 50 C 42 42 47 32 50 6 Z")],
        ParticleKind::Sleepy => vec![Part::Stroke("M28 26 H 72 L 28 74 H 72", 14.0, 0.0)],
    }
}

enum Tok {
    Cmd(char),
    Num(f32),
}

/// Minimal absolute-only SVG path parser (M L H V C Q Z) for the particle icons.
fn parse_svg_path(d: &str) -> Option<Path> {
    let chars: Vec<char> = d.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphabetic() {
            toks.push(Tok::Cmd(c));
            i += 1;
        } else if c.is_ascii_digit() || c == '-' || c == '.' || c == '+' {
            let s = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let txt: String = chars[s..i].iter().collect();
            toks.push(Tok::Num(txt.parse::<f32>().ok()?));
        } else {
            i += 1;
        }
    }
    let num = |idx: &mut usize| -> Option<f32> {
        match toks.get(*idx) {
            Some(Tok::Num(v)) => {
                *idx += 1;
                Some(*v)
            }
            _ => None,
        }
    };
    let mut pb = PathBuilder::new();
    let (mut cx, mut cy, mut sx, mut sy) = (0f32, 0f32, 0f32, 0f32);
    let mut cmd = ' ';
    let mut i = 0;
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
            if c == 'Z' || c == 'z' {
                pb.close();
                cx = sx;
                cy = sy;
                continue;
            }
        }
        match cmd {
            'M' => {
                let (x, y) = (num(&mut i)?, num(&mut i)?);
                pb.move_to(x, y);
                (cx, cy, sx, sy) = (x, y, x, y);
                cmd = 'L';
            }
            'L' => {
                let (x, y) = (num(&mut i)?, num(&mut i)?);
                pb.line_to(x, y);
                (cx, cy) = (x, y);
            }
            'H' => {
                cx = num(&mut i)?;
                pb.line_to(cx, cy);
            }
            'V' => {
                cy = num(&mut i)?;
                pb.line_to(cx, cy);
            }
            'C' => {
                let (a, b, c, d2, x, y) = (num(&mut i)?, num(&mut i)?, num(&mut i)?, num(&mut i)?, num(&mut i)?, num(&mut i)?);
                pb.cubic_to(a, b, c, d2, x, y);
                (cx, cy) = (x, y);
            }
            'Q' => {
                let (a, b, x, y) = (num(&mut i)?, num(&mut i)?, num(&mut i)?, num(&mut i)?);
                pb.quad_to(a, b, x, y);
                (cx, cy) = (x, y);
            }
            _ => return None,
        }
    }
    pb.finish()
}

fn draw_part(pm: &mut Pixmap, part: Part, base: Transform, rgb: [u8; 3], line_pass: bool, lw: f32) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
    paint.anti_alias = true;
    let stroke = |w: f32| Stroke {
        width: w,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    match part {
        Part::Fill(d) => {
            if let Some(path) = parse_svg_path(d) {
                pm.fill_path(&path, &paint, FillRule::Winding, base, None);
                if line_pass {
                    pm.stroke_path(&path, &paint, &stroke(2.0 * lw), base, None);
                }
            }
        }
        Part::Stroke(d, w, rot) => {
            if let Some(path) = parse_svg_path(d) {
                let ts = if rot != 0.0 { base.pre_concat(Transform::from_rotate_at(rot, 50.0, 50.0)) } else { base };
                let width = if line_pass { w + 2.0 * lw } else { w };
                pm.stroke_path(&path, &paint, &stroke(width), ts, None);
            }
        }
        Part::Ellipse(cx, cy, rx, ry, rot) => {
            let path = Rect::from_xywh(cx - rx, cy - ry, 2.0 * rx, 2.0 * ry).and_then(PathBuilder::from_oval);
            if let Some(path) = path {
                let ts = base.pre_concat(Transform::from_rotate_at(rot, cx, cy));
                pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
                if line_pass {
                    pm.stroke_path(&path, &paint, &stroke(2.0 * lw), ts, None);
                }
            }
        }
    }
}

fn gaussian_blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = ((sigma * 3.0).ceil() as isize).max(1);
    let mut k: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let xx = x as isize + j as isize - r;
                if xx >= 0 && (xx as usize) < w {
                    acc += src[y * w + xx as usize] * kv;
                }
            }
            tmp[y * w + x] = acc;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let yy = y as isize + j as isize - r;
                if yy >= 0 && (yy as usize) < h {
                    acc += tmp[yy as usize * w + x] * kv;
                }
            }
            out[y * w + x] = acc;
        }
    }
    out
}

/// Rasterizes one particle into a `TEX_SIZE`² premultiplied RGBA8 buffer covering the SVG viewBox
/// `[-p .. 100+p]` (p = 6 + line width + glow padding), exactly like `PetFx.svg`.
pub fn rasterize_particle(key: &ParticleTexKey) -> Vec<u8> {
    let n = TEX_SIZE;
    let lw = if key.edge == PetEdge::Line { key.line_w_q as f32 / 2.0 } else { 0.0 };
    let d = if key.edge == PetEdge::Glow { key.glow_q as f32 / 2.0 } else { 0.0 };
    let glow_pad = if key.edge == PetEdge::Glow { (3.2 * d).round() } else { 0.0 };
    let p = 6.0 + lw + glow_pad;
    let sc = n as f32 / (100.0 + 2.0 * p);
    let base = Transform::from_row(sc, 0.0, 0.0, sc, p * sc, p * sc);
    let parts = particle_parts(key.kind, key.variant);
    let mut pm = match Pixmap::new(n, n) {
        Some(p) => p,
        None => return vec![0u8; (n * n * 4) as usize],
    };
    if key.edge == PetEdge::Line && lw > 0.0 {
        for &part in &parts {
            draw_part(&mut pm, part, base, key.line, true, lw);
        }
    }
    for &part in &parts {
        draw_part(&mut pm, part, base, key.fill, false, 0.0);
    }
    if key.edge != PetEdge::Glow {
        return pm.take();
    }

    let (w, h) = (n as usize, n as usize);
    let alpha: Vec<f32> = pm.data().chunks_exact(4).map(|px| px[3] as f32 / 255.0).collect();
    let blurred = gaussian_blur(&alpha, w, h, (d * sc).max(0.3));
    let opacity = (0.6 + 0.03 * d).min(0.95);
    let lc = [key.line[0] as f32 / 255.0, key.line[1] as f32 / 255.0, key.line[2] as f32 / 255.0];
    let data = pm.data_mut();
    for i in 0..w * h {
        let fg = &mut data[i * 4..i * 4 + 4];
        let ga = (blurred[i] * opacity).clamp(0.0, 1.0);
        let fa = fg[3] as f32 / 255.0;
        let inv = 1.0 - fa;
        let oa = (fa + ga * inv).clamp(0.0, 1.0);
        let mut out = [0u8; 4];
        for c in 0..3 {
            let v = fg[c] as f32 / 255.0 + lc[c] * ga * inv;
            out[c] = (v.clamp(0.0, oa) * 255.0).round() as u8;
        }
        out[3] = (oa * 255.0).round() as u8;
        for c in 0..3 {
            out[c] = out[c].min(out[3]);
        }
        fg.copy_from_slice(&out);
    }
    pm.take()
}

// ───────────────────────────── GPU atlas ─────────────────────────────

/// Lazily rasterized + uploaded particle textures. Cardinality is bounded by `ParticleTexKey`
/// (candy: 7 colours, rainbow: 24 hue buckets), so GPU memory stays flat.
#[derive(Default)]
pub struct PetParticleAtlas {
    cache: HashMap<ParticleTexKey, GpuTexture>,
}

impl PetParticleAtlas {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Drop all textures (call when the character/config changes).
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// `cfg` is accepted for API symmetry; the key already carries every resolved style input.
    pub fn get_or_upload(
        &mut self,
        gpu: &GpuContext,
        key: &ParticleTexKey,
        _cfg: &CharacterConfig,
    ) -> Result<&GpuTexture, String> {
        if !self.cache.contains_key(key) {
            let px = rasterize_particle(key);
            let tex = GpuTexture::from_rgba(gpu, TEX_SIZE, TEX_SIZE, &px)?;
            self.cache.insert(*key, tex);
        }
        self.cache.get(key).ok_or_else(|| "particle texture missing".to_string())
    }
}

// ───────────────────────────── Tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kinds: &[&str]) -> CharacterConfig {
        let mut c = CharacterConfig::default();
        c.pet_on = true;
        c.pet_kinds = kinds.iter().map(|s| s.to_string()).collect();
        c.pet_size_mode = "fixed".to_string();
        c
    }

    const HB: RectDip = RectDip { x: 50.0, y: 0.0, w: 100.0, h: 100.0, cx: None };

    #[test]
    fn direction_flip_activates_but_monotonic_does_not() {
        let c = cfg(&["heart"]);
        let mut fx = PetFxState::with_seed(1);
        fx.on_move(0.0, 0.0, 0.0, &c, HB, 200.0);
        fx.on_move(10.0, 0.0, 0.05, &c, HB, 200.0);
        assert!(!fx.is_active(0.06), "monotonic start must not activate");
        fx.on_move(0.0, 0.0, 0.10, &c, HB, 200.0);
        assert!(fx.is_active(0.15), "reversal activates");

        // keep moving monotonically: must lapse after ~900ms + 450ms
        let mut x = 0.0;
        let mut t = 0.10;
        while t < 2.0 {
            t += 0.016;
            x -= 5.0;
            fx.on_move(x, 0.0, t, &c, HB, 200.0);
        }
        assert!(!fx.is_active(2.1));
        assert!(fx.is_active(0.5) || fx.active_until_s > 0.0);
    }

    #[test]
    fn gap_resets_sample() {
        let c = cfg(&["heart"]);
        let mut fx = PetFxState::with_seed(2);
        fx.on_move(0.0, 0.0, 0.0, &c, HB, 200.0);
        fx.on_move(10.0, 0.0, 0.05, &c, HB, 200.0);
        fx.on_move(0.0, 0.0, 0.5, &c, HB, 200.0); // >300ms gap: reset, no reversal
        assert!(!fx.is_active(0.5));
    }

    #[test]
    fn caps_at_24_particles() {
        let mut c = cfg(&["heart"]);
        c.pet_amount = 10.0;
        c.pet_speed = 20000.0;
        let mut fx = PetFxState::with_seed(3);
        let (mut t, mut sign, mut max_n) = (0.0, 1.0, 0usize);
        let mut last = 0usize;
        while t < 12.0 {
            sign = -sign;
            fx.on_move(100.0 + sign * 5.0, 10.0, t, &c, HB, 200.0);
            let out = fx.step(t, &c, HB, 200.0);
            max_n = max_n.max(out.len());
            last = out.len();
            t += 0.016;
        }
        assert!(max_n <= 24);
        assert_eq!(last, 24);
    }

    #[test]
    fn note_pairs_260ms_later() {
        let c = cfg(&["note"]);
        let mut fx = PetFxState::with_seed(4);
        let (mut t, mut sign) = (0.0, 1.0);
        while t < 0.9 {
            sign = -sign;
            fx.on_move(100.0 + sign * 5.0, 10.0, t, &c, HB, 200.0);
            fx.step(t, &c, HB, 200.0);
            t += 0.008;
        }
        let notes: Vec<_> = fx.particles.iter().filter(|p| p.key.kind == ParticleKind::Note).collect();
        assert!(notes.len() >= 2, "got {}", notes.len());
        assert!((notes[1].born - notes[0].born - 0.26).abs() < 1e-6);
    }

    #[test]
    fn no_three_same_in_a_row() {
        let mut rng = Rng::new(9);
        let mut h = Vec::new();
        let picks: Vec<usize> = (0..300).map(|_| pick_no_triple(&mut h, 4, &mut rng)).collect();
        assert!(picks.windows(3).all(|w| !(w[0] == w[1] && w[1] == w[2])));
    }

    #[test]
    fn pat_transform_sways_and_rests() {
        let mut c = cfg(&["heart"]);
        c.head4 = "h4.png".to_string();
        let mut fx = PetFxState::with_seed(5);
        fx.on_move(0.0, 0.0, 0.0, &c, HB, 200.0);
        fx.on_move(10.0, 0.0, 0.05, &c, HB, 200.0);
        fx.on_move(0.0, 0.0, 0.10, &c, HB, 200.0); // reversal -> petting becomes active
        fx.on_move(150.0, 0.0, 0.15, &c, HB, 200.0); // reversal, far right of head centre -> o = 1
        let (tx, rot, expr) = fx.pat_transform_800(0.5, false);
        assert!(tx > 11.0 && rot > 0.0 && expr);
        assert_eq!(fx.pat_transform_800(0.5, true).0, -tx);
        let (tx2, _, expr2) = fx.pat_transform_800(5.0, false);
        assert!(tx2.abs() < 1e-3 && !expr2);
        fx.reset_immediate();
        assert_eq!(fx.pat_transform_800(0.2, false), (0.0, 0.0, false));
    }

    #[test]
    fn pat_say_rules() {
        let mut c = cfg(&["heart"]);
        c.pat_lines = vec!["  ".to_string(), "mmm".to_string()];
        let mut fx = PetFxState::with_seed(6);
        let mut fired = Vec::new();
        let (mut t, mut sign) = (0.0, 1.0);
        while t < 1.0 {
            sign = -sign;
            if fx.on_move(100.0 + sign * 5.0, 0.0, t, &c, HB, 200.0) {
                fired.push(t);
            }
            t += 0.016;
        }
        assert_eq!(fired.len(), 1);
        assert!(fired[0] >= 0.5);
    }

    #[test]
    fn area_mask_cut_and_hit() {
        let spr = RgbaSprite { width: 10, height: 10, pixels: vec![255; 10 * 10 * 4] };
        let m = PetAreaMask::build(Some(&spr), None, 50.0);
        assert!(m.hit(0.5, 0.3));
        assert!(!m.hit(0.5, 0.9));
        assert!(PetAreaMask::build(None, None, 0.0).hit(0.5, 0.9));
    }

    #[test]
    fn rasterizer_valid_for_all_kinds_variants_edges() {
        for kind in ParticleKind::ALL {
            for v in 0..kind.variant_count() as u8 {
                for edge in [PetEdge::Glow, PetEdge::Line, PetEdge::None] {
                    let key = ParticleTexKey {
                        kind,
                        variant: v,
                        fill: [255, 143, 177],
                        line: [255, 255, 255],
                        edge,
                        line_w_q: if edge == PetEdge::Line { 4 } else { 0 },
                        glow_q: if edge == PetEdge::Glow { 10 } else { 0 },
                    };
                    let px = rasterize_particle(&key);
                    assert_eq!(px.len(), (TEX_SIZE * TEX_SIZE * 4) as usize);
                    assert!(px.chunks_exact(4).any(|p| p[3] > 0), "{kind:?}/{v}/{edge:?} empty");
                    assert!(px.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
                }
            }
        }
    }

    #[test]
    fn key_cardinality_is_bounded() {
        let mut c = cfg(&["heart"]);
        c.pet_color_mode = "rainbow".to_string();
        let mut set = std::collections::HashSet::new();
        let mut rng = Rng::new(7);
        for _ in 0..5000 {
            set.insert(ParticleTexKey::resolve(&c, ParticleKind::Heart, 0, rng.f64()));
        }
        assert!(set.len() <= 24);
        c.pet_color_mode = "candy".to_string();
        set.clear();
        for _ in 0..5000 {
            set.insert(ParticleTexKey::resolve(&c, ParticleKind::Heart, 0, rng.f64()));
        }
        assert!(set.len() <= 7);
    }
}
