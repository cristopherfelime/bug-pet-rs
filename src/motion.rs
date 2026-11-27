use crate::config::CharacterConfig;
use rand::Rng;
use serde_json::Value;
use std::f64::consts::PI;

const DEG2RAD: f64 = PI / 180.0;

#[inline]
fn cubic_bezier_sample(x1: f64, y1: f64, x2: f64, y2: f64, t_x: f64) -> f64 {
    let x = t_x.clamp(0.0, 1.0);
    // Newton-Raphson + bisection solve for parameter u where BezierX(u) == x
    let bx = |u: f64| {
        let iu = 1.0 - u;
        3.0 * iu * iu * u * x1 + 3.0 * iu * u * u * x2 + u * u * u
    };
    let dbx = |u: f64| {
        let iu = 1.0 - u;
        3.0 * iu * iu * x1 + 6.0 * iu * u * (x2 - x1) + 3.0 * u * u * (1.0 - x2)
    };
    let mut u = x;
    for _ in 0..5 {
        let err = bx(u) - x;
        if err.abs() < 1e-5 {
            break;
        }
        let d = dbx(u);
        if d.abs() < 1e-6 {
            break;
        }
        u = (u - err / d).clamp(0.0, 1.0);
    }
    let iu = 1.0 - u;
    3.0 * iu * iu * u * y1 + 3.0 * iu * u * u * y2 + u * u * u
}

/// Drag swing (`#stack`), secondary head wobble (`#pat`), and drop squish (`--psx/--psy/--plopY`)
/// matching `holdLoop` (`pet.js:664-696`).
#[derive(Clone, Debug)]
pub struct HoldPhysics {
    pub on: bool,
    pub a: f64,
    pub w: f64,
    pub h: f64,
    pub hw: f64,
    pub vx: f64,
    pub lx: Option<f64>,
    pub lt: f64,
    pub plop_at_s: f64,
    pub alt: f64,
    pub active: bool,
}

impl Default for HoldPhysics {
    fn default() -> Self {
        Self {
            on: false,
            a: 0.0,
            w: 0.0,
            h: 0.0,
            hw: 0.0,
            vx: 0.0,
            lx: None,
            lt: 0.0,
            plop_at_s: -10.0,
            alt: 1.0,
            active: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HoldOutputs {
    pub swing_rad: f32,
    pub pat_extra_rad: f32,
    pub sx: f32,
    pub sy: f32,
    pub plop_y_800: f32,
    pub lift_a: f64,
}

impl HoldPhysics {
    pub fn on_drag_start(&mut self, now_s: f64) {
        self.on = true;
        self.lx = None;
        self.vx = 0.0;
        self.lt = now_s;
        self.active = true;
    }

    pub fn on_drag_move(&mut self, screen_x_dip: f64, now_s: f64) {
        if let Some(lx) = self.lx {
            if now_s > self.lt {
                let dt = (now_s - self.lt).min(0.1);
                let inst = (screen_x_dip - lx) / dt.max(0.004);
                self.vx += (inst - self.vx) * (1.0 - (-dt / 0.08).exp());
            }
        }
        self.lx = Some(screen_x_dip);
        self.lt = now_s;
    }

    pub fn on_drag_end(&mut self, drop_wobble: f64, now_s: f64) {
        self.on = false;
        self.plop_at_s = now_s;
        let e = (drop_wobble.clamp(0.0, 200.0)) / 100.0;
        let t = if self.h.abs() > 0.004 {
            -self.h.signum()
        } else if self.a.abs() > 0.004 {
            -self.a.signum()
        } else {
            self.alt = -self.alt;
            self.alt
        };
        self.hw += 2.6 * e * t;
        self.a = 0.0;
        self.w = 0.0;
        self.active = true;
    }

    pub fn step(
        &mut self,
        dt_s: f64,
        now_s: f64,
        drag_swing: f64,
        drop_squish: f64,
        size_dip: f64,
    ) -> HoldOutputs {
        if !self.active && !self.on {
            return HoldOutputs {
                swing_rad: 0.0,
                pat_extra_rad: 0.0,
                sx: 1.0,
                sy: 1.0,
                plop_y_800: 0.0,
                lift_a: 0.0,
            };
        }

        let dt = dt_s.clamp(0.001, 0.05);
        if (now_s - self.lt) > 0.060 {
            self.vx *= (-10.0 * dt).exp();
        }
        let o = (drag_swing / 100.0).max(0.0);
        let a_cap = 0.08 + 0.62 * o.min(1.0) + 0.3 * (o - 1.0).max(0.0);
        let r = if self.on && a_cap > 0.0 {
            a_cap * (self.vx / size_dip.max(48.0) * 0.11 * o / a_cap).tanh()
        } else {
            0.0
        };

        let h = dt / 4.0;
        for _ in 0..4 {
            self.w += (38.0 * (r - self.a) - 7.5 * self.w) * h;
            self.a += self.w * h;
            self.hw += (40.0 * (0.55 * self.a - self.h) - 6.0 * self.hw) * h;
            self.h += self.hw * h;
        }

        let mut c = 1.0f64;
        let u = (drop_squish / 100.0).clamp(0.0, 2.0);
        let d = now_s - self.plop_at_s;
        if (0.0..0.6).contains(&d) {
            c = 1.0 - 0.1 * u * (-7.0 * d).exp() * ((3.2 * d).min(1.0) * PI).sin();
        }

        let sy = c as f32;
        let sx = (1.0 + 0.6 * (1.0 - c)) as f32;
        let plop_y_800 = (0.36 * (1.0 - c) * 800.0) as f32;

        if !self.on && (self.a.abs() + self.w.abs() + self.h.abs() + self.hw.abs()) < 0.001 && d > 0.6 {
            self.a = 0.0;
            self.w = 0.0;
            self.h = 0.0;
            self.hw = 0.0;
            self.active = false;
        }

        HoldOutputs {
            swing_rad: self.a as f32,
            pat_extra_rad: (self.h - self.a) as f32,
            sx,
            sy,
            plop_y_800,
            lift_a: self.a,
        }
    }
}

/// Press shiver (`Motion.Shiver`, `motion.js:121-144`).
#[derive(Clone, Debug, Default)]
pub struct ShiverState {
    press_at: Option<f64>,
    last_jitter_at: f64,
    pub body_tx_800: f32,
    pub body_ty_800: f32,
    pub body_rot_rad: f32,
    pub head_tx_800: f32,
    pub head_ty_800: f32,
    pub head_rot_rad: f32,
}

impl ShiverState {
    pub fn down(&mut self, cfg: &CharacterConfig, now_s: f64) {
        if cfg.shiver_head != 0.0 || cfg.shiver_body != 0.0 {
            self.press_at = Some(now_s);
            self.last_jitter_at = 0.0;
        }
    }

    pub fn up(&mut self) {
        self.press_at = None;
        self.body_tx_800 = 0.0;
        self.body_ty_800 = 0.0;
        self.body_rot_rad = 0.0;
        self.head_tx_800 = 0.0;
        self.head_ty_800 = 0.0;
        self.head_rot_rad = 0.0;
    }

    pub fn step(&mut self, cfg: &CharacterConfig, now_s: f64) {
        let Some(p_at) = self.press_at else {
            return;
        };
        if now_s - p_at < 0.280 {
            return;
        }
        if now_s - self.last_jitter_at >= 0.033 {
            self.last_jitter_at = now_s;
            let mut rng = rand::thread_rng();
            let mut rnd = || 2.0 * rng.gen::<f64>() - 1.0;
            // Notice CSS px offset is `shiver * (size/200)`, so in 800 space that's `shiver * 4.0`.
            let sb = cfg.shiver_body;
            let sh = cfg.shiver_head;
            if sb != 0.0 {
                self.body_tx_800 = (rnd() * sb * 4.0) as f32;
                self.body_ty_800 = (rnd() * sb * 2.0) as f32;
                self.body_rot_rad = (rnd() * sb * 0.35 * DEG2RAD) as f32;
            }
            if sh != 0.0 {
                self.head_tx_800 = (rnd() * sh * 4.0) as f32;
                self.head_ty_800 = (rnd() * sh * 2.0) as f32;
                self.head_rot_rad = (rnd() * sh * 0.6 * DEG2RAD) as f32;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct MotionSpec {
    pub style: String,
    pub amp: f64,
    pub dy: f64,
    pub rot: f64,
    pub alt: bool,
    pub dur: f64,
    pub blink_every: u32,
    pub fast: String,
    pub gap: f64,
}

impl MotionSpec {
    pub fn from_config(raw: &Value, is_key: bool, legacy_style: &str, legacy_amp: f64) -> Self {
        let mut spec = if is_key {
            MotionSpec {
                style: "bob".to_string(),
                amp: 1.0,
                dy: -2.4,
                rot: 0.0,
                alt: false,
                dur: 160.0,
                blink_every: 0,
                fast: "cap".to_string(),
                gap: 200.0,
            }
        } else {
            MotionSpec {
                style: "nod".to_string(),
                amp: 1.0,
                dy: 3.2,
                rot: 2.6,
                alt: true,
                dur: 0.0,
                blink_every: 0,
                fast: String::new(),
                gap: 0.0,
            }
        };

        if raw.is_null() {
            if matches!(legacy_style, "nod" | "bob" | "press" | "custom") {
                spec.style = legacy_style.to_string();
            }
            if legacy_amp.is_finite() {
                spec.amp = legacy_amp.clamp(0.2, 3.0);
            }
            return spec;
        }

        if let Some(st) = raw.get("style").and_then(|v| v.as_str()) {
            if matches!(st, "nod" | "bob" | "press" | "custom") {
                spec.style = st.to_string();
            }
        }
        if let Some(v) = raw.get("amp").and_then(|x| x.as_f64()) {
            spec.amp = v.clamp(0.2, 3.0);
        }
        if let Some(v) = raw.get("dy").and_then(|x| x.as_f64()) {
            spec.dy = v.clamp(-16.0, 16.0);
        }
        if let Some(v) = raw.get("rot").and_then(|x| x.as_f64()) {
            spec.rot = v.clamp(0.0, 15.0);
        }
        if let Some(v) = raw.get("alt").and_then(|x| x.as_bool()) {
            spec.alt = v;
        }
        if let Some(v) = raw.get("dur").and_then(|x| x.as_f64()) {
            spec.dur = v.clamp(0.0, 800.0);
        }
        let blink_raw = raw
            .get("blinkEvery")
            .and_then(|x| x.as_f64())
            .or_else(|| {
                if raw.get("blink").and_then(|x| x.as_bool()) == Some(true) {
                    Some(1.0)
                } else {
                    None
                }
            });
        if let Some(v) = blink_raw {
            spec.blink_every = v.clamp(0.0, 10.0).round() as u32;
        }
        if is_key {
            if let Some(f) = raw.get("fast").and_then(|x| x.as_str()) {
                if matches!(f, "cap" | "loop" | "all") {
                    spec.fast = f.to_string();
                }
            }
            if let Some(g) = raw.get("gap").and_then(|x| x.as_f64()) {
                spec.gap = g.clamp(60.0, 800.0);
            }
        }
        spec
    }
}

#[derive(Clone, Debug)]
struct ActiveReactorAnim {
    start_s: f64,
    dur_s: f64,
    is_bob: bool,
    dy_800: f64,
    rot_rad: f64,
    peak_offset: f64,
}

/// Port of `Motion.Reactor` (`motion.js:30-120`) for both music beats and key strokes.
#[derive(Clone, Debug)]
pub struct ReactorState {
    sign: f64,
    last_beat_at: f64,
    beat_count: u32,
    last_key_at: f64,
    key_count: u32,
    active_anim: Option<ActiveReactorAnim>,
    pub press_until_s: Option<f64>,
    pub pending_blink: bool,
}

impl Default for ReactorState {
    fn default() -> Self {
        Self {
            sign: 1.0,
            last_beat_at: -10.0,
            beat_count: 0,
            last_key_at: -10.0,
            key_count: 0,
            active_anim: None,
            press_until_s: None,
            pending_blink: false,
        }
    }
}

impl ReactorState {
    pub fn trigger_beat(
        &mut self,
        cfg: &CharacterConfig,
        strength: f64,
        interval_s: f64,
        now_s: f64,
        busy: bool,
    ) {
        if busy || now_s - self.last_beat_at < 0.150 {
            return;
        }
        self.last_beat_at = now_s;
        let spec = MotionSpec::from_config(&cfg.beat_motion, false, &cfg.react_style, cfg.react_amp);
        self.fire(&spec, false, strength, interval_s, now_s);
    }

    pub fn trigger_key(&mut self, cfg: &CharacterConfig, now_s: f64, busy: bool) {
        if busy {
            return;
        }
        let spec = MotionSpec::from_config(&cfg.key_motion, true, &cfg.react_style, cfg.react_amp);
        let min_gap = if spec.fast == "all" {
            0.070
        } else {
            spec.gap.max(60.0) / 1000.0
        };
        if now_s - self.last_key_at < min_gap {
            return;
        }
        self.last_key_at = now_s;
        self.fire(&spec, true, 1.0, 0.0, now_s);
    }

    fn fire(&mut self, spec: &MotionSpec, is_key: bool, strength: f64, interval_s: f64, now_s: f64) {
        self.sign = -self.sign;
        if is_key {
            self.key_count += 1;
            if spec.blink_every > 0 && spec.style != "press" && self.key_count % spec.blink_every == 0 {
                self.pending_blink = true;
            }
        } else {
            self.beat_count += 1;
            if spec.blink_every > 0 && spec.style != "press" && self.beat_count % spec.blink_every == 0 {
                self.pending_blink = true;
            }
        }

        let mut dur_ms = spec.dur;
        if dur_ms == 0.0 {
            dur_ms = if is_key {
                150.0
            } else {
                let iv = if interval_s > 0.0 { interval_s } else { 0.5 };
                (1000.0 * iv * 0.5).clamp(140.0, 320.0)
            };
        }

        if spec.style == "press" {
            let p_ms = dur_ms.clamp(60.0, if is_key { 90.0 } else { 130.0 });
            self.press_until_s = Some(now_s + p_ms / 1000.0);
            self.active_anim = None;
            return;
        }

        let u = if is_key {
            1.0
        } else {
            (0.55 + 0.45 * strength.min(1.0)) * strength.max(1.0)
        };
        let (c, d, is_bob) = match spec.style.as_str() {
            "custom" => (
                spec.dy * u,
                spec.rot * u * (if spec.alt { self.sign } else { 1.0 }),
                false,
            ),
            "bob" => (-4.0 * spec.amp * u * (if is_key { 0.7 } else { 1.4 }), 0.0, true),
            _ => (
                3.2 * spec.amp * u * (if is_key { 0.7 } else { 1.4 }),
                (if is_key { 1.2 } else { 3.6 }) * spec.amp * u * self.sign,
                false,
            ),
        };

        // Notice motion.js scales CSS dy by `size/200`, which in `800` space is `dy * 4.0`.
        self.active_anim = Some(ActiveReactorAnim {
            start_s: now_s,
            dur_s: (dur_ms / 1000.0).max(0.05),
            is_bob,
            dy_800: c * 4.0,
            rot_rad: d * DEG2RAD,
            peak_offset: if is_key { 0.38 } else { 0.30 },
        });
    }

    pub fn step(&mut self, now_s: f64, busy: bool) -> (f32, f32, bool) {
        let mut press_active = false;
        if let Some(until) = self.press_until_s {
            if now_s < until && !busy {
                press_active = true;
            } else {
                self.press_until_s = None;
            }
        }

        let Some(ref anim) = self.active_anim else {
            return (0.0, 0.0, press_active);
        };
        let u = (now_s - anim.start_s) / anim.dur_s;
        if u >= 1.0 {
            self.active_anim = None;
            return (0.0, 0.0, press_active);
        }
        if u <= 0.0 {
            return (0.0, 0.0, press_active);
        }

        let h = anim.peak_offset;
        if anim.is_bob {
            let h2 = (h + 0.4).min(0.85);
            let dy = if u < h {
                let s = cubic_bezier_sample(0.2, 0.6, 0.4, 1.0, u / h);
                anim.dy_800 * s
            } else if u < h2 {
                let s = cubic_bezier_sample(0.4, 0.0, 0.4, 1.0, (u - h) / (h2 - h));
                lerp_f64(anim.dy_800, -0.3 * anim.dy_800, s)
            } else {
                let s = cubic_bezier_sample(0.4, 0.0, 0.4, 1.0, (u - h2) / (1.0 - h2));
                lerp_f64(-0.3 * anim.dy_800, 0.0, s)
            };
            (dy as f32, 0.0, press_active)
        } else {
            let w = if u < h {
                cubic_bezier_sample(0.2, 0.6, 0.4, 1.0, u / h)
            } else {
                1.0 - cubic_bezier_sample(0.4, 0.0, 0.4, 1.0, (u - h) / (1.0 - h))
            };
            ((anim.dy_800 * w) as f32, (anim.rot_rad * w) as f32, press_active)
        }
    }
}

#[inline]
fn lerp_f64(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_physics_swings_squishes_and_settles() {
        let mut h = HoldPhysics::default();
        h.on_drag_start(0.0);
        h.on_drag_move(0.0, 0.01);
        h.on_drag_move(200.0, 0.05);
        let mid = h.step(0.016, 0.066, 100.0, 100.0, 180.0);
        assert!(mid.swing_rad.abs() > 0.001);

        h.on_drag_end(100.0, 0.08);
        let plop = h.step(0.016, 0.14, 100.0, 100.0, 180.0);
        assert!(plop.sy < 1.0);
        assert!(plop.sx > 1.0);

        for k in 1..240 {
            h.step(0.016, 0.14 + (k as f64) * 0.016, 100.0, 100.0, 180.0);
        }
        assert!(!h.active);
    }
}
