use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct BeatEvent {
    pub strength: f64,
    pub interval_s: f64,
    #[allow(dead_code)]
    pub is_game: bool,
}

#[inline]
pub fn rms_of(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for &x in samples {
        let xd = x as f64;
        sum += xd * xd;
    }
    (sum / samples.len() as f64).sqrt()
}

#[inline]
pub fn band_energy(bytes: &[u8], bw: f64, lo: f64, hi: f64) -> f64 {
    if bw <= 0.0 || bytes.is_empty() {
        return 0.0;
    }
    let s = ((lo / bw).floor() as usize).max(1);
    let h = ((hi / bw).ceil() as usize).max(s + 1);
    let mut sum = 0.0f64;
    let mut cnt = 0usize;
    let end = h.min(bytes.len().saturating_sub(1));
    if s <= end {
        for &b in &bytes[s..=end] {
            sum += b as f64;
            cnt += 1;
        }
    }
    if cnt > 0 {
        sum / cnt as f64
    } else {
        0.0
    }
}

#[inline]
pub fn low_energy(bytes: &[u8], bw: f64) -> f64 {
    band_energy(bytes, bw, 40.0, 160.0)
}

#[inline]
fn median(vals: &VecDeque<f64>) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = vals.iter().copied().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[m]
    } else {
        0.5 * (sorted[m - 1] + sorted[m])
    }
}

#[derive(Clone, Copy, Debug)]
struct SenseCfg {
    k: f64,
    floor: f64,
    rel: f64,
}

impl SenseCfg {
    fn from_sense(sense: f64) -> Self {
        let s = if sense.is_finite() {
            sense.clamp(1.0, 10.0)
        } else {
            5.0
        };
        let h = (s - 1.0) / 9.0;
        let lerp = |a: f64, b: f64| a + (b - a) * h;
        Self {
            k: lerp(2.4, 0.95),
            floor: lerp(7.0, 2.0),
            rel: lerp(0.62, 0.24),
        }
    }
}

const MIN_GAP: f64 = 0.26;
const HIST_WIN: f64 = 1.4;
const LEVEL_THR: f64 = 0.006;

#[derive(Clone, Debug)]
pub struct OnsetInfo {
    pub strength: f64,
    pub interval: f64,
}

pub struct Detector {
    cfg: SenseCfg,
    prev: f64,
    hist: VecDeque<(f64, f64)>,
    last_beat: f64,
    gaps: VecDeque<f64>,
    loud_since: f64,
    quiet_since: f64,
    pub playing: bool,
    pub level: f64,
    avg_e: f64,
    pub last_flux: f64,
}

impl Detector {
    pub fn new(sense: f64) -> Self {
        Self {
            cfg: SenseCfg::from_sense(sense),
            prev: 0.0,
            hist: VecDeque::with_capacity(128),
            last_beat: -1.0,
            gaps: VecDeque::with_capacity(8),
            loud_since: -1.0,
            quiet_since: 0.0,
            playing: false,
            level: 0.0,
            avg_e: 0.0,
            last_flux: 0.0,
        }
    }

    pub fn set_sense(&mut self, sense: f64) {
        self.cfg = SenseCfg::from_sense(sense);
    }

    pub fn step(&mut self, t: f64, e: f64, rms: f64) -> Option<OnsetInfo> {
        self.level = 0.85 * self.level + 0.15 * rms;
        if self.level > LEVEL_THR {
            if self.loud_since < 0.0 {
                self.loud_since = t;
            }
            self.quiet_since = -1.0;
            if !self.playing && (t - self.loud_since) > 0.5 {
                self.playing = true;
            }
        } else {
            self.loud_since = -1.0;
            if self.quiet_since < 0.0 {
                self.quiet_since = t;
            }
            if self.playing && (t - self.quiet_since) > 3.0 {
                self.playing = false;
                self.gaps.clear();
            }
        }

        self.avg_e = if self.avg_e != 0.0 {
            0.97 * self.avg_e + 0.03 * e
        } else {
            e
        };
        let flux = (e - self.prev).max(0.0);
        self.last_flux = flux;
        self.prev = e;
        self.hist.push_back((t, flux));
        while self.hist.front().is_some_and(|&(ht, _)| t - ht > HIST_WIN) {
            self.hist.pop_front();
        }
        if self.hist.len() < 8 {
            return None;
        }

        let n = self.hist.len() as f64;
        let mean = self.hist.iter().map(|&(_, f)| f).sum::<f64>() / n;
        let var = self
            .hist
            .iter()
            .map(|&(_, f)| {
                let d = f - mean;
                d * d
            })
            .sum::<f64>()
            / n;
        let sd = var.sqrt();
        let thr = mean + self.cfg.k * sd;

        if !self.playing {
            return None;
        }
        if flux < self.cfg.floor || flux <= thr || flux < self.cfg.rel * self.avg_e {
            return None;
        }
        if self.last_beat >= 0.0 && (t - self.last_beat) < MIN_GAP {
            return None;
        }
        if self.last_beat >= 0.0 {
            let s = t - self.last_beat;
            if (MIN_GAP..=1.3).contains(&s) {
                if self.gaps.len() >= 8 {
                    self.gaps.pop_front();
                }
                self.gaps.push_back(s);
            }
        }
        self.last_beat = t;

        let strength = ((flux - mean) / (3.0 * sd + 1e-6)).clamp(0.15, 1.0);
        let med = median(&self.gaps);
        let interval = if med != 0.0 { med } else { 0.5 };
        Some(OnsetInfo { strength, interval })
    }
}

const STEP_A: f64 = 0.01;
const ENV_WIN: f64 = 8.0;
const LAG_MIN_S: f64 = 0.33;
const LAG_MAX_S: f64 = 0.86;

pub struct Tracker {
    pub det: Detector,
    env: VecDeque<(f64, f64)>,
    pub period: f64,
    conf: f64,
    pub next: f64,
    hits: VecDeque<bool>,
    last_tempo: f64,
    cand: f64,
    count: u64,
    last_onset: f64,
    lk: bool,
    bad_since: f64,
    mem_period: f64,
    mem_at: f64,
    sense: f64,
}

impl Tracker {
    pub fn new(sense: f64) -> Self {
        let s = if sense.is_finite() {
            sense.clamp(1.0, 10.0)
        } else {
            5.0
        };
        Self {
            det: Detector::new(s),
            env: VecDeque::with_capacity(600),
            period: 0.0,
            conf: 0.0,
            next: 0.0,
            hits: VecDeque::with_capacity(8),
            last_tempo: -1.0,
            cand: 0.0,
            count: 0,
            last_onset: -1.0,
            lk: false,
            bad_since: -1.0,
            mem_period: 0.0,
            mem_at: -99.0,
            sense: s,
        }
    }

    pub fn set_sense(&mut self, sense: f64) {
        let s = if sense.is_finite() {
            sense.clamp(1.0, 10.0)
        } else {
            5.0
        };
        self.sense = s;
        self.det.set_sense(s);
    }

    pub fn gain(&self) -> f64 {
        0.6 + 0.08 * self.sense
    }

    pub fn lock_at(&self) -> f64 {
        1.75 - 0.05 * self.sense
    }

    pub fn reset(&mut self, t: f64) {
        if self.period > 0.0 {
            self.mem_period = self.period;
            self.mem_at = t;
        }
        self.period = 0.0;
        self.conf = 0.0;
        self.hits.clear();
        self.cand = 0.0;
        self.lk = false;
        self.bad_since = -1.0;
    }

    fn grid(&self, t: f64) -> (Vec<f32>, f64) {
        let bins = (ENV_WIN / STEP_A).round() as usize;
        let mut g = vec![0.0f32; bins];
        let t0 = t - ENV_WIN;
        for &(te, flux) in &self.env {
            let idx = ((te - t0) / STEP_A).floor() as isize;
            if idx >= 0 && (idx as usize) < bins {
                g[idx as usize] += flux as f32;
            }
        }
        let mean = g.iter().copied().sum::<f32>() / bins as f32;
        for x in &mut g {
            *x = (*x - mean).max(0.0);
        }
        (g, t0)
    }

    fn estimate(&mut self, t: f64) {
        let (g, t0) = self.grid(t);
        let h = g.len();
        let lag_lo = (LAG_MIN_S / STEP_A).round() as usize;
        let lag_hi = (LAG_MAX_S / STEP_A).round() as usize;

        let ac = |lag: usize| -> f64 {
            let mut sum = 0.0f64;
            for k in lag..h {
                sum += (g[k] as f64) * (g[k - lag] as f64);
            }
            sum / (h - lag) as f64
        };

        let mut best_score = 0.0f64;
        let mut best_l = 0usize;
        let mut sum_score = 0.0f64;
        let mut cnt = 0usize;

        for l in lag_lo..=lag_hi {
            let iv = l as f64 * STEP_A;
            let prior = (-0.5 * ((iv / 0.5).log2() / 0.7).powi(2)).exp();
            let base = ac(l) + 0.5 * if 2 * l < h { ac(2 * l) } else { 0.0 };
            let score = base * (0.6 + 0.4 * prior);
            sum_score += score;
            cnt += 1;
            if score > best_score {
                best_score = score;
                best_l = l;
            }
        }
        if best_l == 0 || sum_score <= 0.0 || cnt == 0 {
            return;
        }

        let g0 = ac(best_l - 1);
        let p0 = ac(best_l);
        let m0 = ac(best_l + 1);
        let denom = g0 - 2.0 * p0 + m0;
        let shift = if denom != 0.0 {
            (0.5 * (g0 - m0) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let v = (best_l as f64 + shift) * STEP_A;
        let x = best_score / (sum_score / cnt as f64);

        if self.period > 0.0 {
            if (v - self.period).abs() / self.period < 0.08 {
                self.period = 0.85 * self.period + 0.15 * v;
                self.conf = 0.7 * self.conf + 0.3 * x;
                self.cand = 0.0;
            } else if x > 1.6 {
                if self.cand > 0.0 && (self.cand - v).abs() / v < 0.08 {
                    self.lock(v, x, &g, t0);
                } else {
                    self.cand = v;
                }
            }
        } else {
            let near = self.mem_period > 0.0
                && (t - self.mem_at) < 15.0
                && (v - self.mem_period).abs() / self.mem_period < 0.08;
            if x > self.lock_at() + if near { -0.2 } else { 0.1 } {
                self.lock(v, x, &g, t0);
            }
        }
    }

    fn lock(&mut self, p: f64, conf: f64, g: &[f32], t0: f64) {
        self.period = p;
        self.conf = conf;
        self.cand = 0.0;
        self.hits.clear();
        let e = g.len() as isize;
        let n = p / STEP_A;
        let max_ph = n.ceil() as usize;
        let mut best_ph = 0usize;
        let mut best_score = -1.0f64;

        for ph in 0..max_ph {
            if (ph as f64) >= n {
                break;
            }
            let mut s = 0.0f64;
            let mut h = (e - 1 - ph as isize) as f64;
            while h >= 0.0 {
                let r = h.round() as isize;
                if r >= 0 && r < e {
                    let ru = r as usize;
                    let left = if ru > 0 { g[ru - 1] as f64 } else { 0.0 };
                    let right = if ru + 1 < g.len() { g[ru + 1] as f64 } else { 0.0 };
                    s += g[ru] as f64 + 0.5 * (left + right);
                }
                h -= n;
            }
            if s > best_score {
                best_score = s;
                best_ph = ph;
            }
        }
        let l = t0 + ((e as usize - 1 - best_ph) as f64) * STEP_A;
        self.next = l + p;
    }

    pub fn step(&mut self, t: f64, low: f64, rms: f64) -> Option<OnsetInfo> {
        let onset = self.det.step(t, low, rms);
        self.env.push_back((t, self.det.last_flux));
        while self.env.front().is_some_and(|&(et, _)| t - et > ENV_WIN) {
            self.env.pop_front();
        }
        if !self.det.playing {
            if self.period > 0.0 {
                self.reset(t);
            }
            return onset;
        }
        if (t - self.last_tempo) >= 0.25
            && self
                .env
                .front()
                .is_some_and(|&(et0, _)| t - et0 > 3.5)
        {
            self.last_tempo = t;
            self.estimate(t);
        }
        if onset.is_some() {
            self.last_onset = t;
            if self.period > 0.0 {
                let i = ((t - self.next) / self.period).round();
                let s = t - (self.next + i * self.period);
                let hit = s.abs() < 0.18 * self.period;
                let offbeat = !hit && (s.abs() - 0.5 * self.period).abs() < 0.15 * self.period;
                if hit {
                    self.next += 0.3 * s;
                }
                if !offbeat {
                    if self.hits.len() >= 8 {
                        self.hits.pop_front();
                    }
                    self.hits.push_back(hit);
                }
            }
        }
        self.lk = self.judge(t);
        if self.period > 0.0 {
            while self.next < t - 0.5 * self.period {
                self.next += self.period;
            }
        }
        onset
    }

    fn judge(&mut self, t: f64) -> bool {
        if self.period <= 0.0 {
            return false;
        }
        let n_hits = self.hits.len();
        let hr = if n_hits > 0 {
            self.hits.iter().filter(|&&x| x).count() as f64 / n_hits as f64
        } else {
            1.0
        };
        if !self.lk {
            return self.conf >= self.lock_at() && (n_hits < 3 || hr >= 0.5);
        }
        if self.conf < self.lock_at() - 0.3 || (n_hits >= 4 && hr < 0.3) {
            if self.bad_since < 0.0 {
                self.bad_since = t;
            }
            (t - self.bad_since) < 2.0
        } else {
            self.bad_since = -1.0;
            true
        }
    }

    pub fn locked(&self) -> bool {
        self.lk
    }

    pub fn due(&mut self, t: f64, latency: f64) -> Option<BeatEvent> {
        if !self.locked() {
            return None;
        }
        if self.last_onset < 0.0 || (t - self.last_onset) > (5.0 * self.period).max(2.5) {
            return None;
        }
        let half = self.period < 0.42;
        let interval = if half { 2.0 * self.period } else { self.period };
        let e = latency + 0.3 * (0.5 * interval).clamp(0.14, 0.32);
        if t < self.next - e {
            return None;
        }
        self.next += self.period;
        self.count += 1;
        if half && self.count % 2 == 1 {
            return None;
        }
        Some(BeatEvent {
            strength: (if self.count % 4 == 0 { 1.0 } else { 0.8 }) * self.gain(),
            interval_s: interval,
            is_game: false,
        })
    }
}

pub struct AutoGain {
    pub gain_now: f64,
    pub want: f64,
    peaks: VecDeque<(f64, f64)>,
}

impl AutoGain {
    pub fn new(want: f64) -> Self {
        let clamped = if want > 0.0 {
            want.round().clamp(1.0, 30.0)
        } else {
            0.0
        };
        Self {
            gain_now: if clamped > 0.0 { clamped } else { 1.0 },
            want: clamped,
            peaks: VecDeque::with_capacity(260),
        }
    }

    pub fn set_want(&mut self, want: f64) {
        let w = want.round().clamp(0.0, 30.0);
        self.want = w;
        if w > 0.0 {
            self.gain_now = w;
            self.peaks.clear();
        } else {
            self.peaks.clear();
        }
    }

    pub fn step(&mut self, now_s: f64, post_gain_rms: f64) {
        if self.want > 0.0 {
            self.gain_now = self.want;
            return;
        }
        let raw_rms = post_gain_rms / self.gain_now.max(1e-6);
        self.peaks.push_back((now_s, raw_rms));
        while self.peaks.front().is_some_and(|&(t, _)| now_s - t > 4.0) {
            self.peaks.pop_front();
        }
        let peak = self
            .peaks
            .iter()
            .map(|&(_, v)| v)
            .fold(0.0f64, f64::max);
        if peak < 6e-4 {
            return;
        }
        let target = (0.06 / peak).clamp(1.0, 60.0);
        let rate = if target < self.gain_now { 0.08 } else { 0.02 };
        self.gain_now = (self.gain_now + (target - self.gain_now) * rate).clamp(1.0, 100.0);
    }
}

/// WebAudio DynamicsCompressor emulation (threshold -10 dB, knee 6, ratio 12, attack 3ms, release 120ms).
pub struct DynamicsCompressor {
    env: f32,
}

impl DynamicsCompressor {
    pub fn new() -> Self {
        Self { env: 0.0 }
    }

    pub fn process(&mut self, samples: &mut [f32], sample_rate: f32) {
        let atk = (-1.0 / (0.003 * sample_rate.max(8000.0))).exp();
        let rel = (-1.0 / (0.120 * sample_rate.max(8000.0))).exp();
        const THRESH_DB: f32 = -10.0;
        const KNEE_DB: f32 = 6.0;
        const RATIO: f32 = 12.0;

        for x in samples.iter_mut() {
            let ax = x.abs();
            let coeff = if ax > self.env { atk } else { rel };
            self.env = coeff * self.env + (1.0 - coeff) * ax;
            let level_db = 20.0 * self.env.max(1e-8).log10();

            let gain_db = if 2.0 * (level_db - THRESH_DB) < -KNEE_DB {
                0.0
            } else if 2.0 * (level_db - THRESH_DB).abs() <= KNEE_DB {
                let d = level_db - THRESH_DB + KNEE_DB * 0.5;
                ((1.0 / RATIO) - 1.0) * d * d / (2.0 * KNEE_DB)
            } else {
                (THRESH_DB + (level_db - THRESH_DB) / RATIO) - level_db
            };

            *x *= 10.0f32.powf(gain_db / 20.0);
        }
    }
}

pub struct BlackmanFft2048 {
    fft: Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex32>,
    bytes: [u8; 1024],
}

impl BlackmanFft2048 {
    pub fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(2048);
        let mut window = Vec::with_capacity(2048);
        for n in 0..2048 {
            let p = n as f32 / 2048.0;
            // Exact WebAudio AnalyserNode Blackman window (alpha = 0.16)
            let w = 0.42 - 0.5 * (TAU * p).cos() + 0.08 * (2.0 * TAU * p).cos();
            window.push(w);
        }
        Self {
            fft,
            window,
            buf: vec![Complex32::default(); 2048],
            bytes: [0u8; 1024],
        }
    }

    pub fn compute_bytes(&mut self, time_domain: &[f32; 2048]) -> &[u8; 1024] {
        for i in 0..2048 {
            self.buf[i] = Complex32::new(time_domain[i] * self.window[i], 0.0);
        }
        self.fft.process(&mut self.buf);
        for k in 0..1024 {
            let mag = self.buf[k].norm() / 2048.0;
            let db = 20.0 * mag.max(1e-20).log10();
            let byte = (((db + 100.0) / 70.0) * 255.0).floor().clamp(0.0, 255.0) as u8;
            self.bytes[k] = byte;
        }
        &self.bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenMode {
    Music,
    Game,
    Both,
}

impl ListenMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "game" => Self::Game,
            "both" => Self::Both,
            _ => Self::Music,
        }
    }
}

pub struct BeatProcessor {
    pub auto_gain: AutoGain,
    pub compressor: DynamicsCompressor,
    pub fft: BlackmanFft2048,
    pub tracker: Tracker,
    pub game_det: Detector,
    pub mode: ListenMode,
    pub offset_s: f64,
    post_ring: [f32; 2048],
    ignore_until_s: f64,
    held_e: Option<(f64, f64)>,
    last_sent_s: f64,
}

impl BeatProcessor {
    pub fn new(sense: f64, gain: f64, mode_str: &str, offset_ms: f64) -> Self {
        Self {
            auto_gain: AutoGain::new(gain),
            compressor: DynamicsCompressor::new(),
            fft: BlackmanFft2048::new(),
            tracker: Tracker::new(sense),
            game_det: Detector::new(sense),
            mode: ListenMode::parse(mode_str),
            offset_s: offset_ms / 1000.0,
            post_ring: [0.0f32; 2048],
            ignore_until_s: -1.0,
            held_e: None,
            last_sent_s: -1.0,
        }
    }

    pub fn update_prefs(&mut self, sense: f64, gain: f64, mode_str: &str, offset_ms: f64) {
        self.tracker.set_sense(sense);
        self.game_det.set_sense(sense);
        self.auto_gain.set_want(gain);
        self.mode = ListenMode::parse(mode_str);
        self.offset_s = offset_ms / 1000.0;
    }

    pub fn ignore_for(&mut self, now_s: f64, dur_s: f64) {
        self.ignore_until_s = self.ignore_until_s.max(now_s + dur_s);
    }

    pub fn is_playing(&self) -> bool {
        self.tracker.det.playing || (self.mode != ListenMode::Music && self.game_det.playing)
    }

    pub fn bpm(&self) -> f64 {
        if self.tracker.locked() && self.tracker.period > 0.0 {
            60.0 / self.tracker.period
        } else {
            0.0
        }
    }

    pub fn locked(&self) -> bool {
        self.tracker.locked()
    }

    pub fn process_frame(
        &mut self,
        raw_mono_2048: &[f32; 2048],
        new_hop_len: usize,
        sample_rate: f64,
        now_s: f64,
    ) -> Option<BeatEvent> {
        let hop = new_hop_len.min(2048);
        if hop > 0 {
            let hop_start = 2048 - hop;
            self.post_ring.copy_within(hop.., 0);
            let g = self.auto_gain.gain_now as f32;
            for i in 0..hop {
                self.post_ring[hop_start + i] = raw_mono_2048[hop_start + i] * g;
            }
            self.compressor
                .process(&mut self.post_ring[hop_start..], sample_rate as f32);
        }

        let rms = rms_of(&self.post_ring);
        self.auto_gain.step(now_s, rms);

        let bytes = self.fft.compute_bytes(&self.post_ring);
        let bw = sample_rate / 2048.0;

        let run_music = self.mode != ListenMode::Game;
        let run_game = self.mode != ListenMode::Music;

        let mut a = if run_music { low_energy(bytes, bw) } else { 0.0 };
        let mut c = if run_game {
            band_energy(bytes, bw, 80.0, 6000.0)
        } else {
            0.0
        };

        let ignored = now_s < self.ignore_until_s;
        if ignored {
            if let Some((ha, hc)) = self.held_e {
                a = a.min(ha);
                c = c.min(hc);
            }
        } else {
            self.held_e = Some((a, c));
        }

        let d = if run_music {
            self.tracker.step(now_s, a, rms)
        } else {
            None
        };
        let l = if run_game {
            self.game_det.step(now_s, c, rms)
        } else {
            None
        };

        let mut chosen: Option<BeatEvent> = None;
        if run_music {
            if let Some(due_ev) = self.tracker.due(now_s, 0.07 + self.offset_s) {
                chosen = Some(due_ev);
            } else if let Some(onset) = d {
                if !self.tracker.locked() && !ignored && self.mode != ListenMode::Both {
                    chosen = Some(BeatEvent {
                        strength: onset.strength * self.tracker.gain(),
                        interval_s: onset.interval,
                        is_game: false,
                    });
                }
            }
        }

        if chosen.is_none() && run_game && !ignored {
            if let Some(gonset) = l {
                let mut allow = true;
                if run_music && self.tracker.locked() && self.tracker.period > 0.0 {
                    let p = self.tracker.period;
                    let dist = (now_s - self.tracker.next)
                        .abs()
                        .min((now_s - (self.tracker.next - p)).abs());
                    allow = dist > 0.3 * p && gonset.strength >= 0.55;
                }
                if allow {
                    chosen = Some(BeatEvent {
                        strength: gonset.strength * self.tracker.gain(),
                        interval_s: gonset.interval,
                        is_game: true,
                    });
                }
            }
        }

        if let Some(ev) = chosen {
            if self.last_sent_s < 0.0 || (now_s - self.last_sent_s) >= 0.150 {
                self.last_sent_s = now_s;
                return Some(ev);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_locks_on_120_bpm_synthetic_kick_and_skips_silence() {
        let sr = 48000.0;
        let mut proc_bpm = BeatProcessor::new(9.0, 0.0, "music", 0.0);
        let mut buf = [0.0f32; 2048];
        let hop = 768usize; // ~16ms @ 48kHz

        let mut sample_idx = 0usize;
        let mut beats = 0usize;
        for frame in 0..500 {
            buf.copy_within(hop.., 0);
            for i in (2048 - hop)..2048 {
                let t = sample_idx as f64 / sr;
                let phase_beat = t % 0.5; // 120 BPM = 0.5s period
                let env = if phase_beat < 0.09 {
                    (-phase_beat * 35.0).exp()
                } else {
                    0.0
                };
                // 75 Hz bass kick on top of a quiet 220 Hz sustained musical bed
                let kick = 0.40 * env * (TAU as f64 * 75.0 * phase_beat).sin();
                let bed = 0.025 * (TAU as f64 * 220.0 * t).sin();
                buf[i] = (kick + bed) as f32;
                sample_idx += 1;
            }
            let now_s = frame as f64 * 0.016;
            if proc_bpm.process_frame(&buf, hop, sr, now_s).is_some() {
                beats += 1;
            }
        }

        assert!(proc_bpm.is_playing(), "kick train should trigger playing");
        assert!(beats >= 8, "expected >= 8 beats, got {beats}");
        if proc_bpm.locked() {
            let bpm = proc_bpm.bpm();
            assert!((114.0..=126.0).contains(&bpm), "expected ~120 BPM, got {bpm}");
        }

        let mut proc_sil = BeatProcessor::new(9.0, 0.0, "music", 0.0);
        let zeros = [0.0f32; 2048];
        let mut sil_beats = 0usize;
        for frame in 0..100 {
            if proc_sil
                .process_frame(&zeros, hop, sr, frame as f64 * 0.016)
                .is_some()
            {
                sil_beats += 1;
            }
        }
        assert!(!proc_sil.is_playing());
        assert_eq!(sil_beats, 0);
    }
}
