use rand::Rng;
use std::collections::VecDeque;
use std::f64::consts::TAU;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

const WAVE_FORMAT_PCM_TAG: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT_TAG: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE_TAG: u16 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT_GUID: GUID = GUID::from_values(
    0x00000003,
    0x0000,
    0x0010,
    [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
);
const KSDATAFORMAT_SUBTYPE_PCM_GUID: GUID = GUID::from_values(
    0x00000001,
    0x0000,
    0x0010,
    [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
);

#[derive(Clone, Copy, Debug)]
pub enum WaveShape {
    Sine,
    Square,
    Triangle,
}

#[derive(Clone, Debug)]
struct OscVoice {
    shape: WaveShape,
    f0: f64,
    f1: Option<f64>,
    dur: f64,
    gain: f64,
    attack: f64,
    delay_samples: usize,
    elapsed_samples: usize,
    phase: f64,
}

impl OscVoice {
    fn total_samples(&self, sr: f64) -> usize {
        self.delay_samples + ((self.dur + 0.02) * sr).ceil().max(1.0) as usize
    }

    fn is_done(&self, sr: f64) -> bool {
        self.elapsed_samples >= self.total_samples(sr)
    }

    fn next_sample(&mut self, sr: f64) -> f64 {
        let idx = self.elapsed_samples;
        self.elapsed_samples += 1;
        if idx < self.delay_samples {
            return 0.0;
        }
        let local_idx = idx - self.delay_samples;
        let t = local_idx as f64 / sr;
        let stop_dur = self.dur + 0.02;
        if t >= stop_dur {
            return 0.0;
        }

        let freq = match self.f1 {
            Some(f1) if f1 > 0.0 && self.f0 > 0.0 => {
                let frac = (t / self.dur.max(1e-5)).clamp(0.0, 1.0);
                self.f0 * (f1 / self.f0).powf(frac)
            }
            _ => self.f0,
        };

        let wave = match self.shape {
            WaveShape::Sine => (TAU * self.phase).sin(),
            WaveShape::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            WaveShape::Triangle => {
                let p = (self.phase + 0.25).fract();
                1.0 - 4.0 * (p - 0.5).abs()
            }
        };

        self.phase = (self.phase + freq / sr).fract();

        let peak = self.gain.max(2e-4);
        let attack = self.attack.max(1e-4).min(self.dur * 0.9);
        let env = if t < attack {
            1e-4 * (peak / 1e-4).powf(t / attack)
        } else if t < self.dur {
            let decay_d = (self.dur - attack).max(1e-4);
            peak * (1e-4 / peak).powf((t - attack) / decay_d)
        } else {
            let tail = ((stop_dur - t) / 0.02).clamp(0.0, 1.0);
            1e-4 * tail
        };

        wave * env
    }
}

#[derive(Clone, Debug)]
struct NoiseVoice {
    samples: Vec<f32>,
    cursor: usize,
    delay_samples: usize,
}

impl NoiseVoice {
    fn new_highpass(sr: f64, dur: f64, freq: f64, q: f64, gain: f64, delay_samples: usize) -> Self {
        let n = (sr * dur).floor().max(1.0) as usize;
        let mut rng = rand::thread_rng();

        let w0 = TAU * (freq.clamp(20.0, sr * 0.45)) / sr;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        let b0 = ((1.0 + cos_w0) * 0.5) / a0;
        let b1 = (-(1.0 + cos_w0)) / a0;
        let b2 = ((1.0 + cos_w0) * 0.5) / a0;
        let a1 = (-2.0 * cos_w0) / a0;
        let a2 = (1.0 - alpha) / a0;

        let mut x1 = 0.0;
        let mut x2 = 0.0;
        let mut y1 = 0.0;
        let mut y2 = 0.0;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let white = (2.0 * rng.gen::<f64>() - 1.0) * (1.0 - (i as f64) / (n as f64));
            let y = b0 * white + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = white;
            y2 = y1;
            y1 = y;
            out.push((y * gain) as f32);
        }
        Self {
            samples: out,
            cursor: 0,
            delay_samples,
        }
    }

    fn is_done(&self) -> bool {
        self.cursor >= self.delay_samples + self.samples.len()
    }

    fn next_sample(&mut self) -> f64 {
        let idx = self.cursor;
        self.cursor += 1;
        if idx < self.delay_samples {
            0.0
        } else {
            self.samples
                .get(idx - self.delay_samples)
                .copied()
                .unwrap_or(0.0) as f64
        }
    }
}

#[derive(Clone, Debug)]
enum SynthCommand {
    Click {
        kind: String,
        is_up: bool,
        vol: f64,
        jitter: f64,
    },
    #[allow(dead_code)]
    Alert {
        kind: String,
        vol: f64,
    },
    Shutdown,
}

#[derive(Default)]
pub struct SynthMixer {
    oscs: Vec<OscVoice>,
    noises: Vec<NoiseVoice>,
}

impl SynthMixer {
    pub fn is_active(&self) -> bool {
        !self.oscs.is_empty() || !self.noises.is_empty()
    }

    fn push_osc(
        &mut self,
        sr: f64,
        delay_s: f64,
        shape: WaveShape,
        f0: f64,
        f1: Option<f64>,
        dur: f64,
        gain: f64,
        attack: f64,
    ) {
        self.oscs.push(OscVoice {
            shape,
            f0,
            f1,
            dur,
            gain,
            attack,
            delay_samples: (delay_s.max(0.0) * sr).round() as usize,
            elapsed_samples: 0,
            phase: 0.0,
        });
    }

    /// Exact port of `SoundKit.play` (`sounds.js:18-69`).
    pub fn enqueue_click(&mut self, sr: f64, kind: &str, is_up: bool, vol: f64, jitter: f64) {
        if vol <= 0.0 {
            return;
        }
        let mut rng = rand::thread_rng();
        let s = 1.0 + (2.0 * rng.gen::<f64>() - 1.0) * jitter / 100.0;
        let l = vol * if is_up { 0.65 } else { 1.0 };

        match kind {
            "tock" => {
                self.push_osc(
                    sr,
                    0.0,
                    WaveShape::Sine,
                    (if is_up { 620.0 } else { 900.0 }) * s,
                    Some((if is_up { 420.0 } else { 520.0 }) * s),
                    0.06,
                    0.5 * l,
                    0.002,
                );
            }
            "pop" => {
                if is_up {
                    self.push_osc(
                        sr,
                        0.0,
                        WaveShape::Sine,
                        900.0 * s,
                        Some(500.0 * s),
                        0.03,
                        0.18 * l,
                        0.002,
                    );
                } else {
                    self.push_osc(
                        sr,
                        0.0,
                        WaveShape::Sine,
                        380.0 * s,
                        Some(1300.0 * s),
                        0.045,
                        0.45 * l,
                        0.002,
                    );
                }
            }
            "beep" => {
                if !is_up {
                    self.push_osc(
                        sr,
                        0.0,
                        WaveShape::Triangle,
                        1320.0 * s,
                        None,
                        0.06,
                        0.32 * l,
                        0.004,
                    );
                }
            }
            "wood" => {
                self.push_osc(
                    sr,
                    0.0,
                    WaveShape::Sine,
                    (if is_up { 760.0 } else { 980.0 }) * s,
                    Some((if is_up { 700.0 } else { 880.0 }) * s),
                    if is_up { 0.05 } else { 0.09 },
                    0.55 * l,
                    0.002,
                );
                self.push_osc(
                    sr,
                    0.0,
                    WaveShape::Triangle,
                    (if is_up { 1900.0 } else { 2450.0 }) * s,
                    None,
                    0.03,
                    0.12 * l,
                    0.002,
                );
            }
            "type" => {
                self.noises.push(NoiseVoice::new_highpass(
                    sr,
                    if is_up { 0.018 } else { 0.03 },
                    if is_up { 3500.0 } else { 2200.0 },
                    0.8,
                    (if is_up { 0.35 } else { 0.7 }) * l,
                    0,
                ));
                if !is_up {
                    self.push_osc(
                        sr,
                        0.0,
                        WaveShape::Square,
                        180.0 * s,
                        None,
                        0.02,
                        0.08 * l,
                        0.002,
                    );
                }
            }
            "bell" => {
                if !is_up {
                    self.push_osc(sr, 0.0, WaveShape::Sine, 2093.0 * s, None, 0.45, 0.22 * l, 0.003);
                    self.push_osc(sr, 0.0, WaveShape::Sine, 3136.0 * s, None, 0.3, 0.1 * l, 0.003);
                }
            }
            "blip" => {
                if !is_up {
                    self.push_osc(
                        sr,
                        0.0,
                        WaveShape::Square,
                        300.0 * s,
                        Some(980.0 * s),
                        0.07,
                        0.14 * l,
                        0.002,
                    );
                }
            }
            "soft" => {
                self.push_osc(
                    sr,
                    0.0,
                    WaveShape::Sine,
                    (if is_up { 180.0 } else { 230.0 }) * s,
                    Some((if is_up { 130.0 } else { 140.0 }) * s),
                    0.09,
                    0.7 * l,
                    0.006,
                );
            }
            _ => {
                let f0 = (if is_up { 1150.0 } else { 1750.0 }) * s;
                self.push_osc(
                    sr,
                    0.0,
                    WaveShape::Square,
                    f0,
                    Some(0.55 * f0),
                    if is_up { 0.028 } else { 0.035 },
                    (if is_up { 0.11 } else { 0.16 }) * vol,
                    0.0005,
                );
            }
        }
    }

    /// Exact port of `SoundKit.alert` (`sounds.js:84-112`).
    pub fn enqueue_alert(&mut self, sr: f64, kind: &str, vol: f64) {
        if vol <= 0.0 {
            return;
        }
        let u = vol.min(1.0);
        let mut two = |delay_s: f64, f: f64, dur: f64, gain: f64, shape: WaveShape| {
            self.push_osc(sr, delay_s, shape, f, None, dur, gain * u, 0.006);
            self.push_osc(
                sr,
                delay_s,
                WaveShape::Sine,
                2.0 * f,
                None,
                0.6 * dur,
                0.18 * gain * u,
                0.006,
            );
        };

        match kind {
            "bell" => {
                for d in [0.0, 0.42] {
                    self.push_osc(sr, d, WaveShape::Sine, 1568.0, None, 1.1, 0.26 * u, 0.003);
                    self.push_osc(sr, d, WaveShape::Sine, 3951.0, None, 0.6, 0.07 * u, 0.003);
                }
            }
            "marimba" => {
                for (d, f) in [(0.0, 523.0), (0.14, 659.0), (0.28, 784.0)] {
                    two(d, f, 0.32, 0.42, WaveShape::Triangle);
                }
            }
            "soft" => {
                for (d, f) in [(0.0, 440.0), (0.32, 554.0), (0.64, 659.0)] {
                    self.push_osc(sr, d, WaveShape::Sine, f, None, 0.75, 0.3 * u, 0.09);
                }
            }
            _ => {
                two(0.0, 1319.0, 0.7, 0.32, WaveShape::Sine);
                two(0.34, 1047.0, 0.95, 0.32, WaveShape::Sine);
            }
        }
    }

    pub fn render_samples(&mut self, sr: f64, out: &mut [f32]) {
        for sample in out.iter_mut() {
            let mut sum = 0.0f64;
            for osc in &mut self.oscs {
                sum += osc.next_sample(sr);
            }
            for noise in &mut self.noises {
                sum += noise.next_sample();
            }
            *sample = sum.clamp(-0.98, 0.98) as f32;
        }
        self.oscs.retain(|v| !v.is_done(sr));
        self.noises.retain(|v| !v.is_done());
    }
}

struct SynthShared {
    queue: VecDeque<SynthCommand>,
    shutdown: bool,
}

pub struct SoundEngine {
    shared: Arc<(Mutex<SynthShared>, Condvar)>,
    join: Option<JoinHandle<()>>,
}

impl SoundEngine {
    pub fn start() -> Self {
        let shared = Arc::new((
            Mutex::new(SynthShared {
                queue: VecDeque::new(),
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let shared_bg = shared.clone();
        let join = thread::Builder::new()
            .name("bug-pet-synth".to_string())
            .spawn(move || synth_thread_main(shared_bg))
            .ok();
        Self { shared, join }
    }

    pub fn play_click(&self, kind: &str, is_up: bool, vol: f64, jitter: f64) {
        if vol <= 0.0 {
            return;
        }
        let (lock, cv) = &*self.shared;
        if let Ok(mut st) = lock.lock() {
            if st.shutdown {
                return;
            }
            st.queue.push_back(SynthCommand::Click {
                kind: kind.to_string(),
                is_up,
                vol,
                jitter,
            });
            cv.notify_one();
        }
    }

    #[allow(dead_code)]
    pub fn play_alert(&self, kind: &str, vol: f64) {
        if vol <= 0.0 {
            return;
        }
        let (lock, cv) = &*self.shared;
        if let Ok(mut st) = lock.lock() {
            if st.shutdown {
                return;
            }
            st.queue.push_back(SynthCommand::Alert {
                kind: kind.to_string(),
                vol,
            });
            cv.notify_one();
        }
    }

    pub fn shutdown(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            if let Ok(mut st) = lock.lock() {
                st.shutdown = true;
                st.queue.push_back(SynthCommand::Shutdown);
                cv.notify_one();
            }
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for SoundEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone, Copy)]
enum RenderSampleFmt {
    Float32,
    Pcm16,
}

struct WasapiRenderStream {
    client: IAudioClient,
    render: IAudioRenderClient,
    sample_rate: u32,
    channels: u16,
    buffer_frames: u32,
    fmt: RenderSampleFmt,
    started: bool,
}

impl WasapiRenderStream {
    fn open() -> Result<Self, String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("CoCreateInstance(MMDeviceEnumerator): {e}"))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("GetDefaultAudioEndpoint: {e}"))?;
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("IMMDevice::Activate(IAudioClient): {e}"))?;

            let mix_ptr = client
                .GetMixFormat()
                .map_err(|e| format!("GetMixFormat: {e}"))?;
            let mix = std::ptr::read_unaligned(mix_ptr);
            let fmt = classify_render_format(mix_ptr, &mix)?;

            // 100ms shared-mode buffer
            let init_res = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, mix_ptr, None);
            CoTaskMemFree(Some(mix_ptr as *const _));
            init_res.map_err(|e| format!("IAudioClient::Initialize: {e}"))?;

            let buffer_frames = client
                .GetBufferSize()
                .map_err(|e| format!("GetBufferSize: {e}"))?;
            let render: IAudioRenderClient = client
                .GetService()
                .map_err(|e| format!("GetService(IAudioRenderClient): {e}"))?;

            Ok(Self {
                client,
                render,
                sample_rate: mix.nSamplesPerSec.max(8000),
                channels: mix.nChannels.max(1),
                buffer_frames,
                fmt,
                started: false,
            })
        }
    }

    fn set_running(&mut self, run: bool) {
        if run && !self.started {
            if unsafe { self.client.Start() }.is_ok() {
                self.started = true;
            }
        } else if !run && self.started {
            unsafe {
                let _ = self.client.Stop();
                let _ = self.client.Reset();
            }
            self.started = false;
        }
    }

    fn write_chunk(&mut self, mixer: &mut SynthMixer) -> Result<bool, windows::core::Error> {
        unsafe {
            let padding = self.client.GetCurrentPadding()?;
            let avail = self.buffer_frames.saturating_sub(padding);
            let want = ((self.sample_rate / 40).max(256)).min(avail);
            if want == 0 {
                return Ok(false);
            }

            let ptr = self.render.GetBuffer(want)?;
            let mut mono = vec![0.0f32; want as usize];
            mixer.render_samples(self.sample_rate as f64, &mut mono);

            let ch = self.channels as usize;
            match self.fmt {
                RenderSampleFmt::Float32 => {
                    let slice = std::slice::from_raw_parts_mut(ptr as *mut f32, want as usize * ch);
                    for (i, &m) in mono.iter().enumerate() {
                        let base = i * ch;
                        for c in 0..ch {
                            slice[base + c] = m;
                        }
                    }
                }
                RenderSampleFmt::Pcm16 => {
                    let slice = std::slice::from_raw_parts_mut(ptr as *mut i16, want as usize * ch);
                    for (i, &m) in mono.iter().enumerate() {
                        let v = (m * 32767.0).clamp(-32768.0, 32767.0) as i16;
                        let base = i * ch;
                        for c in 0..ch {
                            slice[base + c] = v;
                        }
                    }
                }
            }
            self.render.ReleaseBuffer(want, 0)?;
            Ok(true)
        }
    }
}

unsafe fn classify_render_format(
    ptr: *const WAVEFORMATEX,
    mix: &WAVEFORMATEX,
) -> Result<RenderSampleFmt, String> {
    let tag = mix.wFormatTag;
    let bits = mix.wBitsPerSample;
    let cb = mix.cbSize;
    if tag == WAVE_FORMAT_IEEE_FLOAT_TAG && bits == 32 {
        return Ok(RenderSampleFmt::Float32);
    }
    if tag == WAVE_FORMAT_PCM_TAG && bits == 16 {
        return Ok(RenderSampleFmt::Pcm16);
    }
    if tag == WAVE_FORMAT_EXTENSIBLE_TAG && cb >= 22 {
        let ext = std::ptr::read_unaligned(ptr as *const WAVEFORMATEXTENSIBLE);
        let sub = ext.SubFormat;
        if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT_GUID && bits == 32 {
            return Ok(RenderSampleFmt::Float32);
        }
        if sub == KSDATAFORMAT_SUBTYPE_PCM_GUID && bits == 16 {
            return Ok(RenderSampleFmt::Pcm16);
        }
    }
    Err(format!(
        "Unsupported WASAPI render mix format tag={tag} bits={bits}"
    ))
}

fn synth_thread_main(shared: Arc<(Mutex<SynthShared>, Condvar)>) {
    let com_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let need_uninit = com_hr.is_ok();

    let mut mixer = SynthMixer::default();
    let mut stream: Option<WasapiRenderStream> = None;
    let mut silent_tail_chunks = 0u32;

    loop {
        let mut should_exit = false;
        let mut drained_cmds: Vec<SynthCommand> = Vec::new();
        {
            let (lock, cv) = &*shared;
            let mut guard = match lock.lock() {
                Ok(g) => g,
                Err(_) => break,
            };

            if !mixer.is_active() && silent_tail_chunks == 0 && guard.queue.is_empty() && !guard.shutdown {
                if let Some(ref mut st) = stream {
                    st.set_running(false);
                }
                guard = match cv.wait(guard) {
                    Ok(g) => g,
                    Err(_) => break,
                };
            }

            while let Some(cmd) = guard.queue.pop_front() {
                match cmd {
                    SynthCommand::Shutdown => should_exit = true,
                    other => drained_cmds.push(other),
                }
            }
            if guard.shutdown {
                should_exit = true;
            }
        }

        if should_exit {
            break;
        }

        if !drained_cmds.is_empty() && stream.is_none() {
            stream = WasapiRenderStream::open().ok();
        }

        let sr = stream
            .as_ref()
            .map(|s| s.sample_rate as f64)
            .unwrap_or(48000.0);

        for cmd in drained_cmds {
            match cmd {
                SynthCommand::Click {
                    kind,
                    is_up,
                    vol,
                    jitter,
                } => {
                    mixer.enqueue_click(sr, &kind, is_up, vol, jitter);
                    silent_tail_chunks = 2;
                }
                SynthCommand::Alert { kind, vol } => {
                    mixer.enqueue_alert(sr, &kind, vol);
                    silent_tail_chunks = 2;
                }
                SynthCommand::Shutdown => {}
            }
        }

        if mixer.is_active() || silent_tail_chunks > 0 {
            if !mixer.is_active() {
                silent_tail_chunks = silent_tail_chunks.saturating_sub(1);
            }
            if let Some(ref mut st) = stream {
                if let Err(_e) = st.write_chunk(&mut mixer) {
                    stream = WasapiRenderStream::open().ok();
                } else {
                    st.set_running(true);
                }
            } else {
                // Drop voices if no output device is available
                let mut discard = [0.0f32; 512];
                mixer.render_samples(sr, &mut discard);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    if let Some(ref mut st) = stream {
        st.set_running(false);
    }
    drop(stream);
    if need_uninit {
        unsafe {
            CoUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_presets_produce_finite_nonempty_waveforms_and_expire() {
        let presets = [
            "click", "tock", "pop", "beep", "wood", "type", "bell", "blip", "soft",
        ];
        for p in presets {
            let mut m = SynthMixer::default();
            m.enqueue_click(48000.0, p, false, 0.5, 0.0);
            assert!(m.is_active(), "preset {p} should activate");
            let mut buf = [0.0f32; 4096];
            m.render_samples(48000.0, &mut buf);
            let peak = buf.iter().fold(0.0f32, |acc, &x| {
                assert!(x.is_finite());
                acc.max(x.abs())
            });
            assert!(peak > 0.005, "preset {p} produced near-silent output ({peak})");

            // Drain up to 1s until finished
            let mut silence = [0.0f32; 4800];
            for _ in 0..10 {
                m.render_samples(48000.0, &mut silence);
            }
            assert!(!m.is_active(), "preset {p} should finish within 1s");
        }

        let mut alert_m = SynthMixer::default();
        alert_m.enqueue_alert(48000.0, "chime", 0.6);
        assert!(alert_m.is_active());
        let mut buf = [0.0f32; 4096];
        alert_m.render_samples(48000.0, &mut buf);
        let peak = buf.iter().fold(0.0f32, |acc, &x| acc.max(x.abs()));
        assert!(peak > 0.01);
    }
}
