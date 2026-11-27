use crate::audio::beat::{BeatEvent, BeatProcessor};
use crate::config::ReactPrefs;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
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

#[derive(Debug)]
pub enum LoopbackCommand {
    Ignore(f64),
    #[allow(dead_code)]
    UpdatePrefs {
        sense: f64,
        gain: f64,
        mode: String,
        offset_ms: f64,
    },
    Shutdown,
}

#[derive(Default)]
pub struct LoopbackDiag {
    pub initialized: AtomicBool,
    pub is_playing: AtomicBool,
    pub bpm_x100: AtomicU32,
    pub locked: AtomicBool,
    pub sample_rate: AtomicU32,
    pub channels: AtomicU32,
    pub beats_detected: AtomicU64,
    pub frames_processed: AtomicU64,
    pub silent_sleeps: AtomicU64,
    pub exited: AtomicBool,
}

#[derive(Clone, Debug)]
pub struct LoopbackSnapshot {
    pub initialized: bool,
    pub is_playing: bool,
    pub bpm: f64,
    pub locked: bool,
    pub sample_rate: u32,
    pub channels: u32,
    pub beats_detected: u64,
    pub frames_processed: u64,
    pub silent_sleeps: u64,
}

pub struct LoopbackHandle {
    tx_cmd: Sender<LoopbackCommand>,
    rx_beats: Receiver<BeatEvent>,
    diag: Arc<LoopbackDiag>,
    join: Option<JoinHandle<()>>,
}

impl LoopbackHandle {
    pub fn start(prefs: &ReactPrefs) -> Result<Self, String> {
        let (tx_cmd, rx_cmd) = mpsc::channel::<LoopbackCommand>();
        let (tx_beats, rx_beats) = mpsc::channel::<BeatEvent>();
        let diag = Arc::new(LoopbackDiag::default());
        let diag_bg = diag.clone();
        let initial = prefs.clone();

        let join = thread::Builder::new()
            .name("bug-pet-loopback".to_string())
            .spawn(move || {
                loopback_thread_main(&initial, rx_cmd, tx_beats, diag_bg);
            })
            .map_err(|e| format!("Failed to spawn loopback thread: {e}"))?;

        Ok(Self {
            tx_cmd,
            rx_beats,
            diag,
            join: Some(join),
        })
    }

    pub fn try_recv_beat(&self) -> Option<BeatEvent> {
        self.rx_beats.try_recv().ok()
    }

    pub fn ignore_clicks(&self, dur_s: f64) {
        let _ = self.tx_cmd.send(LoopbackCommand::Ignore(dur_s));
    }

    #[allow(dead_code)]
    pub fn update_prefs(&self, prefs: &ReactPrefs) {
        let _ = self.tx_cmd.send(LoopbackCommand::UpdatePrefs {
            sense: prefs.sense,
            gain: prefs.gain,
            mode: prefs.sound.clone(),
            offset_ms: prefs.beat_offset,
        });
    }

    pub fn is_playing(&self) -> bool {
        self.diag.is_playing.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> LoopbackSnapshot {
        LoopbackSnapshot {
            initialized: self.diag.initialized.load(Ordering::Relaxed),
            is_playing: self.diag.is_playing.load(Ordering::Relaxed),
            bpm: self.diag.bpm_x100.load(Ordering::Relaxed) as f64 / 100.0,
            locked: self.diag.locked.load(Ordering::Relaxed),
            sample_rate: self.diag.sample_rate.load(Ordering::Relaxed),
            channels: self.diag.channels.load(Ordering::Relaxed),
            beats_detected: self.diag.beats_detected.load(Ordering::Relaxed),
            frames_processed: self.diag.frames_processed.load(Ordering::Relaxed),
            silent_sleeps: self.diag.silent_sleeps.load(Ordering::Relaxed),
        }
    }

    pub fn shutdown(&mut self) {
        let _ = self.tx_cmd.send(LoopbackCommand::Shutdown);
        if let Some(j) = self.join.take() {
            let deadline = Instant::now() + Duration::from_millis(400);
            while !self.diag.exited.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if self.diag.exited.load(Ordering::Acquire) {
                let _ = j.join();
            }
            // Otherwise detach cleanly on process exit rather than blocking UI teardown.
        }
    }
}

impl Drop for LoopbackHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone, Copy)]
enum CaptureSampleFmt {
    Float32,
    Pcm16,
}

struct WasapiLoopbackStream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    sample_rate: u32,
    channels: u16,
    fmt: CaptureSampleFmt,
}

impl WasapiLoopbackStream {
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
                .map_err(|e| format!("IMMDevice::Activate: {e}"))?;

            let mix_ptr = client
                .GetMixFormat()
                .map_err(|e| format!("GetMixFormat: {e}"))?;
            let mix = std::ptr::read_unaligned(mix_ptr);
            let fmt = classify_capture_format(mix_ptr, &mix)?;

            // 200ms loopback buffer avoids packet loss across 80ms silence-gated sleeps
            let init_res = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                2_000_000,
                0,
                mix_ptr,
                None,
            );
            CoTaskMemFree(Some(mix_ptr as *const _));
            init_res.map_err(|e| format!("IAudioClient::Initialize(LOOPBACK): {e}"))?;

            let capture: IAudioCaptureClient = client
                .GetService()
                .map_err(|e| format!("GetService(IAudioCaptureClient): {e}"))?;
            client
                .Start()
                .map_err(|e| format!("IAudioClient::Start: {e}"))?;

            Ok(Self {
                client,
                capture,
                sample_rate: mix.nSamplesPerSec.max(8000),
                channels: mix.nChannels.max(1),
                fmt,
            })
        }
    }

    fn drain_mono(&self, out: &mut Vec<f32>) -> Result<(f32, bool), windows::core::Error> {
        let mut peak = 0.0f32;
        let mut had_packet = false;
        let ch = self.channels as usize;

        unsafe {
            loop {
                let pkt_len = self.capture.GetNextPacketSize()?;
                if pkt_len == 0 {
                    break;
                }
                had_packet = true;
                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut num_frames = 0u32;
                let mut flags = 0u32;
                self.capture
                    .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)?;

                let silent_flag = (flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)) != 0;
                if num_frames > 0 {
                    if silent_flag || data_ptr.is_null() {
                        out.resize(out.len() + num_frames as usize, 0.0);
                    } else {
                        match self.fmt {
                            CaptureSampleFmt::Float32 => {
                                let slice = std::slice::from_raw_parts(
                                    data_ptr as *const f32,
                                    num_frames as usize * ch,
                                );
                                for frame in slice.chunks_exact(ch) {
                                    let mut sum = 0.0f32;
                                    for &s in frame {
                                        sum += s;
                                    }
                                    let mono = sum / ch as f32;
                                    peak = peak.max(mono.abs());
                                    out.push(mono);
                                }
                            }
                            CaptureSampleFmt::Pcm16 => {
                                let slice = std::slice::from_raw_parts(
                                    data_ptr as *const i16,
                                    num_frames as usize * ch,
                                );
                                for frame in slice.chunks_exact(ch) {
                                    let mut sum = 0.0f32;
                                    for &s in frame {
                                        sum += s as f32 / 32768.0;
                                    }
                                    let mono = sum / ch as f32;
                                    peak = peak.max(mono.abs());
                                    out.push(mono);
                                }
                            }
                        }
                    }
                }
                self.capture.ReleaseBuffer(num_frames)?;
            }
        }
        Ok((peak, had_packet))
    }
}

impl Drop for WasapiLoopbackStream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

unsafe fn classify_capture_format(
    ptr: *const WAVEFORMATEX,
    mix: &WAVEFORMATEX,
) -> Result<CaptureSampleFmt, String> {
    let tag = mix.wFormatTag;
    let bits = mix.wBitsPerSample;
    let cb = mix.cbSize;
    if tag == WAVE_FORMAT_IEEE_FLOAT_TAG && bits == 32 {
        return Ok(CaptureSampleFmt::Float32);
    }
    if tag == WAVE_FORMAT_PCM_TAG && bits == 16 {
        return Ok(CaptureSampleFmt::Pcm16);
    }
    if tag == WAVE_FORMAT_EXTENSIBLE_TAG && cb >= 22 {
        let ext = std::ptr::read_unaligned(ptr as *const WAVEFORMATEXTENSIBLE);
        let sub = ext.SubFormat;
        if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT_GUID && bits == 32 {
            return Ok(CaptureSampleFmt::Float32);
        }
        if sub == KSDATAFORMAT_SUBTYPE_PCM_GUID && bits == 16 {
            return Ok(CaptureSampleFmt::Pcm16);
        }
    }
    Err(format!(
        "Unsupported WASAPI loopback mix format tag={tag} bits={bits}"
    ))
}

fn loopback_thread_main(
    initial: &ReactPrefs,
    rx_cmd: Receiver<LoopbackCommand>,
    tx_beats: Sender<BeatEvent>,
    diag: Arc<LoopbackDiag>,
) {
    struct ExitFlag(Arc<LoopbackDiag>);
    impl Drop for ExitFlag {
        fn drop(&mut self) {
            self.0.exited.store(true, Ordering::Release);
        }
    }
    let _exit_flag = ExitFlag(diag.clone());

    let com_hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let need_uninit = com_hr.is_ok();

    let mut processor = BeatProcessor::new(
        initial.sense,
        initial.gain,
        &initial.sound,
        initial.beat_offset,
    );
    let mut stream: Option<WasapiLoopbackStream> = None;
    let mut ring = [0.0f32; 2048];
    let mut staged: Vec<f32> = Vec::with_capacity(4096);
    let start = Instant::now();
    let mut silent_since: Option<Instant> = Some(start);
    let mut next_open_retry = start;
    let mut logged_open_err = false;

    loop {
        let now = Instant::now();
        let now_s = (now - start).as_secs_f64();

        while let Ok(cmd) = rx_cmd.try_recv() {
            match cmd {
                LoopbackCommand::Shutdown => {
                    drop(stream);
                    if need_uninit {
                        unsafe {
                            CoUninitialize();
                        }
                    }
                    return;
                }
                LoopbackCommand::Ignore(dur_s) => {
                    processor.ignore_for(now_s, dur_s);
                }
                LoopbackCommand::UpdatePrefs {
                    sense,
                    gain,
                    mode,
                    offset_ms,
                } => {
                    processor.update_prefs(sense, gain, &mode, offset_ms);
                }
            }
        }

        if stream.is_none() && now >= next_open_retry {
            match WasapiLoopbackStream::open() {
                Ok(s) => {
                    diag.initialized.store(true, Ordering::Relaxed);
                    diag.sample_rate.store(s.sample_rate, Ordering::Relaxed);
                    diag.channels.store(s.channels as u32, Ordering::Relaxed);
                    stream = Some(s);
                    logged_open_err = false;
                }
                Err(e) => {
                    if !logged_open_err {
                        eprintln!("[bug-pet-rs] WASAPI loopback open warning: {e}");
                        logged_open_err = true;
                    }
                    diag.initialized.store(false, Ordering::Relaxed);
                    next_open_retry = now + Duration::from_millis(500);
                }
            }
        }

        let mut is_silent_now = true;
        if let Some(ref s) = stream {
            staged.clear();
            match s.drain_mono(&mut staged) {
                Ok((peak, had_packet)) => {
                    let sr = s.sample_rate as f64;
                    let hop = ((sr * 0.016).round() as usize).clamp(256, 2048);

                    if had_packet && peak >= 1e-5 {
                        is_silent_now = false;
                        silent_since = None;

                        // Feed newly captured samples through 2048-sample analysis window
                        let mut offset = 0usize;
                        while offset < staged.len() {
                            let take = (staged.len() - offset).min(hop);
                            ring.copy_within(take.., 0);
                            ring[(2048 - take)..].copy_from_slice(&staged[offset..(offset + take)]);
                            offset += take;

                            if take >= hop / 2 || offset == staged.len() {
                                diag.frames_processed.fetch_add(1, Ordering::Relaxed);
                                if let Some(ev) = processor.process_frame(&ring, take, sr, now_s) {
                                    diag.beats_detected.fetch_add(1, Ordering::Relaxed);
                                    let _ = tx_beats.send(ev);
                                }
                            }
                        }
                    } else {
                        // Windows stops delivering loopback packets or flags SILENT when idle:
                        // decay playing/detector cleanly using zeroed buffer without hammering CPU.
                        if silent_since.is_none() {
                            silent_since = Some(now);
                        }
                        let feed = staged.len().min(hop).max(hop);
                        ring.fill(0.0);
                        diag.frames_processed.fetch_add(1, Ordering::Relaxed);
                        if let Some(ev) = processor.process_frame(&ring, feed, sr, now_s) {
                            diag.beats_detected.fetch_add(1, Ordering::Relaxed);
                            let _ = tx_beats.send(ev);
                        }
                    }
                }
                Err(err) => {
                    if err.code() == AUDCLNT_E_DEVICE_INVALIDATED {
                        stream = None;
                        diag.initialized.store(false, Ordering::Relaxed);
                        next_open_retry = now + Duration::from_millis(500);
                    }
                }
            }
        }

        diag.is_playing
            .store(processor.is_playing(), Ordering::Relaxed);
        diag.locked.store(processor.locked(), Ordering::Relaxed);
        diag.bpm_x100
            .store((processor.bpm() * 100.0).round() as u32, Ordering::Relaxed);

        let deep_silence = is_silent_now
            && silent_since.is_some_and(|t0| now.duration_since(t0) >= Duration::from_millis(500));
        if deep_silence {
            diag.silent_sleeps.fetch_add(1, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(80));
        } else {
            thread::sleep(Duration::from_millis(16));
        }
    }
}
