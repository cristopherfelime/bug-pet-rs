use crate::config::{RigCommonConfig, RigPartConfig};
use crate::rig::mesh::{PropMeta, RigMeshMeta};
use serde_json::Value;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

pub const Z_DT: f64 = 1.0 / 240.0;
const DEG2RAD: f64 = PI / 180.0;

#[inline]
fn lerp(a: f64, b: f64, f: f64) -> f64 {
    a + (b - a) * f
}

#[inline]
fn y_ease(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

#[inline]
fn ss5(x: f64) -> f64 {
    x * x * x * (x * (6.0 * x - 15.0) + 10.0)
}

#[inline]
fn nt(x: f64, m: f64) -> f64 {
    m * (x / m).tanh()
}

#[inline]
fn rt(x: f64, m: f64) -> f64 {
    x / (1.0 + (x / m) * (x / m)).sqrt()
}

#[inline]
fn it_soft(x: f64, m: f64) -> f64 {
    let n = x.abs() / m;
    if n < 0.9 {
        x
    } else {
        x.signum() * m * (0.9 + 0.1 * ((n - 0.9) / 0.1).tanh())
    }
}

#[inline]
fn wrap_pi(a: f64) -> f64 {
    a.sin().atan2(a.cos())
}

#[derive(Clone, Debug)]
pub struct EarParams {
    pub off: [[f64; 2]; 2],
    pub mirror: bool,
    pub tilt: f64,
    pub tilt_l: Option<f64>,
    pub bones: Vec<f64>,
    pub range: f64,
    pub amp: f64,
    pub period: f64,
    pub soft: f64,
    pub bounce: f64,
    pub kick: f64,
    pub wobble: f64,
    pub opp: bool,
    pub scale: f64,
    pub fade: f64,
    pub when: String,
    pub on_drag: bool,
    pub on_beat: bool,
    pub beat_sway: bool,
    pub on_key: bool,
    pub on_pet: String,
    pub pet_amt: f64,
    pub stiff: f64,
    pub drop: bool,
}

#[derive(Clone, Debug)]
pub struct TailParams {
    pub root: [f64; 2],
    pub dir: f64,
    pub curve: f64,
    pub bones: Vec<f64>,
    pub range: f64,
    pub amp: f64,
    pub period: f64,
    pub soft: f64,
    pub bounce: f64,
    pub kick: f64,
    pub wobble: f64,
    pub scale: f64,
    pub fade: f64,
    pub flip: bool,
    pub fur: String,
    pub when: String,
    pub on_beat: bool,
    pub head: bool,
    pub rel: Option<[f64; 2]>,
    pub rev: bool,
    pub on_pet: String,
    pub pet_amt: f64,
}

#[derive(Clone, Debug)]
pub struct WingParams {
    #[allow(dead_code)]
    pub style: String,
    pub x: f64,
    pub y: f64,
    pub tilt: f64,
    pub x_l: Option<f64>,
    pub y_l: Option<f64>,
    pub tilt_l: Option<f64>,
    pub sym: bool,
    pub bones: Vec<f64>,
    pub range: f64,
    pub amp: f64,
    pub period: f64,
    pub down: f64,
    pub soft: f64,
    pub bounce: f64,
    pub depth: f64,
    pub flaps: u32,
    pub flap_t: f64,
    pub scale: f64,
    pub fade: f64,
    pub when: String,
}

#[derive(Clone, Debug)]
pub struct PropParams {
    pub anim: String,
    pub pos: [f64; 2],
    pub scale: f64,
    pub when: String,
    pub follow: String,
    pub hop_h: f64,
    pub hop_every: f64,
    pub hop_squash: f64,
    pub fl_rx: f64,
    pub fl_ry: f64,
    pub fl_period: f64,
    pub sh_amp: f64,
    pub sh_deg: f64,
    pub sh_dur: f64,
    pub sh_every: f64,
    pub rl_x1: f64,
    pub rl_x2: f64,
    pub rl_speed: f64,
    pub rl_pause: f64,
    pub pk_dx: f64,
    pub pk_dy: f64,
    pub pk_wait: f64,
    pub pk_stay: f64,
}

#[derive(Clone, Debug)]
pub enum PartParams {
    Ear(EarParams),
    Tail(TailParams),
    Wing(WingParams),
    Prop(PropParams),
}

fn jnum(v: &Value, key: &str, def: f64, lo: f64, hi: f64) -> f64 {
    match v.get(key).and_then(|x| x.as_f64()) {
        Some(n) if n.is_finite() => n.clamp(lo, hi),
        _ => def,
    }
}

fn jbool(v: &Value, key: &str, def: bool) -> bool {
    v.get(key).and_then(|x| x.as_bool()).unwrap_or(def)
}

fn jstr(v: &Value, key: &str, def: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| def.to_string())
}

fn jvec2(v: &Value, key: &str, def: [f64; 2], lo: f64, hi: f64) -> [f64; 2] {
    if let Some(arr) = v.get(key).and_then(|x| x.as_array()) {
        if arr.len() == 2 {
            if let (Some(x), Some(y)) = (arr[0].as_f64(), arr[1].as_f64()) {
                if x.is_finite() && y.is_finite() {
                    return [x.round().clamp(lo, hi), y.round().clamp(lo, hi)];
                }
            }
        }
    }
    def
}

fn jbones(v: &Value, def_count: usize) -> Vec<f64> {
    if let Some(arr) = v.get("bones").and_then(|x| x.as_array()) {
        if !arr.is_empty() && arr.len() <= 12 {
            let vals: Vec<f64> = arr
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0).max(0.01))
                .collect();
            let sum: f64 = vals.iter().sum();
            if sum > 0.0 {
                return vals.into_iter().map(|x| x / sum).collect();
            }
        }
    }
    vec![1.0 / def_count as f64; def_count]
}

impl PartParams {
    pub fn from_config(part: &RigPartConfig) -> Self {
        let s = &part.s;
        let when = {
            let w = jstr(s, "when", "down");
            if w == "up" { w } else { "down".to_string() }
        };
        match part.kind.as_str() {
            "ear" => {
                let mut off = [[0.0f64; 2]; 2];
                if let Some(arr) = s.get("off").and_then(|x| x.as_array()) {
                    if arr.len() == 2 {
                        for i in 0..2 {
                            if let Some(pair) = arr[i].as_array() {
                                if pair.len() == 2 {
                                    off[i][0] = pair[0].as_f64().unwrap_or(0.0).clamp(-400.0, 1200.0);
                                    off[i][1] = pair[1].as_f64().unwrap_or(0.0).clamp(-400.0, 1200.0);
                                }
                            }
                        }
                    }
                }
                let on_pet = match jstr(s, "onPet", "none").as_str() {
                    "droop" => "droop",
                    "bounce" => "bounce",
                    _ => "none",
                }
                .to_string();
                PartParams::Ear(EarParams {
                    off,
                    mirror: jbool(s, "mirror", true),
                    tilt: jnum(s, "tilt", 0.0, -180.0, 180.0),
                    tilt_l: s.get("tiltL").and_then(|x| x.as_f64()).map(|n| n.clamp(-180.0, 180.0)),
                    bones: jbones(s, 3),
                    range: jnum(s, "range", 35.0, 5.0, 120.0),
                    amp: jnum(s, "amp", 25.0, 0.0, 100.0),
                    period: jnum(s, "period", 1.2, 0.4, 5.0),
                    soft: jnum(s, "soft", 40.0, 0.0, 100.0),
                    bounce: jnum(s, "bounce", 89.0, 0.0, 100.0),
                    kick: jnum(s, "kick", 94.0, 0.0, 100.0),
                    wobble: jnum(s, "wobble", 49.0, 0.0, 100.0),
                    opp: jbool(s, "opp", true),
                    scale: jnum(s, "scale", 100.0, 40.0, 200.0),
                    fade: jnum(s, "fade", 0.0, 0.0, 60.0),
                    when,
                    on_drag: jbool(s, "onDrag", true),
                    on_beat: jbool(s, "onBeat", true),
                    beat_sway: jbool(s, "beatSway", false),
                    on_key: jbool(s, "onKey", true),
                    on_pet,
                    pet_amt: jnum(s, "petAmt", 50.0, 0.0, 100.0),
                    stiff: jnum(s, "stiff", 0.0, 0.0, 100.0),
                    drop: jbool(s, "drop", false),
                })
            }
            "wing" => PartParams::Wing(WingParams {
                style: "bat".to_string(),
                x: jnum(s, "x", 0.0, -300.0, 300.0),
                y: jnum(s, "y", -27.0, -400.0, 400.0),
                tilt: jnum(s, "tilt", -19.0, -180.0, 180.0),
                x_l: s.get("xL").and_then(|v| v.as_f64()).map(|n| n.clamp(-300.0, 300.0)),
                y_l: s.get("yL").and_then(|v| v.as_f64()).map(|n| n.clamp(-400.0, 400.0)),
                tilt_l: s.get("tiltL").and_then(|v| v.as_f64()).map(|n| n.clamp(-180.0, 180.0)),
                sym: jbool(s, "sym", true),
                bones: jbones(s, 6),
                range: jnum(s, "range", 30.0, 5.0, 120.0),
                amp: jnum(s, "amp", 64.0, 0.0, 100.0),
                period: jnum(s, "period", 1.4, 0.4, 5.0),
                down: jnum(s, "down", 50.0, 15.0, 85.0),
                soft: jnum(s, "soft", 41.0, 0.0, 100.0),
                bounce: jnum(s, "bounce", 79.0, 0.0, 100.0),
                depth: jnum(s, "depth", 76.0, 10.0, 100.0),
                flaps: jnum(s, "flaps", 1.0, 1.0, 3.0).round() as u32,
                flap_t: jnum(s, "flapT", 0.45, 0.2, 1.2),
                scale: jnum(s, "scale", 100.0, 40.0, 200.0),
                fade: jnum(s, "fade", 0.0, 0.0, 60.0),
                when,
            }),
            "prop" => {
                let anim = match jstr(s, "anim", "hop").as_str() {
                    "float" => "float",
                    "shake" => "shake",
                    "roll" => "roll",
                    "peek" => "peek",
                    "still" => "still",
                    _ => "hop",
                }
                .to_string();
                let follow = match jstr(s, "follow", "none").as_str() {
                    "head" => "head",
                    "noPet" => "noPet",
                    _ => "none",
                }
                .to_string();
                PartParams::Prop(PropParams {
                    anim,
                    pos: jvec2(s, "pos", [0.0, 0.0], -600.0, 600.0),
                    scale: jnum(s, "scale", 100.0, 30.0, 250.0),
                    when,
                    follow,
                    hop_h: jnum(s, "hopH", 60.0, 5.0, 300.0),
                    hop_every: jnum(s, "hopEvery", 1.6, 0.0, 6.0),
                    hop_squash: jnum(s, "hopSquash", 30.0, 0.0, 60.0),
                    fl_rx: jnum(s, "flRx", 40.0, 0.0, 300.0),
                    fl_ry: jnum(s, "flRy", 24.0, 0.0, 300.0),
                    fl_period: jnum(s, "flPeriod", 4.0, 1.0, 12.0),
                    sh_amp: jnum(s, "shAmp", 8.0, 0.0, 60.0),
                    sh_deg: jnum(s, "shDeg", 6.0, 0.0, 30.0),
                    sh_dur: jnum(s, "shDur", 0.6, 0.2, 2.0),
                    sh_every: jnum(s, "shEvery", 3.0, 0.0, 10.0),
                    rl_x1: jnum(s, "rlX1", -120.0, -400.0, -10.0),
                    rl_x2: jnum(s, "rlX2", 120.0, 10.0, 400.0),
                    rl_speed: jnum(s, "rlSpeed", 150.0, 20.0, 500.0),
                    rl_pause: jnum(s, "rlPause", 0.8, 0.0, 4.0),
                    pk_dx: jnum(s, "pkDx", 90.0, -400.0, 400.0),
                    pk_dy: jnum(s, "pkDy", -70.0, -400.0, 400.0),
                    pk_wait: jnum(s, "pkWait", 2.2, 0.3, 10.0),
                    pk_stay: jnum(s, "pkStay", 1.2, 0.2, 6.0),
                })
            }
            _ => {
                let head = jbool(s, "head", false);
                let pin_at = s.get("pinAt").and_then(|v| v.as_array()).and_then(|a| {
                    if a.len() == 2 {
                        Some([a[0].as_f64()?, a[1].as_f64()?])
                    } else {
                        None
                    }
                });
                let root = jvec2(s, "root", [400.0, 650.0], -400.0, 1200.0);
                let rel = if head {
                    s.get("rel")
                        .and_then(|v| v.as_array())
                        .and_then(|a| {
                            if a.len() == 2 {
                                Some([a[0].as_f64()?, a[1].as_f64()?])
                            } else {
                                None
                            }
                        })
                        .or_else(|| pin_at.map(|p| [root[0] - p[0], root[1] - p[1]]))
                } else {
                    None
                };
                let fur = if jstr(s, "fur", "L") == "R" { "R" } else { "L" }.to_string();
                let on_pet = if jstr(s, "onPet", "none") == "wag" { "wag" } else { "none" }.to_string();
                PartParams::Tail(TailParams {
                    root,
                    dir: jnum(s, "dir", 8.0, -180.0, 180.0),
                    curve: jnum(s, "curve", -14.0, -90.0, 90.0),
                    bones: jbones(s, 12),
                    range: jnum(s, "range", 90.0, 5.0, 120.0),
                    amp: jnum(s, "amp", 90.0, 0.0, 100.0),
                    period: jnum(s, "period", 2.7, 0.4, 5.0),
                    soft: jnum(s, "soft", 88.0, 0.0, 100.0),
                    bounce: jnum(s, "bounce", 69.0, 0.0, 100.0),
                    kick: jnum(s, "kick", 67.0, 0.0, 100.0),
                    wobble: jnum(s, "wobble", 55.0, 0.0, 100.0),
                    scale: jnum(s, "scale", 100.0, 40.0, 200.0),
                    fade: jnum(s, "fade", 0.0, 0.0, 60.0),
                    flip: jbool(s, "flip", false),
                    fur,
                    when,
                    on_beat: jbool(s, "onBeat", true),
                    head,
                    rel,
                    rev: jbool(s, "rev", false),
                    on_pet,
                    pet_amt: jnum(s, "petAmt", 50.0, 0.0, 100.0),
                })
            }
        }
    }

    pub fn when(&self) -> &str {
        match self {
            PartParams::Ear(p) => &p.when,
            PartParams::Tail(p) => &p.when,
            PartParams::Wing(p) => &p.when,
            PartParams::Prop(p) => &p.when,
        }
    }

    pub fn fade_frac(&self) -> f32 {
        match self {
            PartParams::Ear(p) => (p.fade / 100.0) as f32,
            PartParams::Tail(p) => (p.fade / 100.0) as f32,
            PartParams::Wing(p) => (p.fade / 100.0) as f32,
            PartParams::Prop(_) => 0.0,
        }
    }

    pub fn scale_frac(&self) -> f64 {
        match self {
            PartParams::Ear(p) => p.scale / 100.0,
            PartParams::Tail(p) => p.scale / 100.0,
            PartParams::Wing(p) => p.scale / 100.0,
            PartParams::Prop(p) => p.scale / 100.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    pub a: f64,
    pub v: f64,
}

#[inline]
fn step_spring(s: &mut Spring, tgt: f64, k: f64, c: f64, dt: f64) {
    s.v += (k * (tgt - s.a) - c * s.v) * dt;
    s.a += s.v * dt;
}

struct SpringConsts {
    k: f64,
    c: f64,
    tk: f64,
    tc: f64,
    follow: f64,
    bend: f64,
}

#[inline]
fn spring_w(bounce: f64, soft: f64) -> SpringConsts {
    let b = bounce / 100.0;
    let s = soft / 100.0;
    let k = lerp(260.0, 50.0, b);
    let c = 2.0 * lerp(0.75, 0.2, b) * k.sqrt();
    let tk = k * lerp(0.9, 0.2, s);
    let tc = 2.0 * lerp(0.75, 0.25, s) * tk.sqrt();
    SpringConsts {
        k,
        c,
        tk,
        tc,
        follow: lerp(1.0, 1.6, s),
        bend: lerp(0.8, 1.8, s),
    }
}

#[inline]
fn spring_g(w: f64, k0: f64, k1: f64, c0: f64, c1: f64) -> (f64, f64) {
    let k = lerp(k0, k1, w);
    (k, 2.0 * lerp(c0, c1, w) * k.sqrt())
}

#[derive(Clone, Debug)]
pub struct PartPhys {
    pub press: f64,
    pub kick: f64,
    pub ears: [Spring; 2],
    pub ear_tip: [Spring; 2],
    pub ew: [Spring; 2],
    pub ew_t: [Spring; 2],
    pub tail_base: Spring,
    pub tail_tip: Spring,
    pub tw: Spring,
    pub tw_t: Spring,
    pub wing: [Spring; 2],
    pub wing_tip: [Spring; 2],
    pub w_press: f64,
    pub w_t0: f64,
    pub w_sync: Option<f64>,
    pub w_from: f64,
    pub w_g: Option<f64>,
    pub h_t0: Option<f64>,
    pub pk: f64,
    pub lx: f64,
    pub ly: f64,
    pub bx: f64,
    pub bsg: f64,
    pub bt: f64,
    pub b_a: f64,
    pub dip: f64,
    pub lxs: f64,
    pub lys: f64,
    pub kt: f64,
    pub kw: f64,
    pub pet_at: f64,
    pub pl: f64,
    pub lf: f64,
    pub lf_t: f64,
    pub body_a: f64,
    pub ld: f64,
    pub lsp: f64,
    pub la: f64,
    pub lsad: bool,
    pub eh: [Spring; 2],
    pub hg: Spring,
    pub hg_t: Spring,
    pub tr: Spring,
    pub tvx: f64,
    pub wph: Option<f64>,
    pub fs_keep: Option<f64>,
    pub pk_dir: Option<f64>,
    // Props
    pub t_hop: Option<f64>,
    pub t_shake: Option<f64>,
    pub t_bump: Option<f64>,
    pub pk0: Option<f64>,
}

impl Default for PartPhys {
    fn default() -> Self {
        Self {
            press: -9.0,
            kick: 0.0,
            ears: [Spring::default(); 2],
            ear_tip: [Spring::default(); 2],
            ew: [Spring::default(); 2],
            ew_t: [Spring::default(); 2],
            tail_base: Spring::default(),
            tail_tip: Spring::default(),
            tw: Spring::default(),
            tw_t: Spring::default(),
            wing: [Spring::default(); 2],
            wing_tip: [Spring::default(); 2],
            w_press: -9.0,
            w_t0: 0.0,
            w_sync: None,
            w_from: 0.0,
            w_g: None,
            h_t0: None,
            pk: 1.0,
            lx: 0.0,
            ly: 0.0,
            bx: 0.0,
            bsg: 1.0,
            bt: -9.0,
            b_a: 0.0,
            dip: 0.0,
            lxs: 0.0,
            lys: 0.0,
            kt: -9.0,
            kw: 0.0,
            pet_at: -9.0,
            pl: 0.0,
            lf: 0.0,
            lf_t: 0.0,
            body_a: 0.0,
            ld: 1.0,
            lsp: 0.5,
            la: 1.0,
            lsad: false,
            eh: [Spring::default(); 2],
            hg: Spring::default(),
            hg_t: Spring::default(),
            tr: Spring::default(),
            tvx: 0.0,
            wph: None,
            fs_keep: None,
            pk_dir: None,
            t_hop: None,
            t_shake: None,
            t_bump: None,
            pk0: None,
        }
    }
}

#[inline]
fn decay_q(p: &mut PartPhys) {
    let dec = |v: &mut f64, rate: f64| {
        if *v != 0.0 {
            if v.abs() < 1e-4 {
                *v = 0.0;
            } else {
                *v *= (-rate * Z_DT).exp();
            }
        }
    };
    dec(&mut p.lx, 5.0);
    dec(&mut p.ly, 5.0);
    dec(&mut p.bx, 3.0);
    dec(&mut p.tvx, 4.0);
}

#[inline]
fn press_env(p: &PartPhys, t: f64) -> f64 {
    let n = t - p.press;
    if n < 0.0 || n > 0.34 {
        0.0
    } else if n < 0.1 {
        y_ease(n / 0.1)
    } else if n < 0.16 {
        1.0
    } else {
        1.0 - y_ease((n - 0.16) / 0.18)
    }
}

pub fn step_part_physics(params: &PartParams, p: &mut PartPhys, a: f64) {
    decay_q(p);
    p.lf += (p.lf_t - p.lf) * (1.0 - (-Z_DT / 0.12).exp());

    match params {
        PartParams::Ear(o) => {
            let w = spring_w(o.bounce, o.soft);
            let e_ang = (TAU * a / o.period).sin() * o.range * DEG2RAD * (o.amp / 100.0);
            for i in 0..2 {
                let tgt = (if o.opp && i == 1 { -1.0 } else { 1.0 }) * e_ang + p.bx;
                step_spring(&mut p.ears[i], tgt, w.k, w.c, Z_DT);
                step_spring(&mut p.ear_tip[i], p.ears[i].a * w.follow, w.tk, w.tc, Z_DT);
            }
            let wob = o.wobble / 100.0;
            let (kw, cw) = if o.drop {
                spring_g(wob, 130.0, 70.0, 0.42, 0.2)
            } else {
                spring_g(wob, 320.0, 150.0, 0.6, 0.2)
            };
            let (kt, ct) = if o.drop {
                spring_g(wob, 95.0, 50.0, 0.45, 0.22)
            } else {
                spring_g(wob, 240.0, 110.0, 0.6, 0.22)
            };
            let d = press_env(p, a) * (o.kick / 100.0) * (if o.drop { 0.3 } else { 0.75 }) * p.pk;
            let h = a - p.bt;
            p.dip = if (0.0..0.32).contains(&h) {
                p.b_a * if h < 0.06 {
                    y_ease(h / 0.06)
                } else {
                    1.0 - y_ease((h - 0.06) / 0.26)
                }
            } else {
                0.0
            };
            let u = 1.0 - (-Z_DT / 0.14).exp();
            p.lxs += (p.lx - p.lxs) * u;
            p.lys += (p.ly - p.lys) * u;
            let m = a - p.kt;
            p.kw = if (0.0..0.7).contains(&m) {
                0.11 * (-6.0 * m).exp() * (TAU * m / 0.34).sin()
            } else {
                0.0
            };
            let g = (a - p.pet_at) < 0.35;
            p.pl += ((if g { 1.0 } else { 0.0 }) - p.pl)
                * (1.0 - (-Z_DT / if g { 0.18 } else { 0.4 }).exp());
            let b = match o.on_pet.as_str() {
                "droop" => 0.55 * p.pl * (o.pet_amt / 100.0),
                "bounce" => 0.16 * p.pl * (o.pet_amt / 100.0) * (0.5 + 0.5 * (TAU * a / 0.36).sin()),
                _ => 0.0,
            };
            for i in 0..2 {
                let tgt = (if i == 1 { 1.0 } else { -1.0 }) * (d + b);
                step_spring(&mut p.ew[i], tgt, kw, cw, Z_DT);
                step_spring(&mut p.ew_t[i], 1.3 * p.ew[i].a, kt, ct, Z_DT);
            }
            let m_hang = if p.lsad {
                0.7
            } else if o.drop {
                0.12
            } else {
                0.26
            };
            let hk: f64 = if p.lsad { 20.0 } else { 34.0 };
            let hc = 1.7 * hk.sqrt();
            for i in 0..2 {
                let tgt = (if i == 1 { 1.0 } else { -1.0 }) * p.lf * m_hang * p.ld;
                step_spring(&mut p.eh[i], tgt, hk, hc, Z_DT);
            }
        }
        PartParams::Tail(o) => {
            let w = spring_w(o.bounce, o.soft);
            let e_dir = if o.rev { -1.0 } else { 1.0 };
            let mut r = a / o.period;
            let mut s = 1.0;
            if o.on_pet == "wag" {
                let g = (a - p.pet_at) < 0.35;
                p.pl += ((if g { 1.0 } else { 0.0 }) - p.pl)
                    * (1.0 - (-Z_DT / if g { 0.25 } else { 0.7 }).exp());
                let wph = p.wph.get_or_insert(r);
                *wph += (Z_DT / o.period) * (1.0 + 3.0 * p.pl * (o.pet_amt / 100.0));
                r = *wph;
                s = 1.0 + 0.2 * p.pl * (o.pet_amt / 100.0);
            } else {
                p.wph = None;
            }
            let l = e_dir * (TAU * r).sin() * o.range * DEG2RAD * (o.amp / 100.0) * s * (1.0 - 0.75 * p.lf);
            step_spring(
                &mut p.tail_base,
                l + p.kick + p.lx + e_dir * p.bx * 1.6,
                0.3 * w.k,
                0.45 * w.c,
                Z_DT,
            );
            step_spring(&mut p.tail_tip, p.tail_base.a, 0.3 * w.tk, 0.5 * w.tc, Z_DT);

            let c = o.wobble / 100.0;
            let (kp, cp) = spring_g(c, 120.0, 45.0, 0.4, 0.09);
            let (kd, cd) = spring_g(c, 90.0, 35.0, 0.4, 0.1);
            step_spring(&mut p.tw, 0.0, kp, cp, Z_DT);
            step_spring(&mut p.tw_t, 1.3 * p.tw.a, kd, cd, Z_DT);
            p.kick *= (-9.0 * Z_DT).exp();

            let g = p.pk_dir.filter(|v| v.is_finite()).unwrap_or(-FRAC_PI_2)
                + o.dir * DEG2RAD
                + o.curve * DEG2RAD * 0.5;
            let mut b = wrap_pi(FRAC_PI_2 - g);
            if b.abs() > PI - 0.12 {
                b = PI * e_dir;
            }
            let mx = (p.lf * b - p.body_a * p.lf) * (0.4 + 0.6 * p.ld.min(1.0));
            step_spring(&mut p.tr, p.tvx * p.lf * p.la, 18.0, 1.2 * 18.0f64.sqrt(), Z_DT);

            let y_up = p.lf_t > 0.0;
            let xk = if y_up { 10.0 } else { 28.0 };
            step_spring(
                &mut p.hg,
                mx,
                xk,
                2.0 * (if y_up { 0.62 } else { 0.8 }) * xk.sqrt(),
                Z_DT,
            );
            step_spring(
                &mut p.hg_t,
                p.hg.a,
                0.7 * xk,
                2.0 * (if y_up { 0.5 } else { 0.75 }) * (0.7 * xk).sqrt(),
                Z_DT,
            );
        }
        PartParams::Wing(o) => {
            let r_rad = o.range * DEG2RAD;
            let warp = |e_val: f64, n_val: f64| -> f64 {
                let r = (2.0 * (0.5 - n_val) * PI) / (1.0 - (TAU * n_val).cos()).max(0.001);
                e_val + r.clamp(-0.95, 0.95) * (1.0 - (TAU * e_val).cos()) / TAU
            };
            let c_down = 0.25 + 0.5 * (o.down / 100.0);
            let wrap01 = |v: f64| ((v % 1.0) + 1.0) % 1.0;
            let f = (TAU * warp(wrap01((a - p.w_t0) / o.period), c_down)).cos() * (o.amp / 100.0);
            let m = 0.85;
            let g = -0.85 * (o.depth / 100.0);
            let bw = 0.18;
            let hold_m = 0.6;
            let y = bw + (o.flaps as f64) * o.flap_t;
            let u = a - p.w_press;
            let mut x = m;
            let mut w = 0.0;
            if u >= 0.0 && u < y + hold_m {
                w = if u < bw {
                    ss5(u / bw)
                } else if u < y {
                    1.0
                } else {
                    1.0 - ss5((u - y) / hold_m)
                };
                if u >= bw && u < y {
                    if p.w_sync != Some(p.w_press) {
                        p.w_t0 = p.w_press + y;
                        p.w_sync = Some(p.w_press);
                    }
                    let t_frac = ((u - bw) / o.flap_t) % 1.0;
                    x = g + (m - g) * ((TAU * t_frac).cos() + 1.0) * 0.5;
                }
            }
            let alt = if u >= bw && u < y + hold_m {
                (TAU * warp(wrap01((a - p.w_t0) / o.period), c_down)).cos() * (o.amp / 100.0)
            } else {
                f
            };
            let mut v = (if u >= 0.0 && u < bw { p.w_from } else { alt }) * (1.0 - w) + x * w;
            let p_lf = p.lf.clamp(0.0, 1.0);
            if p_lf > 0.01 {
                let ht0 = *p.h_t0.get_or_insert(a);
                let t_cyc = 1.5 / (0.5 + p.lsp);
                let ph = ((a - ht0) % t_cyc) / t_cyc;
                let hv = if ph < 0.27 {
                    m + (g - m) * ss5(ph / 0.27)
                } else if ph < 0.73 {
                    g + (m - g) * ss5((ph - 0.27) / 0.46)
                } else {
                    m
                };
                v = v * (1.0 - p_lf) + (m + (hv - m) * p.la) * p_lf;
            } else {
                p.h_t0 = None;
            }
            p.w_g = Some(v);
            let tg = v * r_rad;
            let e_soft = o.soft / 100.0;
            let o_bnd = lerp(1.0, 0.6, o.bounce / 100.0);
            let s_k = lerp(900.0, 500.0, if w < 0.5 { e_soft } else { 0.0 });
            let l_c = 2.0 * o_bnd.max(0.85) * s_k.sqrt();
            let rk = lerp(500.0, 45.0, e_soft);
            let ic = 2.0 * o_bnd * rk.sqrt();
            for i in 0..2 {
                let side_tg = (if i == 1 { -1.0 } else { 1.0 }) * tg;
                step_spring(&mut p.wing[i], side_tg, s_k, l_c, Z_DT);
                let tip_tg = p.wing[i].a + (if i == 1 { -1.0 } else { 1.0 }) * p.ly;
                step_spring(&mut p.wing_tip[i], tip_tg, rk, ic, Z_DT);
            }
        }
        PartParams::Prop(_) => {}
    }
}

pub fn apply_press(params: &PartParams, p: &mut PartPhys, t: f64) {
    match params {
        PartParams::Prop(o) => match o.anim.as_str() {
            "hop" => p.t_hop = Some(t),
            "shake" => p.t_shake = Some(t),
            "float" | "roll" => p.t_bump = Some(t),
            "peek" => {
                let r = t - p.pk0.unwrap_or(0.0);
                if p.pk0.is_none() || r < o.pk_wait {
                    p.pk0 = Some(t - o.pk_wait);
                }
            }
            _ => {}
        },
        PartParams::Tail(o) => {
            p.pk = 1.0;
            p.press = t;
            p.w_press = t;
            p.w_from = p.w_g.unwrap_or(0.0);
            p.tw.v -= (if o.rev { -1.0 } else { 1.0 }) * (o.kick / 100.0) * 6.0;
        }
        _ => {
            p.pk = 1.0;
            p.press = t;
            p.w_press = t;
            p.w_from = p.w_g.unwrap_or(0.0);
        }
    }
}

pub fn apply_drag(params: &PartParams, p: &mut PartPhys, dx_norm: f64, dy_norm: f64) {
    match params {
        PartParams::Tail(_) => {
            p.tvx += 0.5 * ((0.32 * dx_norm).clamp(-1.2, 1.2) - p.tvx);
        }
        PartParams::Ear(o) if o.on_drag => {
            let s = if p.lf_t > 0.0 {
                p.la * if p.lsad { 0.2 } else { 1.0 }
            } else {
                1.0
            };
            p.lx += 0.5 * ((-0.11 * dx_norm * s).clamp(-0.45, 0.45) - p.lx);
            p.ly += 0.5 * ((0.09 * dy_norm).clamp(-0.35, 0.35) - p.ly);
        }
        _ => {}
    }
}

pub fn apply_beat(params: &PartParams, p: &mut PartPhys, sim_t: f64, intensity: f64) {
    let o = intensity.clamp(0.5, 1.4);
    match params {
        PartParams::Tail(t) if t.on_beat => {
            p.bsg = -p.bsg;
            p.bx = 0.2 * p.bsg * o;
        }
        PartParams::Ear(e) if e.on_beat => {
            if e.beat_sway {
                p.bsg = -p.bsg;
                p.bx = 0.2 * p.bsg * o;
            } else {
                p.bt = sim_t;
                p.b_a = 0.15 * o;
            }
        }
        _ => {}
    }
}

#[allow(dead_code)]
pub fn apply_key(params: &PartParams, p: &mut PartPhys, sim_t: f64) {
    if let PartParams::Ear(e) = params {
        if e.on_key {
            p.kt = sim_t;
        }
    }
}

pub fn apply_pet(params: &PartParams, p: &mut PartPhys, sim_t: f64) {
    match params {
        PartParams::Ear(e) if e.on_pet != "none" => p.pet_at = sim_t,
        PartParams::Tail(t) if t.on_pet != "none" => p.pet_at = sim_t,
        _ => {}
    }
}

pub fn apply_lift(
    part_id: &str,
    part_kind: &str,
    p: &mut PartPhys,
    amt: f64,
    body_a: f64,
    still_ids: &[String],
    hide_ids: &[String],
    sad_ids: &[String],
    drop_wobble: f64,
) {
    let is_still = still_ids.iter().any(|x| x == part_id);
    let is_hide = amt > 0.0 && hide_ids.iter().any(|x| x == part_id);
    p.lf_t = if is_still || is_hide { 0.0 } else { amt };
    p.body_a = body_a;
    p.ld = (drop_wobble / 50.0).clamp(0.0, 2.0);
    p.lsad = part_kind == "ear" && sad_ids.iter().any(|x| x == part_id);
}

pub struct PosedBoneRig {
    pub is_ear: bool,
    pub n_bones: usize,
    pub pose_pin: [f32; 2],
    pub len_c: f32,
    pub dirs0: f32,
    pub a_ang: f32,
    pub b_ang: f32,
    pub bend: f32,
    pub flip_m: f32,
    pub p_fracs: [f32; 13],
    pub joints: [[f32; 2]; 12],
    pub rot_cs: [[f32; 2]; 12],
    pub rest_start: [[f32; 2]; 12],
    pub d_widths: [f32; 12],
    pub ear_spine: [[f32; 2]; 49],
}

pub fn solve_pose(
    part: &RigPartConfig,
    params: &PartParams,
    p: &mut PartPhys,
    side_idx: usize,
    pick: &RigMeshMeta,
    common: &RigCommonConfig,
    rest: bool,
) -> PosedBoneRig {
    let (pose_pin, len_c, dirs, a_raw, b_raw, bend, fr) = match params {
        PartParams::Ear(e) => {
            let pp = [
                pick.pin[0] as f64 + e.off[side_idx][0],
                pick.pin[1] as f64 + e.off[side_idx][1],
            ];
            let lp_c = (pick.len as f64) * (e.scale / 100.0);
            let u = if side_idx == 1 || e.mirror || e.tilt_l.is_none() {
                e.tilt
            } else {
                e.tilt_l.unwrap_or(e.tilt)
            };
            let m_dir = (pick.dir as f64) + (if side_idx == 1 { -1.0 } else { 1.0 }) * u * DEG2RAD;
            let dirs = vec![m_dir; e.bones.len()];
            let lim = e.range * DEG2RAD;
            let y_s = if side_idx == 1 { 1.0 } else { -1.0 };
            let x = y_s * (p.dip + (-0.8 * p.lys).clamp(-0.15, 0.3));
            let w = p.lxs;
            let k = y_s * p.kw;
            let clamp_fn = if e.drop { rt } else { nt };
            let mut a = clamp_fn(p.ears[side_idx].a + p.ew[side_idx].a + x + w + k, lim);
            let mut b = clamp_fn(
                p.ear_tip[side_idx].a + p.ew_t[side_idx].a + 1.15 * x + 1.25 * w + 1.5 * k,
                lim,
            );
            a *= 1.0 - e.stiff / 100.0;
            a += 0.7 * p.eh[side_idx].a;
            b += 1.25 * p.eh[side_idx].a;
            (pp, lp_c, dirs, a, b, spring_w(e.bounce, e.soft).bend, &e.bones)
        }
        PartParams::Wing(w_p) => {
            let (wx, wy, wt) = if w_p.sym || side_idx == 1 {
                (w_p.x, w_p.y, w_p.tilt)
            } else {
                (
                    w_p.x_l.unwrap_or(w_p.x),
                    w_p.y_l.unwrap_or(w_p.y),
                    w_p.tilt_l.unwrap_or(w_p.tilt),
                )
            };
            let pp = [
                pick.pin[0] as f64 + (if side_idx == 1 { 1.0 } else { -1.0 }) * wx,
                pick.pin[1] as f64 + wy,
            ];
            let lp_c = (pick.len as f64) * (w_p.scale / 100.0);
            let m_dir = (pick.dir as f64) + (if side_idx == 1 { -1.0 } else { 1.0 }) * wt * DEG2RAD;
            let dirs = vec![m_dir; w_p.bones.len()];
            let lim = w_p.range * DEG2RAD;
            let a = it_soft(p.wing[side_idx].a, lim);
            let b = a + nt(p.wing_tip[side_idx].a - p.wing[side_idx].a, (0.45 * lim).max(0.12));
            (pp, lp_c, dirs, a, b, spring_w(w_p.bounce, w_p.soft).bend, &w_p.bones)
        }
        PartParams::Tail(t_p) => {
            let pp = if t_p.head && t_p.rel.is_some() {
                let rel = t_p.rel.unwrap();
                [pick.pin[0] as f64 + rel[0], pick.pin[1] as f64 + rel[1]]
            } else {
                t_p.root
            };
            let lp_c = (pick.len as f64) * (t_p.scale / 100.0);
            let t0 = (pick.dir as f64) + t_p.dir * DEG2RAD;
            let c_rad = t_p.curve * DEG2RAD;
            let mut acc = 0.0;
            let dirs: Vec<f64> = t_p
                .bones
                .iter()
                .map(|&f| {
                    let mid = acc + f * 0.5;
                    acc += f;
                    t0 + c_rad * mid
                })
                .collect();
            let lim = t_p.range * DEG2RAD;
            let a = nt(p.tail_base.a + p.tw.a, lim) + p.hg.a + p.tr.a;
            let b = nt(p.tail_tip.a + p.tw_t.a, lim) + p.hg_t.a + 1.8 * p.tr.a;
            (pp, lp_c, dirs, a, b, spring_w(t_p.bounce, t_p.soft).bend, &t_p.bones)
        }
        PartParams::Prop(_) => unreachable!(),
    };

    let (a, b) = if rest { (0.0, 0.0) } else { (a_raw, b_raw) };
    let n_bones = fr.len().min(12);
    let mut joints_d = [[0.0f64; 2]; 13];
    joints_d[0] = pose_pin;

    let mut ang = [0.0f64; 12];
    let mut w_cum = 0.0;
    let (mut kx, mut ky) = (pose_pin[0], pose_pin[1]);
    for e in 0..n_bones {
        let f = fr[e];
        let mid = w_cum + f * 0.5;
        w_cum += f;
        let r = dirs[e] + a + (b - a) * bend * mid * mid;
        ang[e] = r;
        kx += r.cos() * len_c * f;
        ky += r.sin() * len_c * f;
        joints_d[e + 1] = [kx, ky];
    }

    let mut p_fracs = [0.0f32; 13];
    for e in 0..n_bones {
        p_fracs[e + 1] = p_fracs[e] + fr[e] as f32;
    }

    let jf = (common.joint / 100.0) as f32;
    let mut d_widths = [0.0f32; 12];
    if n_bones > 0 {
        d_widths[0] = (0.5 * fr[0] as f32).min(0.03 + 0.12 * jf);
        for t in 1..n_bones {
            d_widths[t] = 0.5 * (fr[t - 1] as f32).min(fr[t] as f32) * jf;
        }
    }

    let (h_dir, s_dir) = (pick.dir.cos(), pick.dir.sin());
    let mut joints = [[0.0f32; 2]; 12];
    let mut rot_cs = [[0.0f32; 2]; 12];
    let mut rest_start = [[0.0f32; 2]; 12];
    for e in 0..n_bones {
        let rel_a = (ang[e] as f32) - pick.dir;
        joints[e] = [joints_d[e][0] as f32, joints_d[e][1] as f32];
        rot_cs[e] = [rel_a.cos(), rel_a.sin()];
        let rs_off = (len_c as f32) * p_fracs[e];
        rest_start[e] = [
            pose_pin[0] as f32 + h_dir * rs_off,
            pose_pin[1] as f32 + s_dir * rs_off,
        ];
    }

    let mut flip_m = 1.0f32;
    if let PartParams::Tail(t_p) = params {
        if t_p.flip {
            let idx = (((n_bones) as f64 / 3.0).round() as usize).max(1).min(n_bones);
            let rj = joints_d[idx];
            let j0 = joints_d[0];
            let mut aa = (rj[1] - j0[1]).atan2(rj[0] - j0[0]) + FRAC_PI_2;
            aa = wrap_pi(aa);
            let o = ((aa / (7.0 * DEG2RAD)).clamp(-1.0, 1.0) * FRAC_PI_2).sin();
            let mut m = if t_p.fur == "L" { -o } else { o };
            let lf_eff = if rest { 0.0 } else { p.lf };
            if lf_eff > 0.02 {
                if let Some(keep) = p.fs_keep {
                    m = keep;
                } else {
                    p.fs_keep = Some(m);
                }
            } else {
                p.fs_keep = Some(m);
            }
            flip_m = m as f32;
        }
    }

    let is_ear = part.kind == "ear";
    let mut ear_spine = [[0.0f32; 2]; 49];
    if is_ear {
        ear_spine[0] = [pose_pin[0] as f32, pose_pin[1] as f32];
        let d0 = dirs[0];
        for r in 1..=48 {
            let tau = (r as f64 - 0.5) / 48.0;
            let ang_t = d0 + a + (b - a) * bend * tau * tau;
            ear_spine[r] = [
                ear_spine[r - 1][0] + (ang_t.cos() * len_c / 48.0) as f32,
                ear_spine[r - 1][1] + (ang_t.sin() * len_c / 48.0) as f32,
            ];
        }
    }

    PosedBoneRig {
        is_ear,
        n_bones,
        pose_pin: [pose_pin[0] as f32, pose_pin[1] as f32],
        len_c: len_c as f32,
        dirs0: dirs[0] as f32,
        a_ang: a as f32,
        b_ang: b as f32,
        bend: bend as f32,
        flip_m,
        p_fracs,
        joints,
        rot_cs,
        rest_start,
        d_widths,
        ear_spine,
    }
}

pub struct PropTransform {
    pub x: f32,
    pub y: f32,
    pub rot_rad: f32,
    pub sx: f32,
    pub sy: f32,
    pub w: f32,
    pub h: f32,
    pub bottom_anchored: bool,
}

pub fn eval_prop_motion(
    t: &PropParams,
    e: &mut PartPhys,
    side: &PropMeta,
    i: f64,
    rest: bool,
) -> PropTransform {
    let o = t.scale / 100.0;
    let s = side.w as f64 * o;
    let l = side.h as f64 * o;
    let c = side.cx as f64 + t.pos[0];
    let p = side.cy as f64 + t.pos[1];
    let mut f = c;
    let mut d = p;
    let mut h = 0.0;
    let mut u = 1.0;
    let mut m = 1.0;
    let mut bottom = false;

    if rest {
        if t.anim == "roll" {
            f = c + t.rl_x1;
        }
        return PropTransform {
            x: f as f32,
            y: d as f32,
            rot_rad: 0.0,
            sx: 1.0,
            sy: 1.0,
            w: s as f32,
            h: l as f32,
            bottom_anchored: false,
        };
    }

    match t.anim.as_str() {
        "hop" => {
            bottom = true;
            let n = 0.55;
            let every = if t.hop_every > 0.0 { t.hop_every } else { 1e9 };
            let r = (i % every) / n;
            let a = e.t_hop.map(|th| (i - th) / n).unwrap_or(9.0);
            let o_ph = if (0.0..1.2).contains(&a) { a } else { r };
            if o_ph < 1.0 {
                d -= 4.0 * o_ph * (1.0 - o_ph) * t.hop_h;
            }
            let l_sq = t.hop_squash / 100.0;
            if (1.0..1.18).contains(&o_ph) {
                let ts = (PI * (o_ph - 1.0) / 0.18).sin();
                m = 1.0 - 0.5 * l_sq * ts;
                u = 1.0 + 0.35 * l_sq * ts;
            } else if o_ph < 0.08 {
                let ts = (PI * o_ph / 0.08).sin();
                m = 1.0 - 0.3 * l_sq * ts;
                u = 1.0 + 0.2 * l_sq * ts;
            }
        }
        "float" => {
            let r = if t.fl_period > 0.0 { t.fl_period } else { 4.0 };
            let a = e.t_bump.map(|tb| i - tb).unwrap_or(9.0);
            let o_b = if (0.0..1.5).contains(&a) {
                (PI * a / 0.6).sin() * (-2.5 * a).exp() * 18.0
            } else {
                0.0
            };
            f += t.fl_rx * (TAU * i / r).sin();
            d += t.fl_ry * (TAU * i / (1.37 * r) + 1.0).sin() - o_b;
            h = 6.0 * DEG2RAD * (t.fl_rx / 40.0).min(1.0) * (TAU * i / r).cos();
        }
        "shake" => {
            let r = if t.sh_every > 0.0 { t.sh_every } else { 1e9 };
            let a = if t.sh_dur > 0.0 { t.sh_dur } else { 0.6 };
            let mut o_ph = (i % r) / a;
            if let Some(ts) = e.t_shake {
                if i - ts >= 0.0 && i - ts < a {
                    o_ph = (i - ts) / a;
                }
            }
            if o_ph < 1.0 {
                let e_sq = (1.0 - o_ph) * (1.0 - o_ph);
                f += t.sh_amp * (TAU * 9.0 * o_ph).sin() * e_sq;
                h = t.sh_deg * DEG2RAD * (TAU * 7.0 * o_ph + 1.0).sin() * e_sq;
            }
        }
        "roll" => {
            let x1 = c + t.rl_x1;
            let x2 = c + t.rl_x2;
            let a = (x2 - x1).abs().max(1.0) / t.rl_speed.max(10.0);
            let o_pause = t.rl_pause;
            let l_tot = 2.0 * (a + o_pause);
            let p_ph = ((i % l_tot) + l_tot) % l_tot;
            let u_p = if p_ph < o_pause {
                0.0
            } else if p_ph < o_pause + a {
                y_ease((p_ph - o_pause) / a)
            } else if p_ph < 2.0 * o_pause + a {
                1.0
            } else {
                1.0 - y_ease((p_ph - 2.0 * o_pause - a) / a)
            };
            f = x1 + (x2 - x1) * u_p;
            h = (f - x1) / (s * 0.5).max(1.0);
            let m_b = e.t_bump.map(|tb| (i - tb) / 0.45).unwrap_or(9.0);
            if (0.0..1.0).contains(&m_b) {
                d -= 4.0 * m_b * (1.0 - m_b) * 24.0;
            }
        }
        "peek" => {
            let r = t.pk_wait;
            let a = t.pk_stay;
            let o_in = 0.38;
            let s_out = 0.32;
            let l_cyc = r + o_in + a + s_out;
            let mut pk0 = e.pk0.unwrap_or(i - 0.5 * r);
            if pk0 > i {
                pk0 = i - 0.5 * r;
            }
            while i - pk0 > l_cyc {
                pk0 += l_cyc;
            }
            e.pk0 = Some(pk0);
            let c_ph = i - pk0;
            let n_back = |x: f64| 1.0 + 2.6 * (x - 1.0).powi(3) + 1.6 * (x - 1.0).powi(2);
            let p_val = if c_ph < r {
                0.0
            } else if c_ph < r + o_in {
                n_back((c_ph - r) / o_in)
            } else if c_ph < r + o_in + a {
                1.0
            } else {
                1.0 - ((c_ph - r - o_in - a) / s_out).powi(3)
            };
            f += t.pk_dx * p_val;
            d += t.pk_dy * p_val;
            h = 10.0 * (if t.pk_dx >= 0.0 { 1.0 } else { -1.0 }) * DEG2RAD * p_val;
        }
        _ => {}
    }

    PropTransform {
        x: f as f32,
        y: d as f32,
        rot_rad: h as f32,
        sx: u as f32,
        sy: m as f32,
        w: s as f32,
        h: l as f32,
        bottom_anchored: bottom,
    }
}

#[allow(dead_code)]
pub fn is_part_active(params: &PartParams, n: &PartPhys, t: f64) -> bool {
    match params {
        PartParams::Prop(o) => {
            o.anim == "float"
                || o.anim == "roll"
                || o.anim == "peek"
                || (o.anim == "hop" && (o.hop_every > 0.0 || n.t_hop.is_some_and(|h| t - h < 0.8)))
                || (o.anim == "shake"
                    && (o.sh_every > 0.0 || n.t_shake.is_some_and(|s| t - s < o.sh_dur + 0.1)))
        }
        PartParams::Ear(o) => {
            if o.amp > 0.0 {
                return true;
            }
            is_spring_moving(n, t)
        }
        PartParams::Tail(o) => {
            if o.amp > 0.0 {
                return true;
            }
            is_spring_moving(n, t)
        }
        PartParams::Wing(o) => {
            if o.amp > 0.0 {
                return true;
            }
            is_spring_moving(n, t)
        }
    }
}

#[allow(dead_code)]
fn is_spring_moving(n: &PartPhys, t: f64) -> bool {
    if n.lf_t > 0.0 || n.lf > 0.001 {
        return true;
    }
    let springs = [
        n.ears[0], n.ears[1], n.ear_tip[0], n.ear_tip[1], n.ew[0], n.ew[1], n.ew_t[0],
        n.ew_t[1], n.tail_base, n.tail_tip, n.tw, n.tw_t, n.wing[0], n.wing[1], n.wing_tip[0],
        n.wing_tip[1], n.eh[0], n.eh[1], n.hg, n.hg_t,
    ];
    if springs.iter().any(|s| s.v.abs() > 0.001 || s.a.abs() > 0.001) {
        return true;
    }
    if t - n.press < 2.5 || t - n.bt < 0.5 || n.pl > 0.001 || t - n.pet_at < 0.5 {
        return true;
    }
    (n.lx.abs() + n.ly.abs() + n.bx.abs() + n.lxs.abs() + n.lys.abs()) > 0.001 || t - n.kt < 0.8
}

/// Matching `rig.js:754-762` (`Et()`).
pub fn canvas_margin_frac(kind: &str, params: &PartParams, first_side: Option<&RigMeshMeta>) -> f64 {
    const TT: f64 = 0.3;
    if kind != "tail" {
        return TT;
    }
    let Some(side) = first_side else {
        return 0.6;
    };
    let PartParams::Tail(t) = params else {
        return 0.6;
    };
    let r = if t.head && t.rel.is_some() {
        let rel = t.rel.unwrap();
        [side.pin[0] as f64 + rel[0], side.pin[1] as f64 + rel[1]]
    } else {
        t.root
    };
    let i = (side.len as f64) * (t.scale / 100.0);
    let a = [i - r[0], i - r[1], r[0] + i - 800.0, r[1] + i - 800.0]
        .into_iter()
        .fold(f64::NEG_INFINITY, f64::max)
        / 800.0
        + 0.06;
    ((a / 0.05).ceil() * 0.05).clamp(TT, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_step_converges_and_clamps_are_finite() {
        let mut s = Spring { a: 0.0, v: 0.0 };
        let w = spring_w(60.0, 40.0);
        for _ in 0..720 {
            step_spring(&mut s, 0.5, w.k, w.c, Z_DT);
        }
        assert!((s.a - 0.5).abs() < 1e-3);
        assert!(nt(100.0, 0.5).abs() <= 0.5);
        assert!(rt(100.0, 0.5).abs() <= 0.5);
        assert!(it_soft(100.0, 0.5).abs() <= 0.501);
        assert!((ss5(0.5) - 0.5).abs() < 1e-9);
    }
}
