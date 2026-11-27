mod assets;
mod audio;
mod bubble;
mod config;
mod motion;
mod persist;
mod pet_fx;
mod pet_state;
mod protect;
mod radial;
mod remind;
mod renderer;
mod rig;
mod shaders;
mod tray;
mod typing;
mod window;

use crate::assets::LoadedCharacter;
use crate::audio::beat::BeatEvent;
use crate::audio::loopback::LoopbackHandle;
use crate::audio::synth::SoundEngine;
use crate::config::{discover_data_dir, AppState, BugGeometry, PetState, ReactPrefs};
use crate::persist::{load_synced_state, StatePersister};
use crate::pet_state::{CharacterGpuTextures, PetRuntime};
use crate::protect::ProtectKey;
use crate::radial::RadialAction;
use crate::remind::{RemindKind, RemindScheduler};
use crate::renderer::{GpuContext, GpuPreference};
use crate::tray::{
    is_autostart_enabled, set_autostart_enabled, show_pet_settings_menu, GlobalMenuState,
    PetMenuSnapshot, PetQuickAction, TrayAction, TrayIcon,
};
use crate::typing::TypingWatcher;
use crate::window::{
    compute_overlay_rect_px, create_pet_window, create_tray_host_window, destroy_all_windows,
    destroy_pet_window, enable_per_monitor_dpi_v2, ensure_and_position_overlay, enumerate_monitors,
    hide_overlay, move_pet_window_px, pick_monitor_for_phys_point, register_window_class,
    resize_pet_window_to_size, resolve_pet_dpr, set_pet_always_on_top, sync_click_through,
    MonitorMetrics, PendingUiAction, SharedAppEvents,
};
use chrono::Local;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::mem::size_of;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, WAIT_OBJECT_0};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimer, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
    TIMER_ALL_ACCESS,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetCursorPos, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage,
    MSG, MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
};

struct CliArgs {
    data_dir: Option<String>,
    smoke_ms: Option<u64>,
    gpu_pref: GpuPreference,
    test_beat_ms: Option<u64>,
    persist: bool,
    read_only: bool,
    bcp_key: Option<String>,
}

fn parse_args() -> CliArgs {
    let mut args = std::env::args().skip(1);
    let mut out = CliArgs {
        data_dir: None,
        smoke_ms: None,
        gpu_pref: GpuPreference::Auto,
        test_beat_ms: None,
        persist: false,
        read_only: false,
        bcp_key: None,
    };
    while let Some(a) = args.next() {
        if a == "--data-dir" {
            out.data_dir = args.next();
        } else if a == "--smoke-ms" {
            if let Some(v) = args.next() {
                out.smoke_ms = v.parse::<u64>().ok();
            }
        } else if a == "--gpu" {
            if let Some(v) = args.next() {
                out.gpu_pref = match v.to_ascii_lowercase().as_str() {
                    "low" => GpuPreference::LowPower,
                    "high" => GpuPreference::HighPerformance,
                    _ => GpuPreference::Auto,
                };
            }
        } else if a == "--test-beat-ms" {
            if let Some(v) = args.next() {
                out.test_beat_ms = v.parse::<u64>().ok();
            }
        } else if a == "--persist" {
            out.persist = true;
        } else if a == "--read-only" {
            out.read_only = true;
        } else if a == "--bcp-key" {
            out.bcp_key = args.next();
        }
    }
    out
}

fn part_res_for(size_dip: f64, dpr: f64) -> u32 {
    let raw = size_dip * dpr * 1.5;
    (64.0 * (raw / 64.0).ceil()).clamp(256.0, 1024.0) as u32
}

fn system_idle_seconds() -> f64 {
    let mut lii = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    let ok = unsafe { GetLastInputInfo(&mut lii) }.as_bool();
    if !ok {
        return 0.0;
    }
    let now_tick = unsafe { GetTickCount() };
    now_tick.wrapping_sub(lii.dwTime) as f64 / 1000.0
}

fn list_available_characters(data_dir: &Path) -> Vec<String> {
    let mut chars = Vec::new();
    let dir = data_dir.join("characters");
    if let Ok(rd) = fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join("character.json").is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    chars.push(name.to_string());
                }
            }
        }
    }
    chars.sort();
    chars
}

fn pet_electron_coords_from_rect(
    rect_px: (i32, i32, i32, i32),
    dpr: f64,
    geom: BugGeometry,
    win_w_dip: f64,
    win_h_dip: f64,
    lift: f64,
    monitors: &[MonitorMetrics],
) -> (f64, f64, Option<(u64, f64, f64)>) {
    let core_cx_dip = (rect_px.0 as f64 / dpr) + win_w_dip * 0.5;
    let core_by_dip = (rect_px.1 as f64 / dpr) + (win_h_dip - geom.size) * 0.5 + geom.size;
    let ex = (core_cx_dip - geom.side - geom.size * 0.5).round();
    let ey = (core_by_dip - geom.bubble_zone - geom.size + lift).round();

    let pcx = rect_px.0 + rect_px.2 / 2;
    let pcy = rect_px.1 + rect_px.3 / 2;
    let mon = pick_monitor_for_phys_point(pcx, pcy, monitors);
    let mon_x = mon.phys_left as f64 / mon.dpr;
    let mon_y = mon.phys_top as f64 / mon.dpr;
    let fx = (core_cx_dip - mon_x) / mon.dip_width().max(1.0);
    let fy = (core_by_dip - mon_y) / mon.dip_height().max(1.0);
    (ex, ey, Some((0, fx, fy)))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("[bug-pet-rs] Fatal error: {e}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_pets_from_state(
    state: &AppState,
    data_dir: &Path,
    protect: &ProtectKey,
    gpu: &GpuContext,
    monitors: &[MonitorMetrics],
    shared: &Rc<RefCell<SharedAppEvents>>,
    char_cache: &mut HashMap<(String, u32), Arc<CharacterGpuTextures>>,
    now_s: f64,
) -> Result<(), String> {
    for pet in &state.pets {
        let geom = BugGeometry::from_size(pet.size);
        let dpr = resolve_pet_dpr(pet, monitors);
        let max_w = part_res_for(geom.size, dpr);
        let cache_key = (pet.char_id.clone(), max_w);

        let gpu_chars = if let Some(cached) = char_cache.get(&cache_key) {
            cached.clone()
        } else {
            let loaded = LoadedCharacter::load(data_dir, &pet.char_id, protect)?;
            let uploaded = Arc::new(CharacterGpuTextures::upload(
                gpu,
                data_dir,
                &pet.char_id,
                protect,
                loaded,
                max_w,
            )?);
            char_cache.insert(cache_key, uploaded.clone());
            uploaded
        };

        let runtime = PetRuntime::new(pet.clone(), state.prefs.ui.clone(), gpu_chars, now_s);
        let pw = create_pet_window(pet.clone(), runtime, gpu, monitors, shared)?;
        let (rx, ry, rw, rh) = pw.rect_px;
        let rig_count = pw.runtime.rig.bundle.parts.len();
        println!(
            "[bug-pet-rs] Loaded pet `{}` ({}): size={}px DIP, rigParts={}, margin={:.2}, partRes={}, win_phys=({},{}) {}x{}, DPR={:.2}, clickThrough={}, music={}, typing={}",
            pw.runtime.state.char_id,
            pw.runtime.id,
            pw.runtime.geom.size,
            rig_count,
            pw.runtime.margin_frac,
            max_w,
            rx,
            ry,
            rw,
            rh,
            pw.dpr,
            pw.runtime.state.click_through,
            pw.runtime.state.music,
            pw.runtime.state.typing
        );
        let hwnd_key = pw.hwnd.0 as isize;
        let mut st = shared.borrow_mut();
        let idx = st.pets.len();
        st.window_map.insert(hwnd_key, idx);
        st.pets.push(pw);
    }
    Ok(())
}

fn sync_audio_and_typing_subsystems(
    shared: &Rc<RefCell<SharedAppEvents>>,
    react: &ReactPrefs,
    tray_hwnd: HWND,
    loopback: &mut Option<LoopbackHandle>,
) {
    let (any_music, any_typing) = {
        let mut st = shared.borrow_mut();
        st.key_every = react.key_every.max(1);
        let m = st.pets.iter().any(|p| p.runtime.state.music);
        let t = st.pets.iter().any(|p| p.runtime.state.typing);
        (m, t)
    };

    if any_music {
        if let Some(ref lb) = *loopback {
            lb.update_prefs(react);
        } else {
            match LoopbackHandle::start(react) {
                Ok(lb) => {
                    println!(
                        "[bug-pet-rs] WASAPI loopback active (mode={}, sense={}, gain={})",
                        react.sound, react.sense, react.gain
                    );
                    *loopback = Some(lb);
                }
                Err(e) => {
                    eprintln!("[bug-pet-rs] Warning: failed to start WASAPI loopback: {e}");
                }
            }
        }
    } else if let Some(mut lb) = loopback.take() {
        lb.shutdown();
    }

    let has_tw = shared.borrow().typing_watcher.is_some();
    if any_typing && !has_tw {
        match TypingWatcher::register(tray_hwnd) {
            Ok(tw) => {
                shared.borrow_mut().typing_watcher = Some(tw);
                println!(
                    "[bug-pet-rs] Raw Input keyboard hook registered (keyEvery={})",
                    react.key_every.max(1)
                );
            }
            Err(e) => {
                eprintln!("[bug-pet-rs] Warning: failed to register Raw Input hook: {e}");
            }
        }
    } else if !any_typing && has_tw {
        shared.borrow_mut().typing_watcher = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_pet_quick_action(
    pet_id: &str,
    action: PetQuickAction,
    gpu: &GpuContext,
    monitors: &[MonitorMetrics],
    shared: &Rc<RefCell<SharedAppEvents>>,
    persister: &mut StatePersister,
    tray_hwnd: HWND,
    loopback: &mut Option<LoopbackHandle>,
    tray_icon: &Option<TrayIcon>,
    state: &AppState,
) {
    let mut need_audio_sync = false;
    match action {
        PetQuickAction::SetSize(sz) => {
            let pos_update = {
                let mut st = shared.borrow_mut();
                if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                    if let Err(e) = resize_pet_window_to_size(pw, sz, gpu) {
                        eprintln!("[bug-pet-rs] Resize failed for `{pet_id}`: {e}");
                        None
                    } else {
                        let (ex, ey, anc) = pet_electron_coords_from_rect(
                            pw.rect_px,
                            pw.dpr,
                            pw.runtime.geom,
                            pw.runtime.win_w_dip,
                            pw.runtime.win_h_dip,
                            pw.runtime.state.lift,
                            monitors,
                        );
                        Some((pw.runtime.state.size, ex, ey, anc))
                    }
                } else {
                    None
                }
            };
            if let Some((actual_sz, ex, ey, anc)) = pos_update {
                persister.patch_pet_size(pet_id, actual_sz);
                persister.patch_pet_pos(pet_id, ex, ey, anc);
            }
        }
        PetQuickAction::DeltaSize(delta) => {
            let pos_update = {
                let mut st = shared.borrow_mut();
                if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                    let next_sz = (pw.runtime.state.size + delta).clamp(64.0, 512.0);
                    if let Err(e) = resize_pet_window_to_size(pw, next_sz, gpu) {
                        eprintln!("[bug-pet-rs] Resize failed for `{pet_id}`: {e}");
                        None
                    } else {
                        let (ex, ey, anc) = pet_electron_coords_from_rect(
                            pw.rect_px,
                            pw.dpr,
                            pw.runtime.geom,
                            pw.runtime.win_w_dip,
                            pw.runtime.win_h_dip,
                            pw.runtime.state.lift,
                            monitors,
                        );
                        Some((pw.runtime.state.size, ex, ey, anc))
                    }
                } else {
                    None
                }
            };
            if let Some((actual_sz, ex, ey, anc)) = pos_update {
                persister.patch_pet_size(pet_id, actual_sz);
                persister.patch_pet_pos(pet_id, ex, ey, anc);
            }
        }
        PetQuickAction::ToggleFlip => {
            let val = {
                let mut st = shared.borrow_mut();
                st.pets
                    .iter_mut()
                    .find(|p| p.runtime.id == pet_id)
                    .map(|pw| {
                        pw.runtime.state.flip = !pw.runtime.state.flip;
                        pw.runtime.state.flip
                    })
            };
            if let Some(v) = val {
                persister.patch_pet_field(pet_id, "flip", json!(v), true);
            }
        }
        PetQuickAction::ToggleLock => {
            let val = {
                let mut st = shared.borrow_mut();
                st.pets
                    .iter_mut()
                    .find(|p| p.runtime.id == pet_id)
                    .map(|pw| {
                        pw.runtime.state.lock = !pw.runtime.state.lock;
                        pw.runtime.state.lock
                    })
            };
            if let Some(v) = val {
                persister.patch_pet_field(pet_id, "lock", json!(v), true);
            }
        }
        PetQuickAction::ToggleClickThrough => {
            let val = {
                let mut st = shared.borrow_mut();
                st.pets
                    .iter_mut()
                    .find(|p| p.runtime.id == pet_id)
                    .map(|pw| {
                        pw.runtime.state.click_through = !pw.runtime.state.click_through;
                        pw.runtime.state.click_through
                    })
            };
            if let Some(v) = val {
                persister.patch_pet_field(pet_id, "clickThrough", json!(v), true);
                sync_click_through(shared);
            }
        }
        PetQuickAction::ToggleTop => {
            let val = {
                let mut st = shared.borrow_mut();
                st.pets
                    .iter_mut()
                    .find(|p| p.runtime.id == pet_id)
                    .map(|pw| {
                        let next = !pw.runtime.state.always_on_top;
                        set_pet_always_on_top(pw, next);
                        next
                    })
            };
            if let Some(v) = val {
                persister.patch_pet_top(pet_id, v);
            }
        }
        PetQuickAction::ToggleMute => {
            let val = {
                let mut st = shared.borrow_mut();
                st.pets
                    .iter_mut()
                    .find(|p| p.runtime.id == pet_id)
                    .map(|pw| {
                        pw.runtime.state.mute = !pw.runtime.state.mute;
                        pw.runtime.state.mute
                    })
            };
            if let Some(v) = val {
                persister.patch_pet_field(pet_id, "mute", json!(v), true);
            }
        }
        PetQuickAction::ToggleMusic => {
            let val = {
                let mut st = shared.borrow_mut();
                st.pets
                    .iter_mut()
                    .find(|p| p.runtime.id == pet_id)
                    .map(|pw| {
                        pw.runtime.state.music = !pw.runtime.state.music;
                        pw.runtime.state.music
                    })
            };
            if let Some(v) = val {
                persister.patch_pet_field(pet_id, "music", json!(v), true);
                need_audio_sync = true;
            }
        }
        PetQuickAction::ToggleTyping => {
            let val = {
                let mut st = shared.borrow_mut();
                st.pets
                    .iter_mut()
                    .find(|p| p.runtime.id == pet_id)
                    .map(|pw| {
                        pw.runtime.state.typing = !pw.runtime.state.typing;
                        pw.runtime.state.typing
                    })
            };
            if let Some(v) = val {
                persister.patch_pet_field(pet_id, "typing", json!(v), true);
                need_audio_sync = true;
            }
        }
        PetQuickAction::SetKeyBg(hex) => {
            let mut st = shared.borrow_mut();
            if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                pw.runtime.state.key_bg = hex.clone();
                pw.runtime.radial.key_bg = hex.clone();
                persister.patch_pet_key_bg(pet_id, &hex);
            }
        }
        PetQuickAction::TogglePart(part_id) => {
            let mut st = shared.borrow_mut();
            if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                pw.runtime.rig.toggle_part(&part_id);
                pw.runtime.radial.parts = pw.runtime.rig.part_toggles();
            }
        }
        PetQuickAction::SetAllParts(on) => {
            let mut st = shared.borrow_mut();
            if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                pw.runtime.rig.set_all_parts(on);
                pw.runtime.radial.parts = pw.runtime.rig.part_toggles();
            }
        }
        PetQuickAction::ClosePet => {
            let remaining = {
                let mut st = shared.borrow_mut();
                if let Some(pos) = st.pets.iter().position(|p| p.runtime.id == pet_id) {
                    let mut removed = st.pets.remove(pos);
                    destroy_pet_window(&mut removed, &mut st.overlay_map);
                    st.rebuild_window_map();
                }
                st.pets.len()
            };
            persister.patch_spawn_or_remove_pet(pet_id, None);
            if let Some(ref ti) = *tray_icon {
                ti.update_count(remaining, &state.prefs.ui.lang);
            }
            need_audio_sync = true;
        }
    }

    if need_audio_sync {
        sync_audio_and_typing_subsystems(shared, &state.prefs.react, tray_hwnd, loopback);
    }
}

#[allow(clippy::too_many_arguments)]
fn reload_from_source_live(
    data_dir: &Path,
    protect: &ProtectKey,
    gpu: &GpuContext,
    monitors: &[MonitorMetrics],
    shared: &Rc<RefCell<SharedAppEvents>>,
    char_cache: &mut HashMap<(String, u32), Arc<CharacterGpuTextures>>,
    available_chars: &mut Vec<String>,
    state: &mut AppState,
    persister: &mut StatePersister,
    remind_sched: &mut RemindScheduler,
    next_pet_num: &mut u64,
    rest_limit_s: &mut f64,
    tray_hwnd: HWND,
    loopback: &mut Option<LoopbackHandle>,
    tray_icon: &Option<TrayIcon>,
    now_s: f64,
) -> Result<(), String> {
    let new_state = persister.reload_from_source()?;

    {
        let mut st = shared.borrow_mut();
        let mut ov_map = std::mem::take(&mut st.overlay_map);
        for pw in &mut st.pets {
            destroy_pet_window(pw, &mut ov_map);
        }
        st.pets.clear();
        st.window_map.clear();
        st.overlay_map = ov_map;
        st.dragging_hwnd = None;
        st.drag_moved = false;
        st.ghost_mode = new_state.prefs.ui.ghost;
    }

    char_cache.clear();
    *available_chars = list_available_characters(data_dir);
    *state = new_state;
    *remind_sched = RemindScheduler::from_json(&state.remind);
    *next_pet_num = state.next_id.max(state.pets.len() as u64 + 1);
    *rest_limit_s = (state.prefs.ui.rest_min.max(1.0)) * 60.0;

    spawn_pets_from_state(
        state,
        data_dir,
        protect,
        gpu,
        monitors,
        shared,
        char_cache,
        now_s,
    )?;

    let count = {
        let mut st = shared.borrow_mut();
        for pw in &mut st.pets {
            pw.runtime.render(gpu, &pw.surface, now_s)?;
        }
        st.pets.len()
    };
    if let Some(ref ti) = *tray_icon {
        ti.update_count(count, &state.prefs.ui.lang);
    }
    sync_audio_and_typing_subsystems(shared, &state.prefs.react, tray_hwnd, loopback);
    sync_click_through(shared);
    gpu.commit()?;
    Ok(())
}

fn run() -> Result<(), String> {
    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    let cli = parse_args();
    enable_per_monitor_dpi_v2();
    let _com_guard = ComGuard(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok());

    let data_dir = discover_data_dir(cli.data_dir.as_deref())?;
    let protect = ProtectKey::init_with_key(&data_dir, cli.bcp_key.as_deref())?;
    let persist_enabled = !cli.read_only && (cli.persist || cli.smoke_ms.is_none());
    let source_state_path = data_dir.join("state.json");
    let (mut state, mut persister, from_sidecar) =
        load_synced_state(&source_state_path, persist_enabled, None)?;
    let monitors = enumerate_monitors();
    let gpu = GpuContext::new(cli.gpu_pref)?;
    let mut available_chars = list_available_characters(&data_dir);

    let mut remind_sched = RemindScheduler::from_json(&state.remind);
    let mut launch_at_login = if persist_enabled {
        is_autostart_enabled()
    } else {
        state.launch_at_login
    };

    println!(
        "[bug-pet-rs] Data dir: {} (protect={}, persist={}, state={})",
        data_dir.display(),
        protect.mode,
        persister.is_enabled(),
        if from_sidecar { "sidecar" } else { "bug-clicker" }
    );
    println!(
        "[bug-pet-rs] GPU adapter ({:?}): {}",
        cli.gpu_pref, gpu.adapter_name
    );
    for (i, m) in monitors.iter().enumerate() {
        println!(
            "[bug-pet-rs] Monitor[{i}]: phys=({},{} {}x{}), work=({},{} {}x{}), dpi={} (DPR {:.2}), primary={}",
            m.phys_left,
            m.phys_top,
            m.phys_width,
            m.phys_height,
            m.work_left,
            m.work_top,
            m.work_width,
            m.work_height,
            m.dpi,
            m.dpr,
            m.is_primary
        );
    }

    register_window_class()?;

    let mut char_cache: HashMap<(String, u32), Arc<CharacterGpuTextures>> = HashMap::new();
    let sound_engine = Some(SoundEngine::start());
    let shared = Rc::new(RefCell::new(SharedAppEvents {
        start_time: Instant::now(),
        should_quit: false,
        ghost_mode: state.prefs.ui.ghost,
        dragging_hwnd: None,
        drag_start_cursor: (0, 0),
        drag_start_win: (0, 0),
        drag_moved: false,
        window_map: HashMap::new(),
        overlay_map: HashMap::new(),
        pets: Vec::new(),
        typing_watcher: None,
        key_every: state.prefs.react.key_every.max(1),
        sound_engine,
        pending_beat_ignore: false,
        pending_actions: Vec::new(),
    }));

    let tray_hwnd = create_tray_host_window(&shared)?;

    let mut next_pet_num = state.next_id.max(state.pets.len() as u64 + 1);
    spawn_pets_from_state(
        &state,
        &data_dir,
        &protect,
        &gpu,
        &monitors,
        &shared,
        &mut char_cache,
        0.0,
    )?;

    if shared.borrow().pets.is_empty() {
        return Err("No pets found in state.json".to_string());
    }

    let tray_icon = match TrayIcon::new(
        tray_hwnd,
        Some(&data_dir),
        shared.borrow().pets.len(),
        &state.prefs.ui.lang,
    ) {
        Ok(ti) => Some(ti),
        Err(e) => {
            eprintln!("[bug-pet-rs] Warning: failed to create system tray icon: {e}");
            None
        }
    };

    let mut loopback: Option<LoopbackHandle> = None;
    sync_audio_and_typing_subsystems(&shared, &state.prefs.react, tray_hwnd, &mut loopback);

    // Initial render before starting steady-state frame timer
    {
        let mut st = shared.borrow_mut();
        for pw in &mut st.pets {
            pw.runtime.render(&gpu, &pw.surface, 0.0)?;
        }
    }
    gpu.commit()?;

    let wait_timer: Option<HANDLE> = unsafe {
        CreateWaitableTimerExW(
            None,
            None,
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS.0,
        )
        .ok()
    };

    let target_frame_dt = Duration::from_nanos(16_666_667);
    let loop_start = Instant::now();
    shared.borrow_mut().start_time = loop_start;

    let mut last_tick = loop_start;
    let mut next_frame = loop_start;
    let mut next_idle_check = loop_start;
    let mut next_remind_check = loop_start + Duration::from_secs(15);
    let mut next_sync_check = loop_start + Duration::from_secs(3);
    let mut last_remind_ms = Local::now().timestamp_millis();
    let mut was_resting = false;
    let mut frames_rendered: u64 = 0;
    let mut next_test_beat_s: f64 = cli
        .test_beat_ms
        .map(|ms| ms as f64 / 1000.0)
        .unwrap_or(f64::INFINITY);
    let test_beat_iv_s = cli
        .test_beat_ms
        .map(|ms| ms as f64 / 1000.0)
        .unwrap_or(0.5);
    let mut rest_limit_s = (state.prefs.ui.rest_min.max(1.0)) * 60.0;
    let mut msg = MSG::default();

    loop {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    shared.borrow_mut().should_quit = true;
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        if shared.borrow().should_quit {
            break;
        }
        if let Some(smoke_ms) = cli.smoke_ms {
            if loop_start.elapsed() >= Duration::from_millis(smoke_ms) {
                break;
            }
        }

        let now = Instant::now();
        let now_s = (now - loop_start).as_secs_f64();

        // Drain UI actions safely outside wnd_proc RefCell borrows
        let actions: Vec<PendingUiAction> = {
            let mut st = shared.borrow_mut();
            std::mem::take(&mut st.pending_actions)
        };
        for act in actions {
            match act {
                PendingUiAction::OpenTrayMenu => {
                    let (ghost_now, pet_snaps) = {
                        let st = shared.borrow();
                        let snaps: Vec<PetMenuSnapshot> =
                            st.pets.iter().map(|p| p.runtime.menu_snapshot()).collect();
                        (st.ghost_mode, snaps)
                    };
                    let t_act = tray_icon.as_ref().and_then(|ti| {
                        let gm = GlobalMenuState {
                            characters: &available_chars,
                            pets: &pet_snaps,
                            ghost: ghost_now,
                            launch_at_login,
                            react: &state.prefs.react,
                            ui: &state.prefs.ui,
                        };
                        ti.show_context_menu(&gm)
                    });
                    if let Some(t_act) = t_act {
                        let mut react_dirty = false;
                        let mut ui_dirty = false;
                        match t_act {
                            TrayAction::SpawnCharacter(char_id) => {
                                let mut pt = POINT::default();
                                let _ = unsafe { GetCursorPos(&mut pt) };
                                let mon = pick_monitor_for_phys_point(pt.x, pt.y, &monitors);
                                let size = 180.0;
                                let geom = BugGeometry::from_size(size);
                                let max_w = part_res_for(geom.size, mon.dpr);
                                let cache_key = (char_id.clone(), max_w);
                                let gpu_chars = if let Some(c) = char_cache.get(&cache_key) {
                                    Ok(c.clone())
                                } else {
                                    LoadedCharacter::load(&data_dir, &char_id, &protect)
                                        .and_then(|loaded| {
                                            CharacterGpuTextures::upload(
                                                &gpu, &data_dir, &char_id, &protect, loaded, max_w,
                                            )
                                        })
                                        .map(|u| {
                                            let arc = Arc::new(u);
                                            char_cache.insert(cache_key, arc.clone());
                                            arc
                                        })
                                };
                                if let Ok(gc) = gpu_chars {
                                    let id = format!("pet{next_pet_num}");
                                    next_pet_num += 1;
                                    let cx_dip =
                                        (mon.work_left + mon.work_width / 2) as f64 / mon.dpr;
                                    let by_dip = (mon.work_top + mon.work_height / 2) as f64
                                        / mon.dpr
                                        + geom.size * 0.5;
                                    let pet_st = PetState {
                                        id: id.clone(),
                                        char_id: char_id.clone(),
                                        x: (cx_dip - geom.side - geom.size * 0.5).round(),
                                        y: (by_dip - geom.bubble_zone - geom.size).round(),
                                        size,
                                        always_on_top: true,
                                        click_through: true,
                                        music: true,
                                        ..Default::default()
                                    };
                                    let runtime = PetRuntime::new(
                                        pet_st.clone(),
                                        state.prefs.ui.clone(),
                                        gc,
                                        now_s,
                                    );
                                    if let Ok(pw) = create_pet_window(
                                        pet_st.clone(),
                                        runtime,
                                        &gpu,
                                        &monitors,
                                        &shared,
                                    ) {
                                        persister.patch_spawn_or_remove_pet(&id, Some(&pet_st));
                                        let count = {
                                            let mut st = shared.borrow_mut();
                                            st.pets.push(pw);
                                            st.rebuild_window_map();
                                            st.pets.len()
                                        };
                                        if let Some(ref ti) = tray_icon {
                                            ti.update_count(count, &state.prefs.ui.lang);
                                        }
                                        sync_audio_and_typing_subsystems(
                                            &shared,
                                            &state.prefs.react,
                                            tray_hwnd,
                                            &mut loopback,
                                        );
                                    }
                                }
                            }
                            TrayAction::GatherAll => {
                                let mut pt = POINT::default();
                                let _ = unsafe { GetCursorPos(&mut pt) };
                                let mon = pick_monitor_for_phys_point(pt.x, pt.y, &monitors);
                                let mut st = shared.borrow_mut();
                                let n = st.pets.len().max(1);
                                let step_x =
                                    ((mon.work_width / (n as i32 + 1)).min(260)).max(80);
                                let start_x = mon.work_left + mon.work_width / 2
                                    - ((n as i32 - 1) * step_x) / 2;
                                let center_y = mon.work_top + mon.work_height / 2;
                                for (i, pw) in st.pets.iter_mut().enumerate() {
                                    let nx = start_x + (i as i32) * step_x - pw.rect_px.2 / 2;
                                    let ny = center_y - pw.rect_px.3 / 2;
                                    move_pet_window_px(pw, nx, ny);
                                    let (ex, ey, anc) = pet_electron_coords_from_rect(
                                        pw.rect_px,
                                        pw.dpr,
                                        pw.runtime.geom,
                                        pw.runtime.win_w_dip,
                                        pw.runtime.win_h_dip,
                                        pw.runtime.state.lift,
                                        &monitors,
                                    );
                                    persister.patch_pet_pos(&pw.runtime.id, ex, ey, anc);
                                }
                            }
                            TrayAction::ToggleGhost => {
                                let new_ghost = !ghost_now;
                                state.prefs.ui.ghost = new_ghost;
                                shared.borrow_mut().ghost_mode = new_ghost;
                                persister.patch_ghost(new_ghost);
                            }
                            TrayAction::ToggleLaunchAtLogin => {
                                let want = !launch_at_login;
                                if set_autostart_enabled(
                                    want,
                                    cli.data_dir.as_deref().map(Path::new),
                                )
                                .is_ok()
                                {
                                    launch_at_login = want;
                                    persister.patch_launch_at_login(want);
                                }
                            }
                            TrayAction::ReloadFromSource => {
                                if let Err(e) = reload_from_source_live(
                                    &data_dir,
                                    &protect,
                                    &gpu,
                                    &monitors,
                                    &shared,
                                    &mut char_cache,
                                    &mut available_chars,
                                    &mut state,
                                    &mut persister,
                                    &mut remind_sched,
                                    &mut next_pet_num,
                                    &mut rest_limit_s,
                                    tray_hwnd,
                                    &mut loopback,
                                    &tray_icon,
                                    now_s,
                                ) {
                                    eprintln!("[bug-pet-rs] Reload from BUG Clicker failed: {e}");
                                } else {
                                    println!("[bug-pet-rs] Reloaded from BUG Clicker state.json.");
                                }
                            }
                            TrayAction::Quit => {
                                shared.borrow_mut().should_quit = true;
                            }
                            TrayAction::PetQuick { pet_id, action } => {
                                apply_pet_quick_action(
                                    &pet_id,
                                    action,
                                    &gpu,
                                    &monitors,
                                    &shared,
                                    &mut persister,
                                    tray_hwnd,
                                    &mut loopback,
                                    &tray_icon,
                                    &state,
                                );
                            }
                            TrayAction::SetReactSense(s) => {
                                state.prefs.react.sense = s.clamp(1.0, 10.0);
                                react_dirty = true;
                            }
                            TrayAction::SetReactSound(m) => {
                                state.prefs.react.sound = m;
                                react_dirty = true;
                            }
                            TrayAction::SetReactGain(g) => {
                                state.prefs.react.gain = g;
                                react_dirty = true;
                            }
                            TrayAction::SetKeyEvery(n) => {
                                state.prefs.react.key_every = n.max(1);
                                react_dirty = true;
                            }
                            TrayAction::ToggleRestIdle => {
                                state.prefs.ui.rest_idle = !state.prefs.ui.rest_idle;
                                ui_dirty = true;
                            }
                            TrayAction::SetRestMin(m) => {
                                state.prefs.ui.rest_min = m.max(1.0);
                                ui_dirty = true;
                            }
                            TrayAction::SetBubbleFont(px) => {
                                state.prefs.ui.bubble_font = px.clamp(10.0, 28.0);
                                ui_dirty = true;
                            }
                            TrayAction::ToggleReduceMotion => {
                                let next = !(state.prefs.ui.lite || state.prefs.ui.reduce_motion);
                                state.prefs.ui.reduce_motion = next;
                                state.prefs.ui.lite = next;
                                ui_dirty = true;
                            }
                            TrayAction::SetLang(lang) => {
                                state.prefs.ui.lang = lang;
                                ui_dirty = true;
                            }
                        }

                        if react_dirty {
                            persister.patch_react_prefs(&state.prefs.react);
                            sync_audio_and_typing_subsystems(
                                &shared,
                                &state.prefs.react,
                                tray_hwnd,
                                &mut loopback,
                            );
                        }
                        if ui_dirty {
                            rest_limit_s = (state.prefs.ui.rest_min.max(1.0)) * 60.0;
                            let count = {
                                let mut st = shared.borrow_mut();
                                for pw in &mut st.pets {
                                    pw.runtime.set_ui_prefs(state.prefs.ui.clone());
                                }
                                st.pets.len()
                            };
                            if let Some(ref ti) = tray_icon {
                                ti.update_count(count, &state.prefs.ui.lang);
                            }
                            persister.patch_ui_prefs(&state.prefs.ui);
                        }
                    }
                }
                PendingUiAction::TogglePetMenu(pet_id) => {
                    let mut st = shared.borrow_mut();
                    if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                        if pw.runtime.radial.is_visible() {
                            pw.runtime.radial.close(now_s);
                        } else {
                            let ov_rect = compute_overlay_rect_px(pw.rect_px, pw.dpr, &monitors);
                            let ov_origin =
                                (ov_rect.0 as f64 / pw.dpr, ov_rect.1 as f64 / pw.dpr);
                            let ov_w = ov_rect.2 as f64 / pw.dpr;
                            let ov_h = ov_rect.3 as f64 / pw.dpr;
                            let (cx, cy) =
                                pw.runtime.core_center_in_overlay(pw.rect_px, ov_origin, pw.dpr);
                            pw.runtime.radial.always_on_top = pw.runtime.state.always_on_top;
                            pw.runtime.radial.key_bg = pw.runtime.state.key_bg.clone();
                            pw.runtime.radial.parts = pw.runtime.rig.part_toggles();
                            pw.runtime.radial.open_at(
                                cx,
                                cy,
                                pw.runtime.geom.size,
                                ov_w,
                                ov_h,
                                Some((0.0, 0.0, ov_w, ov_h)),
                                now_s,
                            );
                            pw.runtime.pet_fx.reset_immediate();
                        }
                    }
                }
                PendingUiAction::OverlayHover {
                    pet_id,
                    local_x_dip,
                    local_y_dip,
                } => {
                    let mut st = shared.borrow_mut();
                    if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                        if pw.runtime.radial.is_visible() {
                            pw.runtime.radial.on_hover(local_x_dip, local_y_dip, now_s);
                        }
                    }
                }
                PendingUiAction::OverlayClick {
                    pet_id,
                    local_x_dip,
                    local_y_dip,
                } => {
                    let mut radial_act: Option<RadialAction> = None;
                    let mut dismissed_remind: Option<(RemindKind, String)> = None;
                    {
                        let mut st = shared.borrow_mut();
                        if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                            if pw.runtime.radial.is_visible() {
                                if pw.runtime.radial.hit_test(local_x_dip, local_y_dip).is_some() {
                                    radial_act =
                                        pw.runtime.radial.on_click(local_x_dip, local_y_dip, now_s);
                                } else {
                                    pw.runtime.radial.close(now_s);
                                }
                            } else if let Some(ref rm) = pw.runtime.active_remind {
                                let (l, t, r, b) = rm.hot_rect_local_dip;
                                if local_x_dip >= l
                                    && local_x_dip <= r
                                    && local_y_dip >= t
                                    && local_y_dip <= b
                                {
                                    dismissed_remind = pw.runtime.dismiss_remind(now_s);
                                }
                            }
                        }
                    }
                    if let Some((RemindKind::Pill, slot)) = dismissed_remind {
                        let today = Local::now().format("%Y-%m-%d").to_string();
                        if remind_sched.mark_pill_taken(&today, &slot) {
                            persister.patch_pill_taken(&today, &slot);
                        }
                    }
                    if let Some(ra) = radial_act {
                        match ra {
                            RadialAction::ToggleTop => {
                                let mut st = shared.borrow_mut();
                                if let Some(pw) =
                                    st.pets.iter_mut().find(|p| p.runtime.id == pet_id)
                                {
                                    let next = !pw.runtime.state.always_on_top;
                                    set_pet_always_on_top(pw, next);
                                    persister.patch_pet_top(&pet_id, next);
                                }
                            }
                            RadialAction::TogglePart(part_id) => {
                                let mut st = shared.borrow_mut();
                                if let Some(pw) =
                                    st.pets.iter_mut().find(|p| p.runtime.id == pet_id)
                                {
                                    pw.runtime.rig.toggle_part(&part_id);
                                    pw.runtime.radial.parts = pw.runtime.rig.part_toggles();
                                }
                            }
                            RadialAction::SetAllParts(on) => {
                                let mut st = shared.borrow_mut();
                                if let Some(pw) =
                                    st.pets.iter_mut().find(|p| p.runtime.id == pet_id)
                                {
                                    pw.runtime.rig.set_all_parts(on);
                                    pw.runtime.radial.parts = pw.runtime.rig.part_toggles();
                                }
                            }
                            RadialAction::SetKeyBg(hex) => {
                                let mut st = shared.borrow_mut();
                                if let Some(pw) =
                                    st.pets.iter_mut().find(|p| p.runtime.id == pet_id)
                                {
                                    pw.runtime.state.key_bg = hex.clone();
                                    pw.runtime.radial.key_bg = hex.clone();
                                    persister.patch_pet_key_bg(&pet_id, &hex);
                                }
                            }
                            RadialAction::Shot => {
                                println!(
                                    "[bug-pet-rs] Radial action `Shot` requested on `{pet_id}` (capture UI not bundled in lightweight runtime)."
                                );
                            }
                            RadialAction::OpenSettings => {
                                let snap = {
                                    let st = shared.borrow();
                                    st.pets
                                        .iter()
                                        .find(|p| p.runtime.id == pet_id)
                                        .map(|p| p.runtime.menu_snapshot())
                                };
                                if let Some(snap) = snap {
                                    if let Some(qa) = show_pet_settings_menu(
                                        tray_hwnd,
                                        &snap,
                                        &state.prefs.ui.lang,
                                    ) {
                                        apply_pet_quick_action(
                                            &pet_id,
                                            qa,
                                            &gpu,
                                            &monitors,
                                            &shared,
                                            &mut persister,
                                            tray_hwnd,
                                            &mut loopback,
                                            &tray_icon,
                                            &state,
                                        );
                                    }
                                }
                            }
                            RadialAction::ClosePet => {
                                apply_pet_quick_action(
                                    &pet_id,
                                    PetQuickAction::ClosePet,
                                    &gpu,
                                    &monitors,
                                    &shared,
                                    &mut persister,
                                    tray_hwnd,
                                    &mut loopback,
                                    &tray_icon,
                                    &state,
                                );
                            }
                            RadialAction::QuitAll => {
                                shared.borrow_mut().should_quit = true;
                            }
                        }
                    }
                }
                PendingUiAction::CloseRadial(pet_id) => {
                    let mut st = shared.borrow_mut();
                    if let Some(pw) = st.pets.iter_mut().find(|p| p.runtime.id == pet_id) {
                        if pw.runtime.radial.is_visible() {
                            pw.runtime.radial.close(now_s);
                        }
                    }
                }
                PendingUiAction::DragEnded(pet_id) => {
                    let st = shared.borrow();
                    if let Some(pw) = st.pets.iter().find(|p| p.runtime.id == pet_id) {
                        let (ex, ey, anc) = pet_electron_coords_from_rect(
                            pw.rect_px,
                            pw.dpr,
                            pw.runtime.geom,
                            pw.runtime.win_w_dip,
                            pw.runtime.win_h_dip,
                            pw.runtime.state.lift,
                            &monitors,
                        );
                        persister.patch_pet_pos(&pet_id, ex, ey, anc);
                    }
                }
            }
        }

        if shared.borrow().should_quit {
            break;
        }

        // 15-second reminder scheduler tick
        if now >= next_remind_check {
            next_remind_check = now + Duration::from_secs(15);
            let now_ms = Local::now().timestamp_millis();
            let pet_tuples: Vec<(String, String, bool)> = shared
                .borrow()
                .pets
                .iter()
                .map(|p| (p.runtime.id.clone(), p.runtime.state.char_id.clone(), false))
                .collect();
            let events = remind_sched.tick(last_remind_ms, now_ms, &pet_tuples);
            last_remind_ms = now_ms;
            for alarm_id in remind_sched.take_disabled_alarm_ids() {
                persister.patch_alarm_off(&alarm_id);
            }
            if !events.is_empty() {
                let mut st = shared.borrow_mut();
                for ev in events {
                    if let Some((ref snd_kind, vol)) = ev.sound {
                        if let Some(ref se) = st.sound_engine {
                            se.play_alert(snd_kind, vol);
                        }
                    }
                    if let Some(pw) = st
                        .pets
                        .iter_mut()
                        .find(|p| p.runtime.id == ev.target_pet_id)
                    {
                        pw.runtime.push_remind(ev, now_s);
                    }
                }
            }
        }

        // Automatic live sync when original BUG Clicker updates state.json
        if cli.smoke_ms.is_none() && now >= next_sync_check {
            next_sync_check = now + Duration::from_secs(3);
            if persister.source_changed() {
                match reload_from_source_live(
                    &data_dir,
                    &protect,
                    &gpu,
                    &monitors,
                    &shared,
                    &mut char_cache,
                    &mut available_chars,
                    &mut state,
                    &mut persister,
                    &mut remind_sched,
                    &mut next_pet_num,
                    &mut rest_limit_s,
                    tray_hwnd,
                    &mut loopback,
                    &tray_icon,
                    now_s,
                ) {
                    Ok(()) => {
                        println!("[bug-pet-rs] Detected BUG Clicker state.json change — reloaded live.");
                    }
                    Err(e) => {
                        eprintln!("[bug-pet-rs] Auto-sync reload warning: {e}");
                    }
                }
            }
        }

        let _ = persister.flush_if_due();

        // Drain beats & click-ignore signal even around rest transitions
        let ignore_click = {
            let mut st = shared.borrow_mut();
            let v = st.pending_beat_ignore;
            st.pending_beat_ignore = false;
            v
        };
        if ignore_click {
            if let Some(ref lb) = loopback {
                lb.ignore_clicks(0.26);
            }
        }

        let mut beats: Vec<BeatEvent> = Vec::new();
        if let Some(ref lb) = loopback {
            while let Some(ev) = lb.try_recv_beat() {
                beats.push(ev);
            }
        }

        // Music-aware OS idle check at 1 Hz
        let audio_playing =
            loopback.as_ref().is_some_and(|lb| lb.is_playing()) || !beats.is_empty();
        let interacting = {
            let st = shared.borrow();
            st.dragging_hwnd.is_some()
                || st.pets.iter().any(|p| {
                    p.runtime.down
                        || p.runtime.release_due_s.is_some()
                        || p.runtime.has_overlay_content(now_s)
                        || p.runtime.pet_fx.is_active(now_s)
                })
        };

        let mut currently_resting = was_resting;
        if audio_playing || interacting {
            currently_resting = false;
        } else if state.prefs.ui.rest_idle && now >= next_idle_check {
            next_idle_check = now + Duration::from_secs(1);
            let idle_s = system_idle_seconds();
            currently_resting = idle_s >= rest_limit_s;
        } else if !state.prefs.ui.rest_idle {
            currently_resting = false;
        }

        if currently_resting != was_resting {
            let mut st = shared.borrow_mut();
            for pw in &mut st.pets {
                pw.runtime.set_resting(currently_resting);
            }
            if !currently_resting {
                last_tick = now;
                next_frame = now;
            }
        }

        let stay_frozen_idle = currently_resting && was_resting;
        was_resting = currently_resting;

        let sleep_dur = if stay_frozen_idle {
            Duration::from_millis(250)
        } else {
            let dt_s = (now - last_tick).as_secs_f32().clamp(0.001, 0.05);
            last_tick = now;

            {
                let shared_ptr = Rc::as_ptr(&shared) as isize;
                let mut st = shared.borrow_mut();
                let fire_test_beat = if now_s >= next_test_beat_s {
                    next_test_beat_s += test_beat_iv_s;
                    true
                } else {
                    false
                };

                let mut delayed_ups: Vec<(String, f64)> = Vec::new();
                let mut ov_map = std::mem::take(&mut st.overlay_map);
                for pw in &mut st.pets {
                    if !currently_resting {
                        if fire_test_beat && pw.runtime.state.music {
                            pw.runtime.on_beat(0.85, test_beat_iv_s, now_s);
                        }
                        for ev in &beats {
                            if pw.runtime.state.music {
                                pw.runtime.on_beat(ev.strength, ev.interval_s, now_s);
                            }
                        }
                    }
                    if pw.runtime.step(now_s, dt_s) {
                        if let Some((kind, vol)) = pw.runtime.click_sound_params() {
                            delayed_ups.push((kind.to_string(), vol));
                        }
                    }
                    pw.runtime.render(&gpu, &pw.surface, now_s)?;

                    let need_overlay = pw.runtime.has_overlay_content(now_s)
                        || pw.runtime.pending_say_pool.is_some();
                    if need_overlay {
                        let want_focus = pw.runtime.radial.is_visible();
                        let want_topmost = pw.runtime.state.always_on_top || want_focus;
                        ensure_and_position_overlay(
                            pw,
                            &gpu,
                            &monitors,
                            shared_ptr,
                            &mut ov_map,
                            want_focus,
                            want_topmost,
                        )?;
                        if let Some(ref ov) = pw.overlay {
                            pw.runtime.update_and_render_overlay(
                                &gpu,
                                &ov.surface,
                                pw.rect_px,
                                ov.rect_px,
                                ov.dpr,
                                now_s,
                            )?;
                        }
                        if !pw.runtime.has_overlay_content(now_s) {
                            hide_overlay(pw);
                        }
                    } else {
                        hide_overlay(pw);
                    }
                }
                st.overlay_map = ov_map;

                if let Some(ref se) = st.sound_engine {
                    for (kind, vol) in delayed_ups {
                        se.play_click(&kind, true, vol, 0.0);
                    }
                }
            }
            sync_click_through(&shared);
            gpu.commit()?;
            frames_rendered += 1;

            let now_after = Instant::now();
            while next_frame <= now_after {
                next_frame += target_frame_dt;
            }
            next_frame.saturating_duration_since(now_after)
        };

        if let Some(th) = wait_timer {
            let due_100ns = -((sleep_dur.as_nanos() / 100).max(1) as i64);
            unsafe {
                let _ = SetWaitableTimer(th, &due_100ns, 0, None, None, false);
                let _ = MsgWaitForMultipleObjectsEx(
                    Some(&[th]),
                    u32::MAX,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                );
            }
        } else {
            let ms = sleep_dur.as_millis().max(1) as u32;
            unsafe {
                let r = MsgWaitForMultipleObjectsEx(
                    None,
                    ms,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                );
                let _ = WAIT_OBJECT_0;
                let _ = r;
            }
        }
    }

    let loop_end_elapsed_s = loop_start.elapsed().as_secs_f64();
    let _ = persister.flush();
    if let Some(mut ti) = tray_icon {
        ti.remove();
    }
    if let Some(th) = wait_timer {
        unsafe {
            let _ = CloseHandle(th);
        }
    }
    if let Some(ref mut se) = shared.borrow_mut().sound_engine {
        se.shutdown();
    }
    if let Some(ref mut lb) = loopback {
        if cli.smoke_ms.is_some() {
            let snap = lb.snapshot();
            println!(
                "[bug-pet-rs] WASAPI loopback summary: init={}, sr={}Hz ch={}, frames={}, beats={}, silentSleeps={}, playing={}, locked={}, bpm={:.1}",
                snap.initialized,
                snap.sample_rate,
                snap.channels,
                snap.frames_processed,
                snap.beats_detected,
                snap.silent_sleeps,
                snap.is_playing,
                snap.locked,
                snap.bpm
            );
        }
        lb.shutdown();
    }
    destroy_all_windows(&shared, Some(tray_hwnd));

    if let Some(smoke_ms) = cli.smoke_ms {
        let total_s = loop_end_elapsed_s;
        let fps = frames_rendered as f64 / total_s.max(0.001);
        let avg_ms = (total_s * 1000.0) / frames_rendered.max(1) as f64;
        println!(
            "[bug-pet-rs] Smoke pass complete ({} ms target, {:.2} s loop, {:.2} s w/shutdown): frames={}, fps={:.1}, avg={:.2} ms/frame",
            smoke_ms,
            total_s,
            loop_start.elapsed().as_secs_f64(),
            frames_rendered,
            fps,
            avg_ms
        );
    }

    Ok(())
}
