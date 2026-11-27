use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AppState {
    pub pets: Vec<PetState>,
    pub next_id: u64,
    pub launch_at_login: bool,
    pub onboarded: bool,
    pub prefs: Prefs,
    pub remind: serde_json::Value,
    pub char_pet: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PetState {
    pub id: String,
    pub char_id: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_pet_size")]
    pub size: f64,
    pub flip: bool,
    pub lock: bool,
    #[serde(default = "default_true")]
    pub always_on_top: bool,
    pub click_through: bool,
    pub mute: bool,
    pub music: bool,
    pub typing: bool,
    pub key_bg: String,
    pub boost: f64,
    pub boost_auto: bool,
    pub anchor: Option<Anchor>,
    pub lift: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Anchor {
    pub display: u64,
    pub fx: f64,
    pub fy: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub react: ReactPrefs,
    pub ui: UiPrefs,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReactPrefs {
    pub sense: f64,
    pub key_every: u32,
    pub beat_offset: f64,
    pub sound: String,
    pub device: String,
    pub device_label: String,
    pub gain: f64,
}

impl Default for ReactPrefs {
    fn default() -> Self {
        Self {
            sense: 5.0,
            key_every: 1,
            beat_offset: 0.0,
            sound: "music".to_string(),
            device: String::new(),
            device_label: String::new(),
            gain: 0.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiPrefs {
    pub bubble_theme: String,
    pub bubble_font: f64,
    pub lang: String,
    pub lite: bool,
    pub menu_style: String,
    pub rest_idle: bool,
    pub rest_min: f64,
    pub reduce_motion: bool,
    pub motion_os: bool,
    pub ghost: bool,
    pub confirm_quit: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            bubble_theme: "dark".to_string(),
            bubble_font: 14.0,
            lang: "en".to_string(),
            lite: false,
            menu_style: "radial".to_string(),
            rest_idle: true,
            rest_min: 5.0,
            reduce_motion: false,
            motion_os: true,
            ghost: false,
            confirm_quit: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CharacterConfig {
    pub name: String,
    pub body: String,
    pub body2: String,
    pub head1: String,
    pub head2: String,
    pub head3: String,
    pub head4: String,
    pub head5: String,
    pub back1: String,
    pub back2: String,
    pub side1: String,
    pub side2: String,
    pub ex1: String,
    pub ex1p: String,
    pub ex2: String,
    pub ex2p: String,
    pub ex3: String,
    pub ex3p: String,
    pub ex4: String,
    pub ex4p: String,
    pub depth: f64,
    pub hold: f64,
    pub squash: f64,
    pub bubble_ms: f64,
    pub volume: f64,
    pub idle_ms: f64,
    pub bubble: bool,
    pub breathe: bool,
    pub breath_ms: f64,
    pub breath_amp: f64,
    pub blink_ms: f64,
    pub sound: bool,
    pub no_repeat: bool,
    pub in_order: bool,
    pub lines: Vec<String>,
    pub idle_lines: Vec<String>,
    pub pat_lines: Vec<String>,
    pub pat_line_gap: f64,
    pub sound_type: String,
    pub sound_pitch: f64,
    pub sound_file: String,
    pub sound_more: Vec<String>,
    pub react_style: String,
    pub react_amp: f64,
    pub protected: bool,
    pub locked: bool,
    pub neck_on: bool,
    pub neck_depth: f64,
    pub neck_color: String,
    pub neck_alpha: f64,
    pub floor_on: bool,
    pub floor_depth: f64,
    pub floor_color: String,
    pub floor_alpha: f64,
    pub pet_on: bool,
    pub drag_on: bool,
    pub drag_swing: f64,
    pub lift_hide: Vec<String>,
    pub lift_still: Vec<String>,
    pub lift_amt: HashMap<String, f64>,
    pub lift_sad: Vec<String>,
    pub lift_aux: serde_json::Value,
    pub lift_only: Vec<String>,
    pub drop_squish: f64,
    pub drop_wobble: f64,
    pub lift_shadow: bool,
    pub lift_shadow_y: f64,
    #[serde(rename = "dragBodyS")]
    pub drag_body_s: f64,
    #[serde(rename = "dragBodyY")]
    pub drag_body_y: f64,
    pub pet_kinds: Vec<String>,
    pub pet_size_mode: String,
    pub pet_size: f64,
    pub pet_fill: String,
    pub pet_line: String,
    pub pet_line_w: f64,
    pub pet_color_mode: String,
    pub pet_kind_colors: HashMap<String, String>,
    pub pet_line_follow: bool,
    pub pet_glow: bool,
    pub pet_glow_size: f64,
    pub pet_amount: f64,
    pub pet_speed: f64,
    pub pet_area: String,
    pub pet_cut: f64,
    pub pet_tilt: f64,
    pub pet_sway: f64,
    pub pet_edge: String,
    pub rig: Vec<RigPartConfig>,
    pub part_order: Vec<String>,
    pub rig_common: RigCommonConfig,
    pub shiver_head: f64,
    pub shiver_body: f64,
    pub beat_motion: serde_json::Value,
    pub key_motion: serde_json::Value,
    pub bubble_style: serde_json::Value,
    pub bubble_styles: Vec<serde_json::Value>,
    pub line_styles: HashMap<String, Vec<String>>,
    pub font_name: String,
    pub font_file: String,
    pub bubble_x: f64,
    pub bubble_y: f64,
    pub bubble_behind: bool,
    pub sound_idle: bool,
    pub head_back: bool,
    pub head_back_sh: bool,
}

impl Default for CharacterConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            body: String::new(),
            body2: String::new(),
            head1: String::new(),
            head2: String::new(),
            head3: String::new(),
            head4: String::new(),
            head5: String::new(),
            back1: String::new(),
            back2: String::new(),
            side1: String::new(),
            side2: String::new(),
            ex1: String::new(),
            ex1p: String::new(),
            ex2: String::new(),
            ex2p: String::new(),
            ex3: String::new(),
            ex3p: String::new(),
            ex4: String::new(),
            ex4p: String::new(),
            depth: 5.0,
            hold: 110.0,
            squash: 0.0,
            bubble_ms: 1500.0,
            volume: 0.45,
            idle_ms: 0.0,
            bubble: true,
            breathe: true,
            breath_ms: 3400.0,
            breath_amp: 12.0,
            blink_ms: 4200.0,
            sound: true,
            no_repeat: true,
            in_order: false,
            lines: Vec::new(),
            idle_lines: Vec::new(),
            pat_lines: Vec::new(),
            pat_line_gap: 8.0,
            sound_type: "click".to_string(),
            sound_pitch: 0.0,
            sound_file: String::new(),
            sound_more: Vec::new(),
            react_style: "nod".to_string(),
            react_amp: 1.0,
            protected: false,
            locked: false,
            neck_on: true,
            neck_depth: 6.0,
            neck_color: "#000000".to_string(),
            neck_alpha: 0.3,
            floor_on: true,
            floor_depth: 6.0,
            floor_color: "#000000".to_string(),
            floor_alpha: 0.34,
            pet_on: false,
            drag_on: true,
            drag_swing: 100.0,
            lift_hide: Vec::new(),
            lift_still: Vec::new(),
            lift_amt: HashMap::new(),
            lift_sad: Vec::new(),
            lift_aux: serde_json::Value::Null,
            lift_only: Vec::new(),
            drop_squish: 100.0,
            drop_wobble: 100.0,
            lift_shadow: true,
            lift_shadow_y: 0.0,
            drag_body_s: 100.0,
            drag_body_y: 0.0,
            pet_kinds: vec!["heart".to_string()],
            pet_size_mode: "random".to_string(),
            pet_size: 26.0,
            pet_fill: "#ffffff".to_string(),
            pet_line: "#ffffff".to_string(),
            pet_line_w: 0.0,
            pet_color_mode: "one".to_string(),
            pet_kind_colors: HashMap::new(),
            pet_line_follow: false,
            pet_glow: true,
            pet_glow_size: 5.0,
            pet_amount: 4.0,
            pet_speed: 1100.0,
            pet_area: "all".to_string(),
            pet_cut: 0.0,
            pet_tilt: 3.0,
            pet_sway: 3.0,
            pet_edge: "glow".to_string(),
            rig: Vec::new(),
            part_order: vec!["body".to_string(), "head".to_string()],
            rig_common: RigCommonConfig::default(),
            shiver_head: 0.0,
            shiver_body: 0.0,
            beat_motion: serde_json::Value::Null,
            key_motion: serde_json::Value::Null,
            bubble_style: serde_json::Value::Null,
            bubble_styles: Vec::new(),
            line_styles: HashMap::new(),
            font_name: String::new(),
            font_file: String::new(),
            bubble_x: 0.0,
            bubble_y: 0.0,
            bubble_behind: false,
            sound_idle: false,
            head_back: false,
            head_back_sh: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RigPartConfig {
    pub id: String,
    pub kind: String,
    pub on: bool,
    pub img: String,
    pub prot: bool,
    pub locked: bool,
    pub s: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RigCommonConfig {
    pub mesh: f64,
    pub joint: f64,
    pub tex: f64,
    pub lag: f64,
}

impl Default for RigCommonConfig {
    fn default() -> Self {
        Self {
            mesh: 11.0,
            joint: 60.0,
            tex: 0.6,
            lag: 0.0,
        }
    }
}

fn default_pet_size() -> f64 {
    160.0
}

fn default_true() -> bool {
    true
}

/// Geometry helper matching BUG Clicker's `main.js:286-291` & `514-542`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct BugGeometry {
    pub size: f64,
    pub side: f64,
    pub bubble_zone: f64,
    pub bottom: f64,
    pub electron_width: f64,
    pub electron_height: f64,
}

impl BugGeometry {
    pub fn from_size(raw_size: f64) -> Self {
        let size = raw_size.round().clamp(48.0, 640.0);
        let side = (0.9 * size).round().max(300.0);
        let bubble_zone = (0.8 * size).round().max(260.0);
        let bottom = 220.0;
        Self {
            size,
            side,
            bubble_zone,
            bottom,
            electron_width: size + 2.0 * side,
            electron_height: size + bubble_zone + bottom,
        }
    }

    /// Computes the core pet box `[center_x, bottom_y]` in Electron virtual desktop DIPs.
    pub fn core_anchor_dip(&self, pet: &PetState) -> (f64, f64) {
        let center_x = pet.x + self.side + self.size * 0.5;
        let bottom_y = (pet.y - pet.lift) + self.bubble_zone + self.size;
        (center_x, bottom_y)
    }

    /// Computes a tight-fit desktop window rect `(left, top, width, height)` in DIPs,
    /// keeping the `size × size` core box centered while leaving `margin_frac * size` padding
    /// around every edge for rigged ears/tails/floor shadow without Electron's giant padding.
    pub fn tight_window_dip(&self, center_x: f64, bottom_y: f64, margin_frac: f64) -> (f64, f64, f64, f64) {
        let win_w = (self.size * (1.0 + 2.0 * margin_frac)).round();
        let win_h = (self.size * (1.0 + 2.0 * margin_frac)).round();
        let core_left = center_x - self.size * 0.5;
        let core_top = bottom_y - self.size;
        let left = (core_left - (win_w - self.size) * 0.5).round();
        let top = (core_top - (win_h - self.size) * 0.5).round();
        (left, top, win_w, win_h)
    }
}

/// Parses `#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA` hex color strings.
pub fn parse_hex_color_rgba(hex: &str) -> Option<[f32; 4]> {
    let s = hex.trim().trim_start_matches('#');
    match s.len() {
        3 | 4 => {
            let expand = |idx: usize| -> Option<f32> {
                let d = u8::from_str_radix(&s[idx..=idx], 16).ok()?;
                Some((d * 17) as f32 / 255.0)
            };
            let a = if s.len() == 4 { expand(3)? } else { 1.0 };
            Some([expand(0)?, expand(1)?, expand(2)?, a])
        }
        6 | 8 => {
            let byte = |idx: usize| -> Option<f32> {
                Some(u8::from_str_radix(&s[idx..idx + 2], 16).ok()? as f32 / 255.0)
            };
            let a = if s.len() == 8 { byte(6)? } else { 1.0 };
            Some([byte(0)?, byte(2)?, byte(4)?, a])
        }
        _ => None,
    }
}

/// Parses `#RRGGBB` or `#RRGGBBAA` hex color strings with fallback.
pub fn parse_hex_color(hex: &str, alpha_override: Option<f64>) -> [f32; 4] {
    let [r, g, b, a] = parse_hex_color_rgba(hex).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let final_a = alpha_override
        .map(|v| (v as f32).clamp(0.0, 1.0))
        .unwrap_or(a);
    [r, g, b, final_a]
}

/// Resolves the active BUG Clicker data directory in priority order:
/// 1. `--data-dir` CLI override
/// 2. `BUG_PET_DATA_DIR` environment variable
/// 3. `%APPDATA%\BUG Clicker\data-location.txt`
/// 4. `%APPDATA%\BUG Clicker`
/// 5. `%LOCALAPPDATA%\BUG Clicker`
/// 6. `%USERPROFILE%\Downloads\BUG Clicker 데이터`
/// 7. `%USERPROFILE%\Downloads\BUG Clicker`
pub fn discover_data_dir(cli_override: Option<&str>) -> Result<PathBuf, String> {
    if let Some(dir) = cli_override {
        let p = PathBuf::from(dir);
        if p.join("state.json").is_file() {
            return Ok(p);
        }
        return Err(format!("--data-dir `{}` does not contain state.json", p.display()));
    }

    if let Ok(env_dir) = std::env::var("BUG_PET_DATA_DIR") {
        let trimmed = env_dir.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            if p.join("state.json").is_file() {
                return Ok(p);
            }
        }
    }

    if let Ok(appdata) = std::env::var("APPDATA") {
        let app_dir = Path::new(&appdata).join("BUG Clicker");
        let ptr = app_dir.join("data-location.txt");
        if let Ok(contents) = fs::read_to_string(&ptr) {
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                let p = PathBuf::from(trimmed);
                if p.join("state.json").is_file() {
                    return Ok(p);
                }
            }
        }
        if app_dir.join("state.json").is_file() {
            return Ok(app_dir);
        }
    }

    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = Path::new(&local).join("BUG Clicker");
        if p.join("state.json").is_file() {
            return Ok(p);
        }
    }

    if let Ok(profile) = std::env::var("USERPROFILE") {
        let downloads = Path::new(&profile).join("Downloads");
        for folder in ["BUG Clicker 데이터", "BUG Clicker"] {
            let p = downloads.join(folder);
            if p.join("state.json").is_file() {
                return Ok(p);
            }
        }
    }

    Err("Could not locate BUG Clicker data directory containing state.json (pass --data-dir or set BUG_PET_DATA_DIR)".to_string())
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn load_state(data_dir: &Path) -> Result<AppState, String> {
    let path = data_dir.join("state.json");
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed reading {}: {e}", path.display()))?;
    serde_json::from_str::<AppState>(&raw)
        .map_err(|e| format!("Failed parsing {}: {e}", path.display()))
}

pub fn load_character_config(char_dir: &Path) -> Result<CharacterConfig, String> {
    let path = char_dir.join("character.json");
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed reading {}: {e}", path.display()))?;
    serde_json::from_str::<CharacterConfig>(&raw)
        .map_err(|e| format!("Failed parsing {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_and_anchor_match_electron_state_json() {
        // Sample coordinates on a 1440x900 primary DIP display
        let alpha = PetState {
            id: "pet1".to_string(),
            char_id: "AlphaPet".to_string(),
            x: 951.0,
            y: 463.0,
            size: 190.0,
            lift: 0.0,
            ..Default::default()
        };
        let ga = BugGeometry::from_size(alpha.size);
        let (acx, aby) = ga.core_anchor_dip(&alpha);
        assert!((acx - 1346.0).abs() < 1e-6);
        assert!((aby - 913.0).abs() < 1e-6);
        assert!((acx / 1440.0 - 0.9347222222222222).abs() < 1e-9);
        assert!((aby / 900.0 - 1.0144444444444445).abs() < 1e-9);

        let beta = PetState {
            id: "pet2".to_string(),
            char_id: "BetaPet".to_string(),
            x: -212.0,
            y: 458.0,
            size: 180.0,
            lift: 0.0,
            ..Default::default()
        };
        let gb = BugGeometry::from_size(beta.size);
        let (bcx, bby) = gb.core_anchor_dip(&beta);
        assert!((bcx - 178.0).abs() < 1e-6);
        assert!((bby - 898.0).abs() < 1e-6);
        assert!((bcx / 1440.0 - 0.12361111111111112).abs() < 1e-9);
        assert!((bby / 900.0 - 0.9977777777777778).abs() < 1e-9);

        let (wl, wt, ww, wh) = ga.tight_window_dip(acx, aby, 0.65);
        assert_eq!(ww, 437.0);
        assert_eq!(wh, 437.0);
        assert!((wl + ww * 0.5 - acx).abs() <= 1.0);
        assert!((wt + (wh - ga.size) * 0.5 + ga.size - aby).abs() <= 1.0);
    }

    #[test]
    fn loads_local_data_dir_if_present() {
        if let Ok(dir) = discover_data_dir(None) {
            let state = load_state(&dir).expect("state.json must parse");
            assert!(!state.pets.is_empty());
            for pet in &state.pets {
                let cdir = dir.join("characters").join(&pet.char_id);
                if cdir.exists() {
                    let cfg = load_character_config(&cdir).expect("character.json must parse");
                    assert!(!cfg.name.is_empty());
                }
            }
        }
    }
}
