use crate::assets::RgbaSprite;
use fontdue::{Font, FontSettings};
use std::collections::HashMap;
use std::f64::consts::PI;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::OnceLock;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Point,
    RadialGradient, Rect, SpreadMode, Stroke, StrokeDash, Transform,
};

static PRIMARY_FONT: OnceLock<Option<Font>> = OnceLock::new();
static FALLBACK_FONT: OnceLock<Option<Font>> = OnceLock::new();

fn load_font_file(path: &str) -> Option<Font> {
    let bytes = std::fs::read(path).ok()?;
    Font::from_bytes(bytes, FontSettings::default()).ok()
}

fn primary_font() -> Option<&'static Font> {
    PRIMARY_FONT
        .get_or_init(|| {
            load_font_file(r"C:\Windows\Fonts\segoeui.ttf")
                .or_else(|| load_font_file(r"C:\Windows\Fonts\malgun.ttf"))
        })
        .as_ref()
}

fn fallback_font() -> Option<&'static Font> {
    FALLBACK_FONT
        .get_or_init(|| load_font_file(r"C:\Windows\Fonts\malgun.ttf"))
        .as_ref()
}

fn pick_font_for_char(ch: char) -> Option<&'static Font> {
    let primary = primary_font();
    if let Some(f) = primary {
        if f.lookup_glyph_index(ch) != 0 {
            return Some(f);
        }
    }
    if let Some(fb) = fallback_font() {
        if fb.lookup_glyph_index(ch) != 0 {
            return Some(fb);
        }
    }
    primary.or_else(fallback_font)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RadialItemKind {
    Top,
    Parts,
    Cam,
    Set,
    Off,
    Quit,
    PartToggle(String),
    PartsAll,
    CamShot,
    CamKeyBg(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadialAction {
    ToggleTop,
    TogglePart(String),
    SetAllParts(bool),
    Shot,
    SetKeyBg(String),
    OpenSettings,
    ClosePet,
    QuitAll,
}

#[derive(Debug, Clone)]
pub struct RadialItemLayout {
    pub id: String,
    pub kind: RadialItemKind,
    pub label: String,
    pub prefix: Option<String>,
    pub icon: String,
    pub x: f64,
    pub y: f64,
    #[allow(dead_code)]
    pub angle: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub on: Option<bool>,
    pub radio: bool,
    pub has_sub: bool,
    pub swatch: Option<String>,
    #[allow(dead_code)]
    pub closes_on_click: bool,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct RadialMenuState {
    pub open: bool,
    pub open_at_s: f64,
    pub closing_at_s: Option<f64>,
    pub last_interact_s: f64,
    pub open_sub: Option<String>,
    pub hovered_id: Option<String>,
    pub root_items: Vec<RadialItemLayout>,
    pub sub_items: Vec<RadialItemLayout>,
    pub center_x: f64,
    pub center_y: f64,
    pub button_size: f64,
    pub radius: f64,
    pub base_radius: f64,
    pub bounds: (f64, f64, f64, f64),
    pub always_on_top: bool,
    pub key_bg: String,
    pub parts: Vec<(String, String, bool)>,
    pub part_icons: HashMap<String, String>,
    pub lang: String,
    pub lite_or_reduced: bool,
    closing_sub_items: Vec<RadialItemLayout>,
    closing_sub_at_s: Option<f64>,
    sub_opened_at_s: f64,
    last_render_hash: u64,
}

pub fn compute_button_size(size_dip: f64) -> f64 {
    2.0 * ((34.0 + 0.08 * size_dip).clamp(40.0, 56.0) * 0.5).round()
}

pub fn compute_radius(size_dip: f64, button_size: f64) -> f64 {
    (48.0 + 0.42 * size_dip).clamp(1.75 * button_size, 230.0).round()
}

pub fn compute_allowed_bounds(
    win_w: f64,
    win_h: f64,
    area: Option<(f64, f64, f64, f64)>,
    button_size: f64,
) -> (f64, f64, f64, f64) {
    let pad = button_size * 0.5 + 6.0;
    let (al, at, ar, ab) = area.unwrap_or((0.0, 0.0, win_w, win_h));
    let mut l = al.max(0.0) + pad;
    let mut t = at.max(0.0) + pad;
    let mut r = win_w.min(ar) - pad;
    let mut b = win_h.min(ab) - pad;
    if l > r {
        let mid = 0.5 * (l + r);
        l = mid;
        r = mid;
    }
    if t > b {
        let mid = 0.5 * (t + b);
        t = mid;
        b = mid;
    }
    (l, t, r, b)
}

#[derive(Clone, Copy)]
enum Lang {
    Ko,
    En,
    Ja,
    Zh,
}

fn parse_lang(lang: &str) -> Lang {
    let l = lang.trim().to_ascii_lowercase();
    if l.starts_with("ko") {
        Lang::Ko
    } else if l.starts_with("ja") {
        Lang::Ja
    } else if l.starts_with("zh") {
        Lang::Zh
    } else {
        Lang::En
    }
}

fn tr(lang: &str, ko: &'static str) -> &'static str {
    match parse_lang(lang) {
        Lang::Ko => ko,
        Lang::En => match ko {
            "늘 앞으로 고정" => "Always on top",
            "파츠 보이기" => "Show parts",
            "촬영 관련" => "Capture mode",
            "촬영하기…" => "Take a shot…",
            "방송용 배경색 (OBS)" => "Broadcast background (OBS)",
            "끄기 (투명)" => "Off (transparent)",
            "초록" => "Green",
            "파랑" => "Blue",
            "분홍" => "Pink",
            "설정 열기…" => "Open settings…",
            "이 캐릭터만 끄기" => "Close this character only",
            "전부 종료" => "Close all characters and quit",
            "전부 끄기" => "Turn all off",
            "전부 켜기" => "Turn all on",
            "켜짐" => "On",
            "꺼짐" => "Off",
            _ => ko,
        },
        Lang::Ja => match ko {
            "늘 앞으로 고정" => "常に最前面",
            "파츠 보이기" => "パーツを表示",
            "촬영 관련" => "撮影モード",
            "촬영하기…" => "撮影する…",
            "방송용 배경색 (OBS)" => "配信用の背景色 (OBS)",
            "끄기 (투명)" => "オフ（透明）",
            "초록" => "緑",
            "파랑" => "青",
            "분홍" => "ピンク",
            "설정 열기…" => "設定を開く…",
            "이 캐릭터만 끄기" => "このキャラクターだけ閉じる",
            "전부 종료" => "すべてのキャラを閉じて終了",
            "전부 끄기" => "すべてオフ",
            "전부 켜기" => "すべてオン",
            "켜짐" => "オン",
            "꺼짐" => "オフ",
            _ => ko,
        },
        Lang::Zh => match ko {
            "늘 앞으로 고정" => "永遠在最上層",
            "파츠 보이기" => "顯示配件",
            "촬영 관련" => "拍攝模式",
            "촬영하기…" => "拍攝…",
            "방송용 배경색 (OBS)" => "直播用背景色 (OBS)",
            "끄기 (투명)" => "關閉（透明）",
            "초록" => "綠色",
            "파랑" => "藍色",
            "분홍" => "粉紅色",
            "설정 열기…" => "開啟設定…",
            "이 캐릭터만 끄기" => "只關閉這個角色",
            "전부 종료" => "關閉所有角色並退出",
            "전부 끄기" => "全部關閉",
            "전부 켜기" => "全部開啟",
            "켜짐" => "開啟",
            "꺼짐" => "關閉",
            _ => ko,
        },
    }
}

fn infer_part_icon(part_id: &str, name: &str, overrides: &HashMap<String, String>) -> String {
    if let Some(ic) = overrides.get(part_id) {
        if !ic.is_empty() {
            return ic.clone();
        }
    }
    let combined = format!("{} {}", part_id, name).to_ascii_lowercase();
    if combined.contains("doll") || name.contains("인형") {
        "doll".to_string()
    } else if combined.contains("ahoge") || name.contains("바보털") {
        "ahoge".to_string()
    } else if combined.contains("eardrop") || combined.contains("ear_drop") || name.contains("늘어진") {
        "earDrop".to_string()
    } else if combined.contains("tail") || name.contains("꼬리") {
        "tail".to_string()
    } else if combined.contains("wing") || name.contains("날개") {
        "wing".to_string()
    } else if combined.contains("prop") || combined.contains("object") || name.contains("소품") {
        "prop".to_string()
    } else {
        "ear".to_string()
    }
}

fn fan_angles(center_angle: f64, spread: f64, count: usize) -> Vec<f64> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![center_angle];
    }
    (0..count)
        .map(|i| center_angle - spread * 0.5 + spread * (i as f64) / ((count - 1) as f64))
        .collect()
}

fn polar_points(cx: f64, cy: f64, r: f64, angles: &[f64]) -> Vec<(f64, f64, f64)> {
    angles
        .iter()
        .map(|&a| (cx + r * a.cos(), cy + r * a.sin(), a))
        .collect()
}

fn bounds_overflow(bounds: (f64, f64, f64, f64), pts: &[(f64, f64, f64)]) -> f64 {
    let (l, t, r, b) = bounds;
    pts.iter()
        .map(|&(x, y, _)| (l - x).max(0.0) + (x - r).max(0.0) + (t - y).max(0.0) + (y - b).max(0.0))
        .sum()
}

fn clamp_points(bounds: (f64, f64, f64, f64), pts: &[(f64, f64, f64)]) -> Vec<(f64, f64, f64)> {
    let (l, t, r, b) = bounds;
    pts.iter()
        .map(|&(x, y, a)| (x.clamp(l, r), y.clamp(t, b), a))
        .collect()
}

fn overlap_count(pts: &[(f64, f64, f64)], existing: &[(f64, f64)], button_size: f64) -> usize {
    let min_dist = 0.92 * button_size;
    let mut n = 0;
    for (i, &(ax, ay, _)) in pts.iter().enumerate() {
        for &(bx, by, _) in &pts[i + 1..] {
            if (ax - bx).hypot(ay - by) < min_dist {
                n += 1;
            }
        }
        for &(ex, ey) in existing {
            if (ax - ex).hypot(ay - ey) < min_dist {
                n += 1;
            }
        }
    }
    n
}

fn place_root_arc(
    count: usize,
    cx: f64,
    cy: f64,
    base_r: f64,
    b: f64,
    bounds: (f64, f64, f64, f64),
) -> (f64, Vec<(f64, f64, f64)>) {
    if count == 0 {
        return (base_r, Vec::new());
    }
    let base_angle = -PI / 2.0;
    const MULTS: [f64; 6] = [1.0, 1.15, 1.3, 1.5, 1.75, 2.0];
    const OFFSETS: [f64; 14] = [
        0.0, 0.25, -0.25, 0.5, -0.5, 0.8, -0.8, 1.2, -1.2, 1.6, -1.6, 2.2, -2.2, PI,
    ];
    let mut best: Option<(f64, f64, Vec<(f64, f64, f64)>)> = None;

    for &m in &MULTS {
        let r_try = base_r * m;
        let spread = (1.6 * PI).min(((count.saturating_sub(1)) as f64) * (b + 10.0) / r_try);
        for &off in &OFFSETS {
            let angles = fan_angles(base_angle + off, spread, count);
            let pts = polar_points(cx, cy, r_try, &angles);
            let ov = bounds_overflow(bounds, &pts);
            if ov == 0.0 {
                let rounded = pts
                    .into_iter()
                    .map(|(x, y, a)| (x.round(), y.round(), a))
                    .collect();
                return (r_try, rounded);
            }
            let clamped = clamp_points(bounds, &pts);
            let score = overlap_count(&clamped, &[], b) as f64 + 0.01 * ov;
            if best.as_ref().map_or(true, |b| score < b.0) {
                best = Some((score, r_try, clamped));
            }
        }
    }

    let (_, r_chosen, pts) = best.unwrap_or((0.0, base_r, Vec::new()));
    let rounded = pts
        .into_iter()
        .map(|(x, y, a)| (x.round(), y.round(), a))
        .collect();
    (r_chosen, rounded)
}

fn place_sub_arc(
    count: usize,
    cx: f64,
    cy: f64,
    chosen_r: f64,
    b: f64,
    sub_angle: f64,
    bounds: (f64, f64, f64, f64),
    root_pts: &[(f64, f64)],
) -> Vec<(f64, f64, f64)> {
    if count == 0 {
        return Vec::new();
    }
    let min_r = chosen_r + b + 8.0;
    let cand_r: Vec<f64> = [
        1.75 * chosen_r,
        1.6 * chosen_r,
        1.45 * chosen_r,
        2.0 * chosen_r,
        2.3 * chosen_r,
        min_r,
    ]
    .into_iter()
    .filter(|&r| r >= min_r)
    .collect();
    const OFFSETS: [f64; 14] = [
        0.0, 0.3, -0.3, 0.6, -0.6, 1.0, -1.0, 1.5, -1.5, 2.0, -2.0, 2.5, -2.5, PI,
    ];
    let mut best: Option<(f64, Vec<(f64, f64, f64)>)> = None;

    for &r_try in &cand_r {
        let spread = (1.7 * PI).min(((count.saturating_sub(1)) as f64) * (b + 8.0) / r_try);
        for &off in &OFFSETS {
            let angles = fan_angles(sub_angle + off, spread, count);
            let pts = polar_points(cx, cy, r_try, &angles);
            let ov = bounds_overflow(bounds, &pts);
            let lap = overlap_count(&pts, root_pts, b);
            if ov == 0.0 && lap == 0 {
                return pts
                    .into_iter()
                    .map(|(x, y, a)| (x.round(), y.round(), a))
                    .collect();
            }
            let clamped = clamp_points(bounds, &pts);
            let score = overlap_count(&clamped, root_pts, b) as f64 + 0.01 * ov;
            if best.as_ref().map_or(true, |b| score < b.0) {
                best = Some((score, clamped));
            }
        }
    }

    if best.as_ref().is_some_and(|b| b.0 > 0.0) && count >= 4 {
        let n1 = (count + 1) / 2;
        let n2 = count - n1;
        for &r1 in &[min_r, 1.6 * chosen_r, 1.75 * chosen_r] {
            let r2 = r1 + b + 6.0;
            let s1 = (1.7 * PI).min(((n1.saturating_sub(1)) as f64) * (b + 8.0) / r1);
            let s2 = (1.7 * PI).min(((n2.saturating_sub(1)) as f64) * (b + 8.0) / r2);
            for &off in &OFFSETS {
                let mut pts = polar_points(cx, cy, r1, &fan_angles(sub_angle + off, s1, n1));
                pts.extend(polar_points(cx, cy, r2, &fan_angles(sub_angle + off, s2, n2)));
                let ov = bounds_overflow(bounds, &pts);
                let p_check = if ov > 0.0 {
                    clamp_points(bounds, &pts)
                } else {
                    pts
                };
                let score = overlap_count(&p_check, root_pts, b) as f64 + 0.01 * ov;
                if ov == 0.0 && score == 0.0 {
                    return p_check
                        .into_iter()
                        .map(|(x, y, a)| (x.round(), y.round(), a))
                        .collect();
                }
                if best.as_ref().map_or(true, |b| score < b.0) {
                    best = Some((score, p_check));
                }
            }
        }
    }

    best.map(|(_, pts)| {
        pts.into_iter()
            .map(|(x, y, a)| (x.round(), y.round(), a))
            .collect()
    })
    .unwrap_or_default()
}

struct ItemSpec {
    id: String,
    kind: RadialItemKind,
    label: String,
    prefix: Option<String>,
    icon: String,
    on: Option<bool>,
    radio: bool,
    has_sub: bool,
    swatch: Option<String>,
    closes_on_click: bool,
}

impl RadialMenuState {
    pub fn new(
        always_on_top: bool,
        key_bg: String,
        parts: Vec<(String, String, bool)>,
        lang: String,
        lite_or_reduced: bool,
    ) -> Self {
        Self {
            open: false,
            open_at_s: 0.0,
            closing_at_s: None,
            last_interact_s: 0.0,
            open_sub: None,
            hovered_id: None,
            root_items: Vec::new(),
            sub_items: Vec::new(),
            center_x: 0.0,
            center_y: 0.0,
            button_size: 48.0,
            radius: 115.0,
            base_radius: 115.0,
            bounds: (0.0, 0.0, 0.0, 0.0),
            always_on_top,
            key_bg,
            parts,
            part_icons: HashMap::new(),
            lang,
            lite_or_reduced,
            closing_sub_items: Vec::new(),
            closing_sub_at_s: None,
            sub_opened_at_s: 0.0,
            last_render_hash: 0,
        }
    }

    fn build_root_specs(&self) -> Vec<ItemSpec> {
        let l = &self.lang;
        let mut items = vec![ItemSpec {
            id: "top".to_string(),
            kind: RadialItemKind::Top,
            label: tr(l, "늘 앞으로 고정").to_string(),
            prefix: None,
            icon: "top".to_string(),
            on: Some(self.always_on_top),
            radio: false,
            has_sub: false,
            swatch: None,
            closes_on_click: false,
        }];
        if !self.parts.is_empty() {
            items.push(ItemSpec {
                id: "parts".to_string(),
                kind: RadialItemKind::Parts,
                label: tr(l, "파츠 보이기").to_string(),
                prefix: None,
                icon: "parts".to_string(),
                on: None,
                radio: false,
                has_sub: true,
                swatch: None,
                closes_on_click: false,
            });
        }
        items.push(ItemSpec {
            id: "cam".to_string(),
            kind: RadialItemKind::Cam,
            label: tr(l, "촬영 관련").to_string(),
            prefix: None,
            icon: "cam".to_string(),
            on: None,
            radio: false,
            has_sub: true,
            swatch: None,
            closes_on_click: false,
        });
        items.push(ItemSpec {
            id: "set".to_string(),
            kind: RadialItemKind::Set,
            label: tr(l, "설정 열기…").to_string(),
            prefix: None,
            icon: "set".to_string(),
            on: None,
            radio: false,
            has_sub: false,
            swatch: None,
            closes_on_click: true,
        });
        items.push(ItemSpec {
            id: "off".to_string(),
            kind: RadialItemKind::Off,
            label: tr(l, "이 캐릭터만 끄기").to_string(),
            prefix: None,
            icon: "off".to_string(),
            on: None,
            radio: false,
            has_sub: false,
            swatch: None,
            closes_on_click: true,
        });
        items.push(ItemSpec {
            id: "quit".to_string(),
            kind: RadialItemKind::Quit,
            label: tr(l, "전부 종료").to_string(),
            prefix: None,
            icon: "quit".to_string(),
            on: None,
            radio: false,
            has_sub: false,
            swatch: None,
            closes_on_click: true,
        });
        items
    }

    fn build_sub_specs(&self, sub_id: &str) -> Vec<ItemSpec> {
        let l = &self.lang;
        match sub_id {
            "parts" => {
                let mut list: Vec<ItemSpec> = self
                    .parts
                    .iter()
                    .map(|(pid, name, on)| ItemSpec {
                        id: format!("p:{pid}"),
                        kind: RadialItemKind::PartToggle(pid.clone()),
                        label: name.clone(),
                        prefix: None,
                        icon: infer_part_icon(pid, name, &self.part_icons),
                        on: Some(*on),
                        radio: false,
                        has_sub: false,
                        swatch: None,
                        closes_on_click: false,
                    })
                    .collect();
                if list.len() > 1 {
                    let all_on = list.iter().all(|it| it.on == Some(true));
                    list.push(ItemSpec {
                        id: "all".to_string(),
                        kind: RadialItemKind::PartsAll,
                        label: tr(l, if all_on { "전부 끄기" } else { "전부 켜기" }).to_string(),
                        prefix: None,
                        icon: "all".to_string(),
                        on: None,
                        radio: false,
                        has_sub: false,
                        swatch: None,
                        closes_on_click: false,
                    });
                }
                list
            }
            "cam" => {
                let mut list = vec![ItemSpec {
                    id: "shot".to_string(),
                    kind: RadialItemKind::CamShot,
                    label: tr(l, "촬영하기…").to_string(),
                    prefix: None,
                    icon: "shot".to_string(),
                    on: None,
                    radio: false,
                    has_sub: false,
                    swatch: None,
                    closes_on_click: true,
                }];
                let pre = Some(tr(l, "방송용 배경색 (OBS)").to_string());
                let bg_opts: [(&str, &str, Option<&str>); 4] = [
                    ("", "끄기 (투명)", None),
                    ("#00ff00", "초록", Some("#00ff00")),
                    ("#0000ff", "파랑", Some("#0000ff")),
                    ("#ff00ff", "분홍", Some("#ff00ff")),
                ];
                for (val, label_ko, sw) in bg_opts {
                    list.push(ItemSpec {
                        id: format!("bg{val}"),
                        kind: RadialItemKind::CamKeyBg(val.to_string()),
                        label: tr(l, label_ko).to_string(),
                        prefix: pre.clone(),
                        icon: if val.is_empty() {
                            "obsOff".to_string()
                        } else {
                            "obs".to_string()
                        },
                        on: Some(self.key_bg.eq_ignore_ascii_case(val)),
                        radio: true,
                        has_sub: false,
                        swatch: sw.map(str::to_string),
                        closes_on_click: false,
                    });
                }
                list
            }
            _ => Vec::new(),
        }
    }

    pub fn open_at(
        &mut self,
        center_x: f64,
        center_y: f64,
        size_dip: f64,
        win_w: f64,
        win_h: f64,
        area: Option<(f64, f64, f64, f64)>,
        now_s: f64,
    ) {
        let b = compute_button_size(size_dip);
        let base_r = compute_radius(size_dip, b);
        let bounds = compute_allowed_bounds(win_w, win_h, area, b);
        let specs = self.build_root_specs();
        let (chosen_r, pts) = place_root_arc(specs.len(), center_x, center_y, base_r, b, bounds);

        self.open = true;
        self.open_at_s = now_s;
        self.closing_at_s = None;
        self.last_interact_s = now_s;
        self.open_sub = None;
        self.hovered_id = None;
        self.sub_items.clear();
        self.closing_sub_items.clear();
        self.closing_sub_at_s = None;
        self.center_x = center_x;
        self.center_y = center_y;
        self.button_size = b;
        self.base_radius = base_r;
        self.radius = chosen_r;
        self.bounds = bounds;
        self.root_items = specs
            .into_iter()
            .zip(pts)
            .enumerate()
            .map(|(index, (sp, (x, y, angle)))| RadialItemLayout {
                id: sp.id,
                kind: sp.kind,
                label: sp.label,
                prefix: sp.prefix,
                icon: sp.icon,
                x,
                y,
                angle,
                origin_x: center_x,
                origin_y: center_y,
                on: sp.on,
                radio: sp.radio,
                has_sub: sp.has_sub,
                swatch: sp.swatch,
                closes_on_click: sp.closes_on_click,
                index,
            })
            .collect();
        self.last_render_hash = 0;
    }

    pub fn close(&mut self, now_s: f64) {
        if !self.open {
            return;
        }
        self.open = false;
        self.closing_at_s = Some(now_s);
        self.hovered_id = None;
        if !self.sub_items.is_empty() {
            self.closing_sub_items = std::mem::take(&mut self.sub_items);
            self.closing_sub_at_s = Some(now_s);
        }
        self.open_sub = None;
    }

    pub fn is_visible(&self) -> bool {
        self.open || self.closing_at_s.is_some() || !self.closing_sub_items.is_empty()
    }

    fn open_anim_total_s(&self, count: usize) -> f64 {
        if count == 0 {
            return 0.0;
        }
        if self.lite_or_reduced {
            0.120
        } else {
            0.320 + ((count - 1) as f64) * 0.035
        }
    }

    fn close_anim_total_s(&self, count: usize) -> f64 {
        if count == 0 {
            return 0.0;
        }
        if self.lite_or_reduced {
            0.100
        } else {
            0.176 + ((count - 1) as f64) * 0.0175
        }
    }

    pub fn is_animating(&self, now_s: f64) -> bool {
        if self.open {
            let root_done = now_s >= self.open_at_s + self.open_anim_total_s(self.root_items.len());
            let sub_done = self.sub_items.is_empty()
                || now_s >= self.sub_opened_at_s + self.open_anim_total_s(self.sub_items.len());
            let csub_done = self.closing_sub_at_s.map_or(true, |ct| {
                now_s >= ct + self.close_anim_total_s(self.closing_sub_items.len())
            });
            !(root_done && sub_done && csub_done)
        } else {
            self.closing_at_s.is_some() || !self.closing_sub_items.is_empty()
        }
    }

    pub fn tick(&mut self, now_s: f64) -> bool {
        if self.open && now_s - self.last_interact_s >= 10.0 {
            self.close(now_s);
        }
        if let Some(ct) = self.closing_sub_at_s {
            if now_s >= ct + self.close_anim_total_s(self.closing_sub_items.len()) {
                self.closing_sub_items.clear();
                self.closing_sub_at_s = None;
            }
        }
        if let Some(ct) = self.closing_at_s {
            if now_s >= ct + self.close_anim_total_s(self.root_items.len()) {
                self.closing_at_s = None;
                self.root_items.clear();
                self.sub_items.clear();
                self.closing_sub_items.clear();
                self.closing_sub_at_s = None;
                self.open_sub = None;
                self.hovered_id = None;
            }
        }
        self.is_visible()
    }

    pub fn hit_test(&self, local_x_dip: f64, local_y_dip: f64) -> Option<String> {
        if !self.open {
            return None;
        }
        let r = self.button_size * 0.5;
        for item in self.sub_items.iter().rev() {
            if (local_x_dip - item.x).hypot(local_y_dip - item.y) <= r {
                return Some(item.id.clone());
            }
        }
        for item in self.root_items.iter().rev() {
            if (local_x_dip - item.x).hypot(local_y_dip - item.y) <= r {
                return Some(item.id.clone());
            }
        }
        None
    }

    pub fn on_hover(&mut self, local_x_dip: f64, local_y_dip: f64, now_s: f64) -> bool {
        let hit = self.hit_test(local_x_dip, local_y_dip);
        let changed = hit != self.hovered_id;
        self.hovered_id = hit;
        if self.hovered_id.is_some() {
            self.last_interact_s = now_s;
        }
        changed
    }

    fn open_submenu(&mut self, sub_id: &str, now_s: f64) {
        if !self.sub_items.is_empty() {
            self.closing_sub_items = std::mem::take(&mut self.sub_items);
            self.closing_sub_at_s = Some(now_s);
        }
        let Some(owner) = self.root_items.iter().find(|it| it.id == sub_id).cloned() else {
            return;
        };
        let a = (owner.y - self.center_y).atan2(owner.x - self.center_x);
        let diff = (a + PI * 0.5).sin().atan2((a + PI * 0.5).cos());
        let sub_angle = -PI * 0.5 + 0.2 * diff;

        let specs = self.build_sub_specs(sub_id);
        let root_pts: Vec<(f64, f64)> = self.root_items.iter().map(|it| (it.x, it.y)).collect();
        let pts = place_sub_arc(
            specs.len(),
            self.center_x,
            self.center_y,
            self.radius,
            self.button_size,
            sub_angle,
            self.bounds,
            &root_pts,
        );
        self.open_sub = Some(sub_id.to_string());
        self.sub_opened_at_s = now_s;
        self.sub_items = specs
            .into_iter()
            .zip(pts)
            .enumerate()
            .map(|(index, (sp, (x, y, angle)))| RadialItemLayout {
                id: sp.id,
                kind: sp.kind,
                label: sp.label,
                prefix: sp.prefix,
                icon: sp.icon,
                x,
                y,
                angle,
                origin_x: owner.x,
                origin_y: owner.y,
                on: sp.on,
                radio: sp.radio,
                has_sub: sp.has_sub,
                swatch: sp.swatch,
                closes_on_click: sp.closes_on_click,
                index,
            })
            .collect();
    }

    fn close_submenu(&mut self, now_s: f64) {
        if self.open_sub.is_some() || !self.sub_items.is_empty() {
            if !self.sub_items.is_empty() {
                self.closing_sub_items = std::mem::take(&mut self.sub_items);
                self.closing_sub_at_s = Some(now_s);
            }
            self.open_sub = None;
        }
    }

    pub fn on_click(
        &mut self,
        local_x_dip: f64,
        local_y_dip: f64,
        now_s: f64,
    ) -> Option<RadialAction> {
        if !self.open {
            return None;
        }
        let hit_id = self.hit_test(local_x_dip, local_y_dip)?;
        self.last_interact_s = now_s;

        if let Some(item) = self
            .sub_items
            .iter()
            .chain(self.root_items.iter())
            .find(|it| it.id == hit_id)
            .cloned()
        {
            if item.has_sub {
                if self.open_sub.as_deref() == Some(item.id.as_str()) {
                    self.close_submenu(now_s);
                } else {
                    self.open_submenu(&item.id, now_s);
                }
                return None;
            }
            match item.kind {
                RadialItemKind::Top => {
                    self.always_on_top = !self.always_on_top;
                    if let Some(r) = self.root_items.iter_mut().find(|it| it.kind == RadialItemKind::Top) {
                        r.on = Some(self.always_on_top);
                    }
                    return Some(RadialAction::ToggleTop);
                }
                RadialItemKind::PartToggle(pid) => {
                    let mut next_on = true;
                    for p in &mut self.parts {
                        if p.0 == pid {
                            p.2 = !p.2;
                            next_on = p.2;
                            break;
                        }
                    }
                    if let Some(sub) = self.sub_items.iter_mut().find(|it| it.id == item.id) {
                        sub.on = Some(next_on);
                    }
                    let all_on = self.parts.iter().all(|p| p.2);
                    let lbl = tr(&self.lang, if all_on { "전부 끄기" } else { "전부 켜기" }).to_string();
                    if let Some(all_it) = self.sub_items.iter_mut().find(|it| it.kind == RadialItemKind::PartsAll) {
                        all_it.label = lbl;
                    }
                    return Some(RadialAction::TogglePart(pid));
                }
                RadialItemKind::PartsAll => {
                    let all_currently_on = self.parts.iter().all(|p| p.2);
                    let target_on = !all_currently_on;
                    for p in &mut self.parts {
                        p.2 = target_on;
                    }
                    for sub in &mut self.sub_items {
                        if matches!(sub.kind, RadialItemKind::PartToggle(_)) {
                            sub.on = Some(target_on);
                        } else if sub.kind == RadialItemKind::PartsAll {
                            sub.label =
                                tr(&self.lang, if target_on { "전부 끄기" } else { "전부 켜기" })
                                    .to_string();
                        }
                    }
                    return Some(RadialAction::SetAllParts(target_on));
                }
                RadialItemKind::CamShot => {
                    self.close(now_s);
                    return Some(RadialAction::Shot);
                }
                RadialItemKind::CamKeyBg(val) => {
                    self.key_bg = val.clone();
                    for sub in &mut self.sub_items {
                        if let RadialItemKind::CamKeyBg(ref v) = sub.kind {
                            sub.on = Some(v.eq_ignore_ascii_case(&val));
                        }
                    }
                    return Some(RadialAction::SetKeyBg(val));
                }
                RadialItemKind::Set => {
                    self.close(now_s);
                    return Some(RadialAction::OpenSettings);
                }
                RadialItemKind::Off => {
                    self.close(now_s);
                    return Some(RadialAction::ClosePet);
                }
                RadialItemKind::Quit => {
                    self.close(now_s);
                    return Some(RadialAction::QuitAll);
                }
                RadialItemKind::Parts | RadialItemKind::Cam => {}
            }
        }
        None
    }

    pub fn hot_rect_dip(&self) -> Option<(f64, f64, f64, f64)> {
        if !self.open {
            return None;
        }
        let pad = self.button_size * 0.5 + 8.0;
        let mut l = f64::INFINITY;
        let mut t = f64::INFINITY;
        let mut r = f64::NEG_INFINITY;
        let mut b = f64::NEG_INFINITY;
        for it in self.root_items.iter().chain(self.sub_items.iter()) {
            l = l.min(it.x - pad);
            t = t.min(it.y - pad);
            r = r.max(it.x + pad);
            b = b.max(it.y + pad);
        }
        if l.is_finite() {
            Some((l.round(), t.round(), r.round(), b.round()))
        } else {
            None
        }
    }

    pub fn render_hash(
        &self,
        overlay_w_dip: f64,
        overlay_h_dip: f64,
        origin_x_dip: f64,
        origin_y_dip: f64,
        dpr: f64,
        now_s: f64,
    ) -> u64 {
        let mut h = DefaultHasher::new();
        self.open.hash(&mut h);
        self.closing_at_s.is_some().hash(&mut h);
        self.closing_sub_at_s.is_some().hash(&mut h);
        self.open_sub.hash(&mut h);
        self.hovered_id.hash(&mut h);
        self.always_on_top.hash(&mut h);
        self.key_bg.hash(&mut h);
        self.parts.hash(&mut h);
        self.lang.hash(&mut h);
        self.lite_or_reduced.hash(&mut h);
        overlay_w_dip.to_bits().hash(&mut h);
        overlay_h_dip.to_bits().hash(&mut h);
        origin_x_dip.to_bits().hash(&mut h);
        origin_y_dip.to_bits().hash(&mut h);
        dpr.to_bits().hash(&mut h);
        self.button_size.to_bits().hash(&mut h);
        for it in &self.root_items {
            it.id.hash(&mut h);
            it.x.to_bits().hash(&mut h);
            it.y.to_bits().hash(&mut h);
            it.on.hash(&mut h);
            it.label.hash(&mut h);
        }
        for it in &self.sub_items {
            it.id.hash(&mut h);
            it.x.to_bits().hash(&mut h);
            it.y.to_bits().hash(&mut h);
            it.on.hash(&mut h);
            it.label.hash(&mut h);
        }
        if self.is_animating(now_s) {
            ((now_s * 120.0).round() as i64).hash(&mut h);
        } else {
            0i64.hash(&mut h);
        }
        h.finish()
    }

    pub fn needs_render(
        &self,
        overlay_w_dip: f64,
        overlay_h_dip: f64,
        origin_x_dip: f64,
        origin_y_dip: f64,
        dpr: f64,
        now_s: f64,
    ) -> bool {
        if !self.is_visible() {
            return self.last_render_hash != 0;
        }
        self.render_hash(overlay_w_dip, overlay_h_dip, origin_x_dip, origin_y_dip, dpr, now_s)
            != self.last_render_hash
    }

    pub fn render_to_sprite(
        &mut self,
        overlay_w_dip: f64,
        overlay_h_dip: f64,
        origin_x_dip: f64,
        origin_y_dip: f64,
        dpr: f64,
        now_s: f64,
    ) -> RgbaSprite {
        let w_px = (overlay_w_dip * dpr).round().max(1.0) as u32;
        let h_px = (overlay_h_dip * dpr).round().max(1.0) as u32;
        let hash =
            self.render_hash(overlay_w_dip, overlay_h_dip, origin_x_dip, origin_y_dip, dpr, now_s);
        self.last_render_hash = if self.is_visible() { hash } else { 0 };

        let Some(mut pixmap) = Pixmap::new(w_px, h_px) else {
            return RgbaSprite {
                width: 1,
                height: 1,
                pixels: vec![0; 4],
            };
        };

        if !self.is_visible() {
            return RgbaSprite {
                width: w_px,
                height: h_px,
                pixels: pixmap.take(),
            };
        }

        let root_count = self.root_items.len();
        for it in &self.root_items {
            let (dx, dy, scale, alpha) = if let Some(ct) = self.closing_at_s {
                self.close_anim_state(it, root_count, ct, now_s)
            } else {
                self.open_anim_state(it, self.open_at_s, now_s)
            };
            self.draw_item(
                &mut pixmap,
                it,
                origin_x_dip,
                origin_y_dip,
                dpr,
                dx,
                dy,
                scale,
                alpha,
            );
        }

        let csub_count = self.closing_sub_items.len();
        if let Some(ct) = self.closing_sub_at_s {
            for it in &self.closing_sub_items {
                let (dx, dy, scale, alpha) = self.close_anim_state(it, csub_count, ct, now_s);
                self.draw_item(
                    &mut pixmap,
                    it,
                    origin_x_dip,
                    origin_y_dip,
                    dpr,
                    dx,
                    dy,
                    scale,
                    alpha,
                );
            }
        }

        for it in &self.sub_items {
            let (dx, dy, scale, alpha) = if let Some(ct) = self.closing_at_s {
                self.close_anim_state(it, self.sub_items.len(), ct, now_s)
            } else {
                self.open_anim_state(it, self.sub_opened_at_s, now_s)
            };
            self.draw_item(
                &mut pixmap,
                it,
                origin_x_dip,
                origin_y_dip,
                dpr,
                dx,
                dy,
                scale,
                alpha,
            );
        }

        if self.open {
            if let Some(ref hid) = self.hovered_id {
                if let Some(hovered) = self
                    .sub_items
                    .iter()
                    .chain(self.root_items.iter())
                    .find(|it| &it.id == hid)
                {
                    self.draw_tooltip(
                        &mut pixmap,
                        hovered,
                        overlay_w_dip,
                        overlay_h_dip,
                        origin_x_dip,
                        origin_y_dip,
                        dpr,
                    );
                }
            }
        }

        RgbaSprite {
            width: w_px,
            height: h_px,
            pixels: pixmap.take(),
        }
    }

    fn open_anim_state(
        &self,
        item: &RadialItemLayout,
        start_s: f64,
        now_s: f64,
    ) -> (f64, f64, f64, f64) {
        if self.lite_or_reduced {
            let p = ((now_s - start_s) / 0.120).clamp(0.0, 1.0);
            return (0.0, 0.0, 1.0, p);
        }
        let delay = (item.index as f64) * 0.035;
        let t = ((now_s - start_s - delay) / 0.320).clamp(0.0, 1.0);
        let ease = cubic_bezier_y(0.34, 1.56, 0.64, 1.0, t);
        let dx = (1.0 - ease) * (item.origin_x - item.x);
        let dy = (1.0 - ease) * (item.origin_y - item.y);
        let scale = (0.3 + 0.7 * ease).max(0.01);
        let alpha = t.clamp(0.0, 1.0);
        (dx, dy, scale, alpha)
    }

    fn close_anim_state(
        &self,
        item: &RadialItemLayout,
        total_items: usize,
        close_s: f64,
        now_s: f64,
    ) -> (f64, f64, f64, f64) {
        if self.lite_or_reduced {
            let p = ((now_s - close_s) / 0.100).clamp(0.0, 1.0);
            return (0.0, 0.0, 1.0, 1.0 - p);
        }
        let rev = total_items.saturating_sub(1).saturating_sub(item.index) as f64;
        let delay = rev * 0.0175;
        let t = ((now_s - close_s - delay) / 0.176).clamp(0.0, 1.0);
        let ease = cubic_bezier_y(0.42, 0.0, 1.0, 1.0, t);
        let dx = ease * (item.origin_x - item.x);
        let dy = ease * (item.origin_y - item.y);
        let scale = (1.0 - 0.7 * ease).max(0.01);
        let alpha = (1.0 - t).clamp(0.0, 1.0);
        (dx, dy, scale, alpha)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_item(
        &self,
        pixmap: &mut Pixmap,
        item: &RadialItemLayout,
        origin_x_dip: f64,
        origin_y_dip: f64,
        dpr: f64,
        dx_dip: f64,
        dy_dip: f64,
        scale: f64,
        alpha: f64,
    ) {
        if alpha <= 0.005 {
            return;
        }
        let cx_px = ((item.x + dx_dip - origin_x_dip) * dpr) as f32;
        let cy_px = ((item.y + dy_dip - origin_y_dip) * dpr) as f32;
        let b_px = (self.button_size * scale * dpr) as f32;
        let r_px = b_px * 0.5;
        let a_f = alpha as f32;

        let is_hovered = self.open && self.hovered_id.as_deref() == Some(item.id.as_str());
        let is_sub_open = self.open && item.has_sub && self.open_sub.as_deref() == Some(item.id.as_str());
        let highlighted = is_hovered || is_sub_open;

        // Soft drop shadow: 0 6px 18px rgba(0, 0, 0, .35)
        let sh_cy = cy_px + 6.0 * (scale * dpr) as f32;
        let sh_r = r_px + 12.0 * (scale * dpr) as f32;
        if let Some(sh_path) = circle_path(cx_px, sh_cy, sh_r) {
            if let Some(grad) = RadialGradient::new(
                Point::from_xy(cx_px, sh_cy),
                Point::from_xy(cx_px, sh_cy),
                sh_r,
                vec![
                    GradientStop::new(0.0, Color::from_rgba8(0, 0, 0, (90.0 * a_f) as u8)),
                    GradientStop::new(0.65, Color::from_rgba8(0, 0, 0, (48.0 * a_f) as u8)),
                    GradientStop::new(1.0, Color::from_rgba8(0, 0, 0, 0)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                let mut paint = Paint::default();
                paint.shader = grad;
                paint.anti_alias = true;
                pixmap.fill_path(&sh_path, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }

        // Main circle fill & 1px border
        let bg_rgb = if highlighted { (0x2a, 0x2a, 0x2a) } else { (0x1e, 0x1e, 0x1e) };
        let bd_rgb = if highlighted { (0xd0, 0xd0, 0xd0) } else { (0x2e, 0x2e, 0x2e) };
        if let Some(btn_path) = circle_path(cx_px, cy_px, r_px) {
            let mut fill_p = Paint::default();
            fill_p.set_color_rgba8(bg_rgb.0, bg_rgb.1, bg_rgb.2, (255.0 * a_f) as u8);
            fill_p.anti_alias = true;
            pixmap.fill_path(&btn_path, &fill_p, FillRule::Winding, Transform::identity(), None);

            let mut stroke_p = Paint::default();
            stroke_p.set_color_rgba8(bd_rgb.0, bd_rgb.1, bd_rgb.2, (255.0 * a_f) as u8);
            stroke_p.anti_alias = true;
            let stroke = Stroke {
                width: (scale * dpr) as f32,
                ..Default::default()
            };
            pixmap.stroke_path(&btn_path, &stroke_p, &stroke, Transform::identity(), None);
        }

        // Dashed inner ring for submenu items (inset 3px)
        if item.has_sub {
            let inset_r = (r_px - 3.5 * (scale * dpr) as f32).max(2.0);
            if let Some(ring_path) = circle_path(cx_px, cy_px, inset_r) {
                let mut dash_p = Paint::default();
                let rc = if is_sub_open { 0x8a } else { 0x4a };
                dash_p.set_color_rgba8(rc, rc, rc, (255.0 * a_f) as u8);
                dash_p.anti_alias = true;
                let dash_unit = (2.8 * scale * dpr) as f32;
                let stroke = Stroke {
                    width: (scale * dpr) as f32,
                    dash: StrokeDash::new(vec![dash_unit, dash_unit], 0.0),
                    ..Default::default()
                };
                pixmap.stroke_path(&ring_path, &dash_p, &stroke, Transform::identity(), None);
            }
        }

        // Vector icon centered in button
        let ic_dip = 2.0 * (0.42 * self.button_size * 0.5).round();
        let ic_px = (ic_dip * scale * dpr) as f32;
        let is_off = item.on == Some(false) && !item.radio;
        let fg_rgb = if is_off { (0x70, 0x70, 0x70) } else { (0xec, 0xec, 0xec) };
        let fg_col = Color::from_rgba8(fg_rgb.0, fg_rgb.1, fg_rgb.2, (255.0 * a_f) as u8);
        let bg_col = Color::from_rgba8(bg_rgb.0, bg_rgb.1, bg_rgb.2, (255.0 * a_f) as u8);

        let icon_rot_deg = if item.icon == "top" && is_off { 35.0 } else { 0.0 };
        draw_radial_icon(
            pixmap,
            &item.icon,
            cx_px,
            cy_px,
            ic_px,
            fg_col,
            bg_col,
            icon_rot_deg,
        );

        // KeyBg swatch bar (12x4 rounded pill, bottom: 5px)
        if let Some(ref sw_hex) = item.swatch {
            if let Some((sr, sg, sb)) = parse_hex_rgb(sw_hex) {
                let sw_w = (12.0 * scale * dpr) as f32;
                let sw_h = (4.0 * scale * dpr) as f32;
                let sw_x = cx_px - sw_w * 0.5;
                let sw_y = cy_px + r_px - (5.0 * scale * dpr) as f32 - sw_h;
                if let Some(sw_path) = rounded_rect_path(sw_x, sw_y, sw_w, sw_h, sw_h * 0.5) {
                    let mut p = Paint::default();
                    p.set_color_rgba8(sr, sg, sb, (255.0 * a_f) as u8);
                    p.anti_alias = true;
                    pixmap.fill_path(&sw_path, &p, FillRule::Winding, Transform::identity(), None);
                }
            }
        }

        // #6FD39B 9px active/radio dot at top-right (right: 3px, top: 3px) with 2px #1e1e1e ring
        let show_dot = item.on == Some(true);
        if show_dot {
            let dot_d = (9.0 * scale * dpr) as f32;
            let dot_r = dot_d * 0.5;
            let off_px = (3.0 * scale * dpr) as f32;
            let dot_cx = cx_px + r_px - off_px - dot_r;
            let dot_cy = cy_px - r_px + off_px + dot_r;
            let halo_r = dot_r + (2.0 * scale * dpr) as f32;
            if let Some(hp) = circle_path(dot_cx, dot_cy, halo_r) {
                let mut p = Paint::default();
                p.set_color_rgba8(0x1e, 0x1e, 0x1e, (255.0 * a_f) as u8);
                p.anti_alias = true;
                pixmap.fill_path(&hp, &p, FillRule::Winding, Transform::identity(), None);
            }
            if let Some(dp) = circle_path(dot_cx, dot_cy, dot_r) {
                let mut p = Paint::default();
                p.set_color_rgba8(0x6f, 0xd3, 0x9b, (255.0 * a_f) as u8);
                p.anti_alias = true;
                pixmap.fill_path(&dp, &p, FillRule::Winding, Transform::identity(), None);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_tooltip(
        &self,
        pixmap: &mut Pixmap,
        item: &RadialItemLayout,
        overlay_w_dip: f64,
        overlay_h_dip: f64,
        origin_x_dip: f64,
        origin_y_dip: f64,
        dpr: f64,
    ) {
        let text = self.tooltip_text(item);
        let font_px = (13.0 * dpr) as f32;
        let (text_w_px, line_h_px) = measure_text(&text, font_px);
        let pad_x = (11.0 * dpr) as f32;
        let pad_y = (5.0 * dpr) as f32;
        let pill_w = (text_w_px + 2.0 * pad_x).max((28.0 * dpr) as f32);
        let pill_h = (line_h_px + 2.0 * pad_y).max((24.0 * dpr) as f32);

        let pill_w_dip = pill_w as f64 / dpr;
        let pill_h_dip = pill_h as f64 / dpr;
        let item_x_local = item.x - origin_x_dip;
        let item_y_local = item.y - origin_y_dip;
        let center_y_local = self.center_y - origin_y_dip;
        let below = item_y_local >= center_y_local - 4.0;

        let mut tip_x = item_x_local - pill_w_dip * 0.5;
        let mut tip_y = if below {
            item_y_local + self.button_size * 0.5 + 6.0
        } else {
            item_y_local - self.button_size * 0.5 - 6.0 - pill_h_dip
        };
        let max_x = (overlay_w_dip - pill_w_dip - 4.0).max(4.0);
        let max_y = (overlay_h_dip - pill_h_dip - 4.0).max(4.0);
        tip_x = tip_x.clamp(4.0, max_x).round();
        tip_y = tip_y.clamp(4.0, max_y).round();

        let x_px = (tip_x * dpr) as f32;
        let y_px = (tip_y * dpr) as f32;

        if let Some(pill_path) = rounded_rect_path(x_px, y_px, pill_w, pill_h, pill_h * 0.5) {
            let mut fill_p = Paint::default();
            fill_p.set_color_rgba8(0x0e, 0x0e, 0x0e, 255);
            fill_p.anti_alias = true;
            pixmap.fill_path(&pill_path, &fill_p, FillRule::Winding, Transform::identity(), None);

            let mut stroke_p = Paint::default();
            stroke_p.set_color_rgba8(0x2e, 0x2e, 0x2e, 255);
            stroke_p.anti_alias = true;
            let stroke = Stroke {
                width: dpr as f32,
                ..Default::default()
            };
            pixmap.stroke_path(&pill_path, &stroke_p, &stroke, Transform::identity(), None);
        }

        draw_text_centered_y(pixmap, &text, x_px + pad_x, y_px + pill_h * 0.5, font_px);
    }

    fn tooltip_text(&self, item: &RadialItemLayout) -> String {
        let mut s = item.label.clone();
        if let Some(ref pre) = item.prefix {
            s = format!("{pre} · {s}");
        }
        if let Some(on) = item.on {
            if !item.radio {
                s.push_str(" · ");
                s.push_str(tr(&self.lang, if on { "켜짐" } else { "꺼짐" }));
            }
        }
        s
    }
}

fn measure_text(text: &str, px: f32) -> (f32, f32) {
    let mut w = 0.0f32;
    let mut h = px * 1.15;
    if let Some(pf) = primary_font().or_else(fallback_font) {
        if let Some(lm) = pf.horizontal_line_metrics(px) {
            h = (lm.ascent - lm.descent).max(px);
        }
    }
    for ch in text.chars() {
        if let Some(font) = pick_font_for_char(ch) {
            let m = font.metrics(ch, px);
            w += m.advance_width;
        } else {
            w += px * 0.55;
        }
    }
    (w, h)
}

fn draw_text_centered_y(pixmap: &mut Pixmap, text: &str, start_x: f32, center_y: f32, px: f32) {
    let baseline_y = if let Some(pf) = primary_font().or_else(fallback_font) {
        if let Some(lm) = pf.horizontal_line_metrics(px) {
            center_y + (lm.ascent + lm.descent) * 0.5
        } else {
            center_y + px * 0.32
        }
    } else {
        return;
    };

    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let data = pixmap.data_mut();
    let mut cursor_x = start_x;

    for ch in text.chars() {
        let Some(font) = pick_font_for_char(ch) else {
            cursor_x += px * 0.55;
            continue;
        };
        let (metrics, bitmap) = font.rasterize(ch, px);
        let gx = (cursor_x + metrics.xmin as f32).round() as i32;
        let gy = (baseline_y - metrics.height as f32 - metrics.ymin as f32).round() as i32;

        for row in 0..metrics.height {
            let py = gy + row as i32;
            if py < 0 || py >= ph {
                continue;
            }
            for col in 0..metrics.width {
                let px_x = gx + col as i32;
                if px_x < 0 || px_x >= pw {
                    continue;
                }
                let cov = bitmap[row * metrics.width + col];
                if cov == 0 {
                    continue;
                }
                let a = cov as f32 / 255.0;
                let inv_a = 1.0 - a;
                let src_c = 236.0 * a;
                let idx = ((py * pw + px_x) * 4) as usize;
                data[idx] = (src_c + data[idx] as f32 * inv_a).round().min(255.0) as u8;
                data[idx + 1] = (src_c + data[idx + 1] as f32 * inv_a).round().min(255.0) as u8;
                data[idx + 2] = (src_c + data[idx + 2] as f32 * inv_a).round().min(255.0) as u8;
                data[idx + 3] = (255.0 * a + data[idx + 3] as f32 * inv_a).round().min(255.0) as u8;
            }
        }
        cursor_x += metrics.advance_width;
    }
}

fn cubic_bezier_1d(p1: f64, p2: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
}

fn cubic_bezier_y(x1: f64, y1: f64, x2: f64, y2: f64, target_x: f64) -> f64 {
    let x = target_x.clamp(0.0, 1.0);
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..16 {
        let mid = 0.5 * (lo + hi);
        if cubic_bezier_1d(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    cubic_bezier_1d(y1, y2, 0.5 * (lo + hi))
}

fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

fn circle_path(cx: f32, cy: f32, r: f32) -> Option<Path> {
    if r <= 0.0 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    pb.finish()
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    if r <= 0.01 {
        let mut pb = PathBuilder::new();
        pb.push_rect(Rect::from_xywh(x, y, w, h)?);
        return pb.finish();
    }
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn parse_svg_path(d: &str) -> Option<Path> {
    let mut tokens: Vec<&str> = Vec::new();
    let bytes = d.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() || b == b',' {
            i += 1;
        } else if b.is_ascii_alphabetic() {
            tokens.push(&d[i..i + 1]);
            i += 1;
        } else {
            let start = i;
            if b == b'-' || b == b'+' {
                i += 1;
            }
            let mut seen_dot = false;
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_ascii_digit() {
                    i += 1;
                } else if c == b'.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if i > start {
                tokens.push(&d[start..i]);
            } else {
                i += 1;
            }
        }
    }

    let mut pb = PathBuilder::new();
    let mut idx = 0;
    let mut cmd = b'M';
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;
    let mut sx = 0.0f32;
    let mut sy = 0.0f32;

    let next_f = |tokens: &[&str], idx: &mut usize| -> Option<f32> {
        let t = *tokens.get(*idx)?;
        if t.as_bytes().first().is_some_and(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        *idx += 1;
        t.parse::<f32>().ok()
    };

    while idx < tokens.len() {
        let tok = tokens[idx];
        if tok.len() == 1 && tok.as_bytes()[0].is_ascii_alphabetic() {
            cmd = tok.as_bytes()[0];
            idx += 1;
            if cmd == b'Z' || cmd == b'z' {
                pb.close();
                cx = sx;
                cy = sy;
            }
            continue;
        }
        match cmd {
            b'M' => {
                let (Some(x), Some(y)) = (next_f(&tokens, &mut idx), next_f(&tokens, &mut idx)) else {
                    break;
                };
                pb.move_to(x, y);
                cx = x;
                cy = y;
                sx = x;
                sy = y;
                cmd = b'L';
            }
            b'm' => {
                let (Some(dx), Some(dy)) = (next_f(&tokens, &mut idx), next_f(&tokens, &mut idx)) else {
                    break;
                };
                cx += dx;
                cy += dy;
                pb.move_to(cx, cy);
                sx = cx;
                sy = cy;
                cmd = b'l';
            }
            b'L' => {
                let (Some(x), Some(y)) = (next_f(&tokens, &mut idx), next_f(&tokens, &mut idx)) else {
                    break;
                };
                pb.line_to(x, y);
                cx = x;
                cy = y;
            }
            b'l' => {
                let (Some(dx), Some(dy)) = (next_f(&tokens, &mut idx), next_f(&tokens, &mut idx)) else {
                    break;
                };
                cx += dx;
                cy += dy;
                pb.line_to(cx, cy);
            }
            b'H' => {
                let Some(x) = next_f(&tokens, &mut idx) else { break };
                cx = x;
                pb.line_to(cx, cy);
            }
            b'h' => {
                let Some(dx) = next_f(&tokens, &mut idx) else { break };
                cx += dx;
                pb.line_to(cx, cy);
            }
            b'V' => {
                let Some(y) = next_f(&tokens, &mut idx) else { break };
                cy = y;
                pb.line_to(cx, cy);
            }
            b'v' => {
                let Some(dy) = next_f(&tokens, &mut idx) else { break };
                cy += dy;
                pb.line_to(cx, cy);
            }
            b'C' => {
                let (Some(x1), Some(y1), Some(x2), Some(y2), Some(x), Some(y)) = (
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                ) else {
                    break;
                };
                pb.cubic_to(x1, y1, x2, y2, x, y);
                cx = x;
                cy = y;
            }
            b'c' => {
                let (Some(dx1), Some(dy1), Some(dx2), Some(dy2), Some(dx), Some(dy)) = (
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                ) else {
                    break;
                };
                pb.cubic_to(cx + dx1, cy + dy1, cx + dx2, cy + dy2, cx + dx, cy + dy);
                cx += dx;
                cy += dy;
            }
            b'Q' => {
                let (Some(x1), Some(y1), Some(x), Some(y)) = (
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                ) else {
                    break;
                };
                pb.quad_to(x1, y1, x, y);
                cx = x;
                cy = y;
            }
            b's' | b'S' => {
                let (Some(dx2), Some(dy2), Some(dx), Some(dy)) = (
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                    next_f(&tokens, &mut idx),
                ) else {
                    break;
                };
                let rel = cmd == b's';
                let (x2, y2, x, y) = if rel {
                    (cx + dx2, cy + dy2, cx + dx, cy + dy)
                } else {
                    (dx2, dy2, dx, dy)
                };
                pb.quad_to(x2, y2, x, y);
                cx = x;
                cy = y;
            }
            _ => break,
        }
    }
    pb.finish()
}

fn icon_transform(cx: f32, cy: f32, ic_px: f32, rot_deg: f32, off_y: f32) -> Transform {
    let s = ic_px / 24.0;
    let mut ts = Transform::from_translate(-12.0, -12.0 - off_y);
    ts = ts.post_scale(s, s);
    if rot_deg.abs() > 0.01 {
        ts = ts.post_rotate_at(rot_deg, 0.0, 0.0);
    }
    ts.post_translate(cx, cy)
}

#[allow(clippy::too_many_arguments)]
fn draw_radial_icon(
    pixmap: &mut Pixmap,
    icon: &str,
    cx: f32,
    cy: f32,
    ic_px: f32,
    fg: Color,
    bg: Color,
    rot_deg: f32,
) {
    let mut stroke_paint = Paint::default();
    stroke_paint.set_color(fg);
    stroke_paint.anti_alias = true;

    let stroke = Stroke {
        width: 1.8,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };

    let mut fill_paint = Paint::default();
    fill_paint.set_color(fg);
    fill_paint.anti_alias = true;

    let off_y = if icon == "parts" { 0.6 } else { 0.0 };
    let ts = icon_transform(cx, cy, ic_px, rot_deg, off_y);

    let stroke_d = |pixmap: &mut Pixmap, d: &str| {
        if let Some(p) = parse_svg_path(d) {
            pixmap.stroke_path(&p, &stroke_paint, &stroke, ts, None);
        }
    };
    let fill_d = |pixmap: &mut Pixmap, d: &str| {
        if let Some(p) = parse_svg_path(d) {
            pixmap.fill_path(&p, &fill_paint, FillRule::EvenOdd, ts, None);
        }
    };

    match icon {
        "top" => {
            stroke_d(pixmap, "M7.5 3h9M9.5 3v6.4L5.8 14h12.4L14.5 9.4V3M12 14v7");
        }
        "parts" => {
            if let Some(c) = circle_path(12.0, 14.0, 6.0) {
                pixmap.stroke_path(&c, &stroke_paint, &stroke, ts, None);
            }
            stroke_d(pixmap, "M6.2 12.4 4.2 6.6 9 8.8M17.8 12.4 19.8 6.6 15 8.8");
        }
        "cam" => {
            if let Some(r) = rounded_rect_path(3.0, 7.0, 18.0, 13.0, 3.0) {
                pixmap.stroke_path(&r, &stroke_paint, &stroke, ts, None);
            }
            if let Some(c) = circle_path(12.0, 13.5, 3.5) {
                pixmap.stroke_path(&c, &stroke_paint, &stroke, ts, None);
            }
            stroke_d(pixmap, "M9 7l1.5-3h3L15 7");
        }
        "set" => {
            // Crisp gear icon matching radial.js 'set'
            if let Some(c) = circle_path(12.0, 12.0, 3.0) {
                pixmap.stroke_path(&c, &stroke_paint, &stroke, ts, None);
            }
            if let Some(c) = circle_path(12.0, 12.0, 7.2) {
                pixmap.stroke_path(&c, &stroke_paint, &stroke, ts, None);
            }
            let mut pb = PathBuilder::new();
            for k in 0..8 {
                let ang = (k as f32) * std::f32::consts::FRAC_PI_4;
                let (s, c) = ang.sin_cos();
                pb.move_to(12.0 + 7.2 * c, 12.0 + 7.2 * s);
                pb.line_to(12.0 + 9.8 * c, 12.0 + 9.8 * s);
            }
            if let Some(teeth) = pb.finish() {
                let tooth_stroke = Stroke {
                    width: 2.3,
                    line_cap: LineCap::Round,
                    ..Default::default()
                };
                pixmap.stroke_path(&teeth, &stroke_paint, &tooth_stroke, ts, None);
            }
        }
        "off" => {
            stroke_d(pixmap, "M6 6l12 12M18 6 6 18");
        }
        "quit" => {
            stroke_d(pixmap, "M12 3v9");
            // Open power ring: arc from -42° to +222° around (12, 13) with radius 8
            let mut pb = PathBuilder::new();
            let start_deg = -42.0f32;
            let end_deg = 222.0f32;
            let steps = 28;
            for i in 0..=steps {
                let deg = start_deg + (end_deg - start_deg) * (i as f32 / steps as f32);
                let rad = deg.to_radians();
                let x = 12.0 + 8.0 * rad.cos();
                let y = 13.0 + 8.0 * rad.sin();
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            if let Some(arc) = pb.finish() {
                pixmap.stroke_path(&arc, &stroke_paint, &stroke, ts, None);
            }
        }
        "ear" => {
            stroke_d(pixmap, "M3 19.5Q12 13.5 21 19.5M6.4 16.4 6.9 5l3.7 9M17.6 16.4 17.1 5l-3.7 9");
        }
        "tail" => {
            stroke_d(pixmap, "M8 21C5 15 14 13 11 8C8 3 16 3 18 4");
        }
        "ahoge" => {
            stroke_d(pixmap, "M3 20.5Q12 11.5 21 20.5M12 16Q13.4 10 12.8 3.5");
        }
        "wing" => {
            stroke_d(pixmap, "M11 15 3.2 7.8l1.6 9.2ZM13 15l7.8-7.2-1.6 9.2Z");
        }
        "earDrop" => {
            stroke_d(pixmap, "M3 13.5Q12 7 21 13.5M6.5 9.5Q3.5 13 5 19.5l3-6M17.5 9.5Q20.5 13 19 19.5l-3-6");
        }
        "prop" => {
            if let Some(r) = rounded_rect_path(6.5, 7.0, 11.0, 11.0, 3.0) {
                pixmap.stroke_path(&r, &stroke_paint, &stroke, ts, None);
            }
            stroke_d(pixmap, "M12 2.5v2.2M4 21h16");
        }
        "doll" => {
            if let Some(r) = rounded_rect_path(7.0, 3.5, 10.0, 9.0, 2.6) {
                pixmap.stroke_path(&r, &stroke_paint, &stroke, ts, None);
            }
            stroke_d(pixmap, "M3.5 20.5Q12 14.5 20.5 20.5");
        }
        "all" => {
            for (rx, ry) in [(3.0, 3.0), (14.0, 3.0), (3.0, 14.0), (14.0, 14.0)] {
                if let Some(r) = rounded_rect_path(rx, ry, 7.0, 7.0, 2.0) {
                    pixmap.stroke_path(&r, &stroke_paint, &stroke, ts, None);
                }
            }
        }
        "add" => {
            stroke_d(pixmap, "M12 5v14M5 12h14");
        }
        "shot" => {
            if let Some(c) = circle_path(12.0, 12.0, 8.0) {
                pixmap.stroke_path(&c, &stroke_paint, &stroke, ts, None);
            }
            if let Some(c) = circle_path(12.0, 12.0, 3.0) {
                pixmap.stroke_path(&c, &stroke_paint, &stroke, ts, None);
            }
        }
        "obs" | "obsOff" => {
            // OBS iris symbol: outer ring + 3 rotated blades
            fill_d(
                pixmap,
                "M11.62 22.00C8.43 21.89 5.42 20.19 3.65 17.50C2.12 15.15 1.64 12.31 2.31 9.58C3.53 4.68 8.24 1.47 13.28 2.09C16.15 2.45 18.77 4.09 20.37 6.52C21.90 8.87 22.38 11.71 21.71 14.44C20.89 17.73 18.42 20.41 15.20 21.49C14.03 21.88 12.90 22.04 11.62 22.00ZM13.00 20.15C13.99 20.02 14.83 19.76 15.70 19.33C18.02 18.16 19.65 15.96 20.09 13.40C20.18 12.87 20.19 12.72 20.19 12.01C20.19 11.30 20.18 11.15 20.09 10.62C19.50 7.21 16.81 4.52 13.40 3.93C12.87 3.84 12.72 3.83 12.01 3.83C11.30 3.83 11.15 3.84 10.62 3.93C7.21 4.52 4.52 7.21 3.93 10.62C3.84 11.15 3.83 11.30 3.83 12.01C3.83 12.85 3.88 13.27 4.05 13.97C4.88 17.36 7.75 19.83 11.27 20.18C11.56 20.21 12.71 20.19 13.00 20.15ZM11.33 18.88C11.24 18.87 11.12 18.85 11.06 18.84L10.95 18.83L11.13 18.74C12.00 18.33 12.76 17.68 13.30 16.91L13.38 16.78L13.23 16.48C12.54 15.07 12.73 13.55 13.76 12.27C14.09 11.87 14.63 11.49 15.21 11.26C16.38 10.80 17.75 10.97 18.76 11.72L18.91 11.82L18.89 12.27C18.88 12.95 18.78 13.49 18.56 14.16C18.20 15.24 17.69 16.07 16.88 16.88C15.80 17.96 14.52 18.60 12.99 18.84C12.68 18.89 11.64 18.91 11.33 18.88ZM8.51 17.95C6.70 16.88 5.51 15.11 5.18 12.98C5.14 12.72 5.13 12.62 5.13 12.01C5.13 11.40 5.14 11.30 5.18 11.04C5.35 9.91 5.76 8.88 6.39 8.01C6.55 7.79 6.63 7.70 6.61 7.75C6.58 7.84 6.57 8.56 6.60 8.82C6.63 9.18 6.72 9.64 6.83 9.98C6.91 10.24 6.96 10.35 7.12 10.71L7.17 10.82L7.35 10.82C8.19 10.82 9.08 11.16 9.74 11.72C10.33 12.22 10.85 13.07 11.00 13.78C11.34 15.37 10.61 17.01 9.21 17.82C9.05 17.91 8.74 18.06 8.70 18.06C8.69 18.06 8.61 18.01 8.51 17.95ZM11.80 10.39C11.21 10.34 10.75 10.21 10.26 9.93C9.08 9.26 8.36 8.03 8.36 6.69C8.36 6.18 8.37 6.15 8.48 6.08C9.18 5.65 10.12 5.32 11.04 5.18C11.30 5.14 11.40 5.13 12.01 5.13C12.62 5.13 12.72 5.14 12.98 5.18C14.53 5.42 15.80 6.06 16.88 7.14C17.55 7.81 18.06 8.56 18.37 9.35L18.44 9.50L18.25 9.37C17.47 8.83 16.49 8.48 15.62 8.43L15.43 8.42L15.33 8.58C14.86 9.38 14.07 9.99 13.17 10.25C12.79 10.36 12.21 10.42 11.80 10.39Z",
            );
            if icon == "obsOff" {
                if let Some(cut) = parse_svg_path("M4.5 19.5 19.5 4.5") {
                    let mut cut_paint = Paint::default();
                    cut_paint.set_color(bg);
                    cut_paint.anti_alias = true;
                    let cut_stroke = Stroke {
                        width: 4.6,
                        line_cap: LineCap::Round,
                        ..Default::default()
                    };
                    pixmap.stroke_path(&cut, &cut_paint, &cut_stroke, ts, None);
                    pixmap.stroke_path(&cut, &stroke_paint, &stroke, ts, None);
                }
            }
        }
        _ => {
            if let Some(c) = circle_path(12.0, 12.0, 6.0) {
                pixmap.stroke_path(&c, &stroke_paint, &stroke, ts, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_size_and_radius_formulas_match_js() {
        assert_eq!(compute_button_size(48.0), 40.0);
        assert_eq!(compute_button_size(160.0), 46.0);
        assert_eq!(compute_button_size(190.0), 50.0);
        assert_eq!(compute_button_size(400.0), 56.0);

        assert_eq!(compute_radius(48.0, 40.0), 70.0);
        assert_eq!(compute_radius(160.0, 46.0), 115.0);
        assert_eq!(compute_radius(190.0, 50.0), 128.0);
        assert_eq!(compute_radius(400.0, 56.0), 216.0);
        assert_eq!(compute_radius(500.0, 56.0), 230.0);
    }

    #[test]
    fn arc_placement_is_non_overlapping_for_root_and_sub() {
        let parts = vec![
            ("e1".to_string(), "Ear".to_string(), true),
            ("t1".to_string(), "Tail".to_string(), true),
            ("w1".to_string(), "Wing".to_string(), false),
            ("p1".to_string(), "Prop".to_string(), true),
        ];
        let mut menu = RadialMenuState::new(true, String::new(), parts, "en".to_string(), false);
        menu.open_at(400.0, 400.0, 180.0, 1440.0, 900.0, None, 1.0);

        assert_eq!(menu.root_items.len(), 6);
        let min_d = 0.92 * menu.button_size;
        for (i, a) in menu.root_items.iter().enumerate() {
            for b in &menu.root_items[i + 1..] {
                assert!(
                    (a.x - b.x).hypot(a.y - b.y) >= min_d,
                    "root items overlapped: {} vs {}",
                    a.id,
                    b.id
                );
            }
        }

        // Open parts submenu (4 parts + all = 5 items)
        let parts_btn = menu.root_items.iter().find(|it| it.id == "parts").unwrap().clone();
        assert!(menu.on_click(parts_btn.x, parts_btn.y, 1.2).is_none());
        assert_eq!(menu.open_sub.as_deref(), Some("parts"));
        assert_eq!(menu.sub_items.len(), 5);

        for (i, a) in menu.sub_items.iter().enumerate() {
            for b in &menu.sub_items[i + 1..] {
                assert!(
                    (a.x - b.x).hypot(a.y - b.y) >= min_d,
                    "sub items overlapped: {} vs {}",
                    a.id,
                    b.id
                );
            }
            for r in &menu.root_items {
                assert!(
                    (a.x - r.x).hypot(a.y - r.y) >= min_d,
                    "sub {} overlapped root {}",
                    a.id,
                    r.id
                );
            }
        }
    }

    #[test]
    fn click_actions_idle_close_and_sprite_render() {
        let parts = vec![
            ("ear".to_string(), "귀 1".to_string(), true),
            ("tail".to_string(), "꼬리 1".to_string(), true),
        ];
        let mut menu = RadialMenuState::new(true, String::new(), parts, "en".to_string(), false);
        menu.open_at(250.0, 250.0, 160.0, 800.0, 600.0, None, 10.0);

        let top_pt = menu.root_items.iter().find(|it| it.id == "top").unwrap().clone();
        assert_eq!(
            menu.on_click(top_pt.x, top_pt.y, 10.2),
            Some(RadialAction::ToggleTop)
        );
        assert!(!menu.always_on_top);

        let cam_pt = menu.root_items.iter().find(|it| it.id == "cam").unwrap().clone();
        assert!(menu.on_click(cam_pt.x, cam_pt.y, 10.3).is_none());
        assert_eq!(menu.open_sub.as_deref(), Some("cam"));

        let green_pt = menu
            .sub_items
            .iter()
            .find(|it| it.id == "bg#00ff00")
            .unwrap()
            .clone();
        assert_eq!(
            menu.on_click(green_pt.x, green_pt.y, 10.4),
            Some(RadialAction::SetKeyBg("#00ff00".to_string()))
        );
        assert_eq!(menu.key_bg, "#00ff00");
        assert!(menu.on_hover(green_pt.x, green_pt.y, 10.5));
        assert!(menu.hot_rect_dip().is_some());

        // Render after settle and verify dirty hash stabilizes
        let sprite = menu.render_to_sprite(800.0, 600.0, 0.0, 0.0, 1.0, 15.0);
        assert_eq!(sprite.width, 800);
        assert_eq!(sprite.height, 600);
        let non_zero_alpha = sprite.pixels.chunks_exact(4).any(|px| px[3] > 0);
        assert!(non_zero_alpha);
        assert!(!menu.needs_render(800.0, 600.0, 0.0, 0.0, 1.0, 15.5));

        // 10s idle auto-close triggers close animation, then fully closes
        assert!(menu.tick(20.5));
        assert!(!menu.open);
        assert!(menu.closing_at_s.is_some());
        assert!(!menu.tick(22.0));
        assert!(!menu.is_visible());
    }
}
