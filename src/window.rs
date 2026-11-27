use crate::audio::synth::SoundEngine;
use crate::config::PetState;
use crate::pet_state::PetRuntime;
use crate::renderer::{GpuContext, PetSurface};
use crate::tray::WM_TRAYICON;
use crate::typing::TypingWatcher;
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::rc::Rc;
use std::time::Instant;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
};

const WM_MOUSELEAVE: u32 = 0x02A3;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetWindowLongPtrW, LoadCursorW,
    PostQuitMessage, RegisterClassExW, SetCursor, SetForegroundWindow, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWLP_USERDATA, HTCLIENT,
    HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, IDC_HAND, LWA_ALPHA, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
    WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CLOSE, WM_INPUT,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WM_SETCURSOR,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_OVERLAPPED, WS_POPUP, WS_VISIBLE,
};

#[derive(Debug, Clone, Copy)]
pub struct MonitorMetrics {
    pub phys_left: i32,
    pub phys_top: i32,
    pub phys_width: i32,
    pub phys_height: i32,
    pub work_left: i32,
    pub work_top: i32,
    pub work_width: i32,
    pub work_height: i32,
    pub dpi: u32,
    pub dpr: f64,
    pub is_primary: bool,
}

impl MonitorMetrics {
    pub fn dip_width(&self) -> f64 {
        self.phys_width as f64 / self.dpr
    }
    pub fn dip_height(&self) -> f64 {
        self.phys_height as f64 / self.dpr
    }
}

pub fn enable_per_monitor_dpi_v2() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn enumerate_monitors() -> Vec<MonitorMetrics> {
    let mut list: Vec<MonitorMetrics> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_mon_proc),
            LPARAM(&mut list as *mut Vec<MonitorMetrics> as isize),
        );
    }
    list
}

unsafe extern "system" fn enum_mon_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<MonitorMetrics>);
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        let mut dpi_x = 96u32;
        let mut dpi_y = 96u32;
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let dpr = (dpi_x.max(96) as f64) / 96.0;
        list.push(MonitorMetrics {
            phys_left: mi.rcMonitor.left,
            phys_top: mi.rcMonitor.top,
            phys_width: mi.rcMonitor.right - mi.rcMonitor.left,
            phys_height: mi.rcMonitor.bottom - mi.rcMonitor.top,
            work_left: mi.rcWork.left,
            work_top: mi.rcWork.top,
            work_width: mi.rcWork.right - mi.rcWork.left,
            work_height: mi.rcWork.bottom - mi.rcWork.top,
            dpi: dpi_x,
            dpr,
            is_primary: (mi.dwFlags & 1) != 0,
        });
    }
    BOOL(1)
}

pub fn resolve_pet_dpr(pet_state: &PetState, monitors: &[MonitorMetrics]) -> f64 {
    let geom = crate::config::BugGeometry::from_size(pet_state.size);
    let (cx_dip, by_dip) = geom.core_anchor_dip(pet_state);
    pick_monitor_for_point(cx_dip, by_dip, monitors).dpr
}

pub fn pick_monitor_for_point(
    cx_dip: f64,
    by_dip: f64,
    monitors: &[MonitorMetrics],
) -> MonitorMetrics {
    let primary = monitors
        .iter()
        .find(|m| m.is_primary)
        .or_else(|| monitors.first())
        .copied()
        .unwrap_or(MonitorMetrics {
            phys_left: 0,
            phys_top: 0,
            phys_width: 1920,
            phys_height: 1080,
            work_left: 0,
            work_top: 0,
            work_width: 1920,
            work_height: 1040,
            dpi: 96,
            dpr: 1.0,
            is_primary: true,
        });

    if cx_dip >= 0.0
        && cx_dip <= primary.dip_width()
        && by_dip >= -100.0
        && by_dip <= primary.dip_height() + 150.0
    {
        primary
    } else {
        monitors
            .iter()
            .min_by_key(|m| {
                let mc_x = m.phys_left as f64 + m.phys_width as f64 * 0.5;
                let mc_y = m.phys_top as f64 + m.phys_height as f64 * 0.5;
                let px = cx_dip * primary.dpr;
                let py = by_dip * primary.dpr;
                let dx = px - mc_x;
                let dy = py - mc_y;
                (dx * dx + dy * dy) as i64
            })
            .copied()
            .unwrap_or(primary)
    }
}

pub fn pick_monitor_for_phys_point(px: i32, py: i32, monitors: &[MonitorMetrics]) -> MonitorMetrics {
    if let Some(m) = monitors.iter().find(|m| {
        px >= m.phys_left
            && px < m.phys_left + m.phys_width
            && py >= m.phys_top
            && py < m.phys_top + m.phys_height
    }) {
        return *m;
    }
    monitors
        .iter()
        .find(|m| m.is_primary)
        .or_else(|| monitors.first())
        .copied()
        .unwrap_or(MonitorMetrics {
            phys_left: 0,
            phys_top: 0,
            phys_width: 1920,
            phys_height: 1080,
            work_left: 0,
            work_top: 0,
            work_width: 1920,
            work_height: 1040,
            dpi: 96,
            dpr: 1.0,
            is_primary: true,
        })
}

/// Converts Electron virtual DIP coordinates for a pet's tight window to physical Win32 pixels.
pub fn resolve_pet_window_rect_px(
    runtime: &PetRuntime,
    monitors: &[MonitorMetrics],
) -> ((i32, i32, i32, i32), f64) {
    let (cx_dip, by_dip) = runtime.geom.core_anchor_dip(&runtime.state);
    let (win_l_dip, win_t_dip, win_w_dip, win_h_dip) =
        runtime.geom.tight_window_dip(cx_dip, by_dip, runtime.margin_frac);

    let target = pick_monitor_for_point(cx_dip, by_dip, monitors);
    let dpr = target.dpr;
    let phys_x = (win_l_dip * dpr).round() as i32;
    let phys_y = (win_t_dip * dpr).round() as i32;
    let phys_w = (win_w_dip * dpr).round().max(32.0) as i32;
    let phys_h = (win_h_dip * dpr).round().max(32.0) as i32;

    ((phys_x, phys_y, phys_w, phys_h), dpr)
}

#[derive(Debug, Clone)]
pub enum PendingUiAction {
    OpenTrayMenu,
    TogglePetMenu(String),
    OverlayHover {
        pet_id: String,
        local_x_dip: f64,
        local_y_dip: f64,
    },
    OverlayClick {
        pet_id: String,
        local_x_dip: f64,
        local_y_dip: f64,
    },
    CloseRadial(String),
    DragEnded(String),
}

pub struct OverlayWindow {
    pub hwnd: HWND,
    pub surface: PetSurface,
    pub rect_px: (i32, i32, i32, i32),
    pub dpr: f64,
    pub visible: bool,
    pub is_transparent: bool,
    pub is_focusable: bool,
    pub always_on_top: bool,
}

pub struct PetWindow {
    pub hwnd: HWND,
    pub runtime: PetRuntime,
    pub surface: PetSurface,
    pub rect_px: (i32, i32, i32, i32),
    pub dpr: f64,
    pub is_transparent: bool,
    pub tracking_leave: bool,
    pub overlay: Option<OverlayWindow>,
}

pub struct SharedAppEvents {
    pub start_time: Instant,
    pub should_quit: bool,
    pub ghost_mode: bool,
    pub dragging_hwnd: Option<isize>,
    pub drag_start_cursor: (i32, i32),
    pub drag_start_win: (i32, i32),
    pub drag_moved: bool,
    pub window_map: HashMap<isize, usize>,
    pub overlay_map: HashMap<isize, String>,
    pub pets: Vec<PetWindow>,
    pub typing_watcher: Option<TypingWatcher>,
    pub key_every: u32,
    pub sound_engine: Option<SoundEngine>,
    pub pending_beat_ignore: bool,
    pub pending_actions: Vec<PendingUiAction>,
}

impl SharedAppEvents {
    pub fn rebuild_window_map(&mut self) {
        self.window_map.clear();
        for (i, pw) in self.pets.iter().enumerate() {
            self.window_map.insert(pw.hwnd.0 as isize, i);
        }
    }
}

const PET_CLASS_NAME: PCWSTR = w!("BugPetRsWindowClass");
const OVERLAY_CLASS_NAME: PCWSTR = w!("BugPetRsOverlayClass");
const TRAY_HOST_CLASS_NAME: PCWSTR = w!("BugPetRsTrayHostClass");

pub fn register_window_class() -> Result<(), String> {
    let hinst = unsafe { GetModuleHandleW(None) }
        .map_err(|e| format!("GetModuleHandleW failed: {e}"))?;
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();

    for (name, proc_fn) in [
        (
            PET_CLASS_NAME,
            pet_wnd_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        ),
        (OVERLAY_CLASS_NAME, overlay_wnd_proc as _),
        (TRAY_HOST_CLASS_NAME, tray_host_wnd_proc as _),
    ] {
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: Default::default(),
            lpfnWndProc: Some(proc_fn),
            cbClsExtra: 0,
            cbWndExtra: size_of::<isize>() as i32,
            hInstance: hinst.into(),
            hIcon: Default::default(),
            hCursor: cursor,
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: name,
            hIconSm: Default::default(),
        };
        let atom = unsafe { RegisterClassExW(&wc) };
        if atom == 0 {
            return Err("RegisterClassExW failed".to_string());
        }
    }
    Ok(())
}

pub fn create_tray_host_window(shared: &Rc<RefCell<SharedAppEvents>>) -> Result<HWND, String> {
    let hinst = unsafe { GetModuleHandleW(None) }
        .map_err(|e| format!("GetModuleHandleW failed: {e}"))?;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            TRAY_HOST_CLASS_NAME,
            w!("bug-pet-rs tray host"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            hinst,
            None,
        )
    }
    .map_err(|e| format!("CreateWindowExW(tray host) failed: {e}"))?;
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(shared) as isize);
    }
    Ok(hwnd)
}

fn apply_click_through(hwnd: HWND, transparent: bool) {
    unsafe {
        let mut ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        ex |= WS_EX_LAYERED.0 as isize;
        if transparent {
            ex |= WS_EX_TRANSPARENT.0 as isize;
        } else {
            ex &= !(WS_EX_TRANSPARENT.0 as isize);
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

pub fn overlay_should_be_transparent(
    radial_visible: bool,
    radial_hot: Option<(f64, f64, f64, f64)>,
    remind_hot: Option<(f64, f64, f64, f64)>,
    local_x_dip: f64,
    local_y_dip: f64,
) -> bool {
    let in_rect = |(hl, ht, hr, hb): (f64, f64, f64, f64)| {
        local_x_dip >= hl && local_x_dip <= hr && local_y_dip >= ht && local_y_dip <= hb
    };
    if radial_visible {
        if let Some(rect) = radial_hot {
            !in_rect(rect)
        } else {
            true
        }
    } else if let Some(rect) = remind_hot {
        !in_rect(rect)
    } else {
        true
    }
}

pub fn create_pet_window(
    pet_state: PetState,
    runtime: PetRuntime,
    gpu: &GpuContext,
    monitors: &[MonitorMetrics],
    shared: &Rc<RefCell<SharedAppEvents>>,
    initially_visible: bool,
) -> Result<PetWindow, String> {
    let (rect_px, dpr) = resolve_pet_window_rect_px(&runtime, monitors);
    let (x, y, w, h) = rect_px;

    let mut ex_style =
        WS_EX_NOREDIRECTIONBITMAP | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    if pet_state.always_on_top {
        ex_style |= WS_EX_TOPMOST;
    }
    let initial_transparent = pet_state.click_through || shared.borrow().ghost_mode;
    if initial_transparent {
        ex_style |= WS_EX_TRANSPARENT;
    }

    let hinst = unsafe { GetModuleHandleW(None) }
        .map_err(|e| format!("GetModuleHandleW failed: {e}"))?;
    let title: Vec<u16> = format!("bug-pet-rs: {}", pet_state.char_id)
        .encode_utf16()
        .chain(Some(0))
        .collect();

    let style = if initially_visible {
        WS_POPUP | WS_VISIBLE
    } else {
        WS_POPUP
    };
    let ctx_ptr = Rc::as_ptr(shared) as isize;
    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            PET_CLASS_NAME,
            PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            w,
            h,
            None,
            None,
            hinst,
            None,
        )
    }
    .map_err(|e| format!("CreateWindowExW failed: {e}"))?;

    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr);
    }

    let surface = PetSurface::new(gpu, hwnd, w as u32, h as u32)?;

    Ok(PetWindow {
        hwnd,
        runtime,
        surface,
        rect_px,
        dpr,
        is_transparent: initial_transparent,
        tracking_leave: false,
        overlay: None,
    })
}

pub fn compute_overlay_rect_px(
    pet_rect_px: (i32, i32, i32, i32),
    dpr: f64,
    monitors: &[MonitorMetrics],
) -> (i32, i32, i32, i32) {
    let pcx = pet_rect_px.0 + pet_rect_px.2 / 2;
    let pcy = pet_rect_px.1 + pet_rect_px.3 / 2;
    let mon = pick_monitor_for_phys_point(pcx, pcy, monitors);
    let max_side_px = (620.0 * dpr).round() as i32;
    let ow = max_side_px.min(mon.work_width.max(320));
    let oh = max_side_px.min(mon.work_height.max(320));
    let min_x = mon.work_left;
    let max_x = (mon.work_left + mon.work_width - ow).max(min_x);
    let min_y = mon.work_top;
    let max_y = (mon.work_top + mon.work_height - oh).max(min_y);
    let ox = (pcx - ow / 2).clamp(min_x, max_x);
    let oy = (pcy - oh / 2).clamp(min_y, max_y);
    (ox, oy, ow.max(64), oh.max(64))
}

pub fn ensure_and_position_overlay(
    pw: &mut PetWindow,
    gpu: &GpuContext,
    monitors: &[MonitorMetrics],
    shared_ptr: isize,
    overlay_map: &mut HashMap<isize, String>,
    want_focus: bool,
    always_on_top: bool,
) -> Result<(), String> {
    let target_rect = compute_overlay_rect_px(pw.rect_px, pw.dpr, monitors);
    let target_topmost = always_on_top || want_focus;
    if pw.overlay.is_none() {
        let hinst = unsafe { GetModuleHandleW(None) }
            .map_err(|e| format!("GetModuleHandleW failed: {e}"))?;
        let mut ex =
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW;
        if target_topmost {
            ex |= WS_EX_TOPMOST;
        }
        if !want_focus {
            ex |= WS_EX_NOACTIVATE;
        }
        let hwnd = unsafe {
            CreateWindowExW(
                ex,
                OVERLAY_CLASS_NAME,
                w!("bug-pet-rs overlay"),
                WS_POPUP,
                target_rect.0,
                target_rect.1,
                target_rect.2,
                target_rect.3,
                None,
                None,
                hinst,
                None,
            )
        }
        .map_err(|e| format!("CreateWindowExW(overlay) failed: {e}"))?;
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, shared_ptr);
        }
        let surface = PetSurface::new(gpu, hwnd, target_rect.2 as u32, target_rect.3 as u32)?;
        overlay_map.insert(hwnd.0 as isize, pw.runtime.id.clone());
        pw.overlay = Some(OverlayWindow {
            hwnd,
            surface,
            rect_px: target_rect,
            dpr: pw.dpr,
            visible: false,
            is_transparent: true,
            is_focusable: want_focus,
            always_on_top: target_topmost,
        });
    }

    if let Some(ref mut ov) = pw.overlay {
        ov.dpr = pw.dpr;
        let insert_after = if target_topmost {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        if ov.rect_px != target_rect {
            ov.rect_px = target_rect;
            ov.surface
                .resize(gpu, target_rect.2 as u32, target_rect.3 as u32)?;
            unsafe {
                let _ = SetWindowPos(
                    ov.hwnd,
                    insert_after,
                    target_rect.0,
                    target_rect.1,
                    target_rect.2,
                    target_rect.3,
                    SWP_NOACTIVATE,
                );
            }
        }
        if ov.is_focusable != want_focus || ov.always_on_top != target_topmost {
            ov.is_focusable = want_focus;
            ov.always_on_top = target_topmost;
            unsafe {
                let mut ex = GetWindowLongPtrW(ov.hwnd, GWL_EXSTYLE);
                if want_focus {
                    ex &= !(WS_EX_NOACTIVATE.0 as isize);
                } else {
                    ex |= WS_EX_NOACTIVATE.0 as isize;
                }
                if target_topmost {
                    ex |= WS_EX_TOPMOST.0 as isize;
                } else {
                    ex &= !(WS_EX_TOPMOST.0 as isize);
                }
                SetWindowLongPtrW(ov.hwnd, GWL_EXSTYLE, ex);
                let _ = SetWindowPos(
                    ov.hwnd,
                    insert_after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
        }
        if !ov.visible {
            ov.visible = true;
            unsafe {
                let _ = ShowWindow(ov.hwnd, if want_focus { SW_SHOW } else { SW_SHOWNOACTIVATE });
                if want_focus {
                    let _ = SetForegroundWindow(ov.hwnd);
                }
            }
        }
    }
    Ok(())
}

pub fn hide_overlay(pw: &mut PetWindow) {
    if let Some(ref mut ov) = pw.overlay {
        if !ov.is_transparent {
            ov.is_transparent = true;
            apply_click_through(ov.hwnd, true);
        }
        if ov.visible {
            ov.visible = false;
            unsafe {
                let _ = ShowWindow(ov.hwnd, SW_HIDE);
            }
        }
    }
}

pub fn set_pet_window_visible(pw: &mut PetWindow, visible: bool) {
    if visible {
        unsafe {
            let _ = ShowWindow(pw.hwnd, SW_SHOWNOACTIVATE);
        }
        set_pet_always_on_top(pw, pw.runtime.state.always_on_top);
    } else {
        hide_overlay(pw);
        unsafe {
            let _ = ShowWindow(pw.hwnd, SW_HIDE);
        }
    }
}

pub fn set_pet_always_on_top(pw: &mut PetWindow, on_top: bool) {
    pw.runtime.state.always_on_top = on_top;
    pw.runtime.radial.always_on_top = on_top;
    unsafe {
        let mut ex = GetWindowLongPtrW(pw.hwnd, GWL_EXSTYLE);
        if on_top {
            ex |= WS_EX_TOPMOST.0 as isize;
        } else {
            ex &= !(WS_EX_TOPMOST.0 as isize);
        }
        SetWindowLongPtrW(pw.hwnd, GWL_EXSTYLE, ex);
        let _ = SetWindowPos(
            pw.hwnd,
            if on_top { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

pub fn move_pet_window_px(pw: &mut PetWindow, new_x: i32, new_y: i32) {
    pw.rect_px.0 = new_x;
    pw.rect_px.1 = new_y;
    unsafe {
        let _ = SetWindowPos(
            pw.hwnd,
            None,
            new_x,
            new_y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn resize_pet_window_to_size(
    pw: &mut PetWindow,
    new_size: f64,
    gpu: &GpuContext,
) -> Result<(), String> {
    let old_size_px = pw.runtime.geom.size * pw.dpr;
    let old_off_y_px = ((pw.runtime.win_h_dip - pw.runtime.geom.size) * 0.5) * pw.dpr;
    let core_cx_px = pw.rect_px.0 as f64 + pw.rect_px.2 as f64 * 0.5;
    let core_by_px = pw.rect_px.1 as f64 + old_off_y_px + old_size_px;

    pw.runtime.set_size(new_size);

    let new_w = (pw.runtime.win_w_dip * pw.dpr).round().max(64.0) as i32;
    let new_h = (pw.runtime.win_h_dip * pw.dpr).round().max(64.0) as i32;
    let new_size_px = pw.runtime.geom.size * pw.dpr;
    let new_off_y_px = ((pw.runtime.win_h_dip - pw.runtime.geom.size) * 0.5) * pw.dpr;
    let new_x = (core_cx_px - new_w as f64 * 0.5).round() as i32;
    let new_y = (core_by_px - new_off_y_px - new_size_px).round() as i32;

    pw.surface.resize(gpu, new_w as u32, new_h as u32)?;
    pw.rect_px = (new_x, new_y, new_w, new_h);
    unsafe {
        let _ = SetWindowPos(
            pw.hwnd,
            None,
            new_x,
            new_y,
            new_w,
            new_h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    Ok(())
}

pub fn sync_click_through(shared: &Rc<RefCell<SharedAppEvents>>) {
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return;
    }

    let mut st = shared.borrow_mut();
    let dragging_hwnd = st.dragging_hwnd;
    let ghost = st.ghost_mode;
    for pw in &mut st.pets {
        let should_transparent = if ghost {
            true
        } else if Some(pw.hwnd.0 as isize) == dragging_hwnd {
            false
        } else if !pw.runtime.state.click_through {
            false
        } else {
            let (wx, wy, ww, wh) = pw.rect_px;
            if pt.x < wx || pt.x >= wx + ww || pt.y < wy || pt.y >= wy + wh {
                true
            } else {
                !pw.runtime.hit_test_screen((pt.x, pt.y), pw.rect_px, pw.dpr)
            }
        };

        if should_transparent != pw.is_transparent {
            pw.is_transparent = should_transparent;
            apply_click_through(pw.hwnd, should_transparent);
        }

        // Hit-test overlay click-through (transparent except over radial items or active remind hot rect)
        if let Some(ref mut ov) = pw.overlay {
            if ov.visible {
                let lx = (pt.x - ov.rect_px.0) as f64 / ov.dpr;
                let ly = (pt.y - ov.rect_px.1) as f64 / ov.dpr;
                let ov_transparent = overlay_should_be_transparent(
                    pw.runtime.radial.is_visible(),
                    pw.runtime.radial.hot_rect_dip(),
                    pw.runtime.active_remind.as_ref().map(|r| r.hot_rect_local_dip),
                    lx,
                    ly,
                );
                if ov_transparent != ov.is_transparent {
                    ov.is_transparent = ov_transparent;
                    apply_click_through(ov.hwnd, ov_transparent);
                }
            }
        }
    }
}

pub fn destroy_pet_window(pw: &mut PetWindow, overlay_map: &mut HashMap<isize, String>) {
    if let Some(ov) = pw.overlay.take() {
        overlay_map.remove(&(ov.hwnd.0 as isize));
        unsafe {
            let _ = DestroyWindow(ov.hwnd);
        }
    }
    unsafe {
        let _ = DestroyWindow(pw.hwnd);
    }
}

pub fn destroy_all_windows(shared: &Rc<RefCell<SharedAppEvents>>, tray_hwnd: Option<HWND>) {
    let mut st = shared.borrow_mut();
    let mut ov_map = std::mem::take(&mut st.overlay_map);
    for pw in &mut st.pets {
        destroy_pet_window(pw, &mut ov_map);
    }
    st.pets.clear();
    st.window_map.clear();
    if let Some(th) = tray_hwnd {
        unsafe {
            let _ = DestroyWindow(th);
        }
    }
}

unsafe extern "system" fn tray_host_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<SharedAppEvents>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let shared = &*ptr;

    if msg == WM_TRAYICON {
        let evt = (lparam.0 & 0xFFFF) as u32;
        if evt == WM_LBUTTONUP || evt == WM_RBUTTONUP {
            if let Ok(mut st) = shared.try_borrow_mut() {
                st.pending_actions.push(PendingUiAction::OpenTrayMenu);
            }
        }
        return LRESULT(0);
    }

    if msg == WM_INPUT {
        if let Ok(mut st) = shared.try_borrow_mut() {
            let now_s = st.start_time.elapsed().as_secs_f64();
            let key_every = st.key_every;
            let triggered = st
                .typing_watcher
                .as_mut()
                .is_some_and(|w| w.handle_wm_input(lparam.0, key_every, now_s));
            if triggered {
                for pw in &mut st.pets {
                    if pw.runtime.state.typing {
                        pw.runtime.on_key(now_s);
                    }
                }
            }
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<SharedAppEvents>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let shared = &*ptr;

    match msg {
        WM_SETCURSOR => {
            if (lparam.0 as u32 & 0xFFFF) == HTCLIENT {
                let mut show_hand = false;
                let mut pt = POINT::default();
                if GetCursorPos(&mut pt).is_ok() {
                    if let Ok(st) = shared.try_borrow() {
                        if let Some(pet_id) = st.overlay_map.get(&(hwnd.0 as isize)) {
                            if let Some(pw) = st.pets.iter().find(|p| p.runtime.id == *pet_id) {
                                if let Some(ref ov) = pw.overlay {
                                    let lx = (pt.x - ov.rect_px.0) as f64 / ov.dpr;
                                    let ly = (pt.y - ov.rect_px.1) as f64 / ov.dpr;
                                    show_hand = (pw.runtime.radial.is_visible()
                                        && pw.runtime.radial.hit_test(lx, ly).is_some())
                                        || pw.runtime.active_remind.as_ref().is_some_and(|r| {
                                            let (l, t, ri, b) = r.hot_rect_local_dip;
                                            lx >= l && lx <= ri && ly >= t && ly <= b
                                        });
                                }
                            }
                        }
                    }
                }
                if show_hand {
                    if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(hand);
                        return LRESULT(1);
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSEMOVE => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                if let Some(pet_id) = st.overlay_map.get(&(hwnd.0 as isize)).cloned() {
                    let x_px = (lparam.0 & 0xFFFF) as i16 as f64;
                    let y_px = ((lparam.0 >> 16) & 0xFFFF) as i16 as f64;
                    let dpr = st
                        .pets
                        .iter()
                        .find(|p| p.runtime.id == pet_id)
                        .map(|p| p.dpr)
                        .unwrap_or(1.0);
                    st.pending_actions.push(PendingUiAction::OverlayHover {
                        pet_id,
                        local_x_dip: x_px / dpr,
                        local_y_dip: y_px / dpr,
                    });
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                if let Some(pet_id) = st.overlay_map.get(&(hwnd.0 as isize)).cloned() {
                    let x_px = (lparam.0 & 0xFFFF) as i16 as f64;
                    let y_px = ((lparam.0 >> 16) & 0xFFFF) as i16 as f64;
                    let dpr = st
                        .pets
                        .iter()
                        .find(|p| p.runtime.id == pet_id)
                        .map(|p| p.dpr)
                        .unwrap_or(1.0);
                    st.pending_actions.push(PendingUiAction::OverlayClick {
                        pet_id,
                        local_x_dip: x_px / dpr,
                        local_y_dip: y_px / dpr,
                    });
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                if let Some(pet_id) = st.overlay_map.get(&(hwnd.0 as isize)).cloned() {
                    st.pending_actions.push(PendingUiAction::CloseRadial(pet_id));
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 as u16 == VK_ESCAPE.0 {
                if let Ok(mut st) = shared.try_borrow_mut() {
                    if let Some(pet_id) = st.overlay_map.get(&(hwnd.0 as isize)).cloned() {
                        st.pending_actions.push(PendingUiAction::CloseRadial(pet_id));
                    }
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                if let Ok(mut st) = shared.try_borrow_mut() {
                    if let Some(pet_id) = st.overlay_map.get(&(hwnd.0 as isize)).cloned() {
                        st.pending_actions.push(PendingUiAction::CloseRadial(pet_id));
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn pet_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<SharedAppEvents>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let shared = &*ptr;

    match msg {
        WM_SETCURSOR => {
            if (lparam.0 as u32 & 0xFFFF) == HTCLIENT {
                if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                    SetCursor(hand);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                let now_s = st.start_time.elapsed().as_secs_f64();
                let key = hwnd.0 as isize;
                if let Some(&idx) = st.window_map.get(&key) {
                    if st.pets[idx].runtime.radial.is_visible() {
                        let pid = st.pets[idx].runtime.id.clone();
                        st.pending_actions.push(PendingUiAction::CloseRadial(pid));
                        return LRESULT(0);
                    }
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let (sound_params, win_pos, locked) = {
                        let pw = &mut st.pets[idx];
                        pw.runtime.press(now_s);
                        let sp = pw
                            .runtime
                            .click_sound_params()
                            .map(|(k, v)| (k.to_string(), v));
                        (sp, (pw.rect_px.0, pw.rect_px.1), pw.runtime.state.lock)
                    };
                    if !locked {
                        st.dragging_hwnd = Some(key);
                        st.drag_start_cursor = (pt.x, pt.y);
                        st.drag_start_win = win_pos;
                        st.drag_moved = false;
                        SetCapture(hwnd);
                    }
                    if let Some((kind, vol)) = sound_params {
                        st.pending_beat_ignore = true;
                        if let Some(ref se) = st.sound_engine {
                            se.play_click(&kind, false, vol, 0.0);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                let now_s = st.start_time.elapsed().as_secs_f64();
                let key = hwnd.0 as isize;
                if st.dragging_hwnd == Some(key) {
                    let mut pt = POINT::default();
                    if GetCursorPos(&mut pt).is_ok() {
                        let dx = pt.x - st.drag_start_cursor.0;
                        let dy = pt.y - st.drag_start_cursor.1;
                        if !st.drag_moved {
                            let slop_px = if let Some(&idx) = st.window_map.get(&key) {
                                (4.0 * st.pets[idx].dpr).round() as i32
                            } else {
                                4
                            };
                            if dx.abs() + dy.abs() >= slop_px {
                                st.drag_moved = true;
                                let scx = st.drag_start_cursor.0;
                                let scy = st.drag_start_cursor.1;
                                if let Some(&idx) = st.window_map.get(&key) {
                                    let dpr = st.pets[idx].dpr;
                                    st.pets[idx].runtime.begin_drag(
                                        scx as f64 / dpr,
                                        scy as f64 / dpr,
                                        now_s,
                                    );
                                }
                            }
                        }
                        if st.drag_moved {
                            let new_x = st.drag_start_win.0 + dx;
                            let new_y = st.drag_start_win.1 + dy;
                            if let Some(&idx) = st.window_map.get(&key) {
                                let pw = &mut st.pets[idx];
                                pw.rect_px.0 = new_x;
                                pw.rect_px.1 = new_y;
                                pw.runtime.drag_move(
                                    pt.x as f64 / pw.dpr,
                                    pt.y as f64 / pw.dpr,
                                    now_s,
                                );
                                let _ = SetWindowPos(
                                    hwnd,
                                    None,
                                    new_x,
                                    new_y,
                                    0,
                                    0,
                                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                                );
                            }
                        }
                    }
                } else if let Some(&idx) = st.window_map.get(&key) {
                    let pw = &mut st.pets[idx];
                    if !pw.tracking_leave {
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        if TrackMouseEvent(&mut tme).is_ok() {
                            pw.tracking_leave = true;
                        }
                    }
                    let x_px = (lparam.0 & 0xFFFF) as i16 as f64;
                    let y_px = ((lparam.0 >> 16) & 0xFFFF) as i16 as f64;
                    let dpr = pw.dpr;
                    pw.runtime.on_pet_hover(x_px / dpr, y_px / dpr, now_s);
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                let now_s = st.start_time.elapsed().as_secs_f64();
                let key = hwnd.0 as isize;
                if let Some(&idx) = st.window_map.get(&key) {
                    st.pets[idx].tracking_leave = false;
                    st.pets[idx].runtime.on_pet_leave(now_s);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                let now_s = st.start_time.elapsed().as_secs_f64();
                let key = hwnd.0 as isize;
                let was_dragging_this = st.dragging_hwnd == Some(key);
                let was_moved = st.drag_moved;
                if was_dragging_this {
                    st.dragging_hwnd = None;
                    st.drag_moved = false;
                    if msg == WM_LBUTTONUP {
                        let _ = ReleaseCapture();
                    }
                }
                if let Some(&idx) = st.window_map.get(&key) {
                    let released_up = if was_dragging_this {
                        st.pets[idx].runtime.end_drag(now_s, was_moved)
                    } else {
                        st.pets[idx].runtime.release(now_s)
                    };
                    if was_dragging_this && was_moved {
                        let pid = st.pets[idx].runtime.id.clone();
                        st.pending_actions.push(PendingUiAction::DragEnded(pid));
                    }
                    if released_up {
                        let sp = st.pets[idx]
                            .runtime
                            .click_sound_params()
                            .map(|(k, v)| (k.to_string(), v));
                        if let Some((kind, vol)) = sp {
                            if let Some(ref se) = st.sound_engine {
                                se.play_click(&kind, true, vol, 0.0);
                            }
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                let key = hwnd.0 as isize;
                if let Some(&idx) = st.window_map.get(&key) {
                    let pid = st.pets[idx].runtime.id.clone();
                    st.pending_actions.push(PendingUiAction::TogglePetMenu(pid));
                }
            }
            LRESULT(0)
        }
        WM_INPUT => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                let now_s = st.start_time.elapsed().as_secs_f64();
                let key_every = st.key_every;
                let triggered = st
                    .typing_watcher
                    .as_mut()
                    .is_some_and(|w| w.handle_wm_input(lparam.0, key_every, now_s));
                if triggered {
                    for pw in &mut st.pets {
                        if pw.runtime.state.typing {
                            pw.runtime.on_key(now_s);
                        }
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_KEYDOWN => {
            if wparam.0 as u16 == VK_ESCAPE.0 {
                if let Ok(mut st) = shared.try_borrow_mut() {
                    st.should_quit = true;
                }
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if let Ok(mut st) = shared.try_borrow_mut() {
                st.should_quit = true;
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_hit_test_is_always_click_through_for_speech_bubbles() {
        assert!(overlay_should_be_transparent(
            false,
            None,
            None,
            310.0,
            310.0
        ));
        assert!(overlay_should_be_transparent(
            true,
            None,
            Some((10.0, 10.0, 50.0, 50.0)),
            20.0,
            20.0
        ));
        assert!(!overlay_should_be_transparent(
            true,
            Some((100.0, 100.0, 260.0, 260.0)),
            None,
            150.0,
            150.0
        ));
        assert!(overlay_should_be_transparent(
            true,
            Some((100.0, 100.0, 260.0, 260.0)),
            None,
            10.0,
            10.0
        ));
        assert!(!overlay_should_be_transparent(
            false,
            None,
            Some((200.0, 80.0, 340.0, 140.0)),
            250.0,
            100.0
        ));
        assert!(overlay_should_be_transparent(
            false,
            None,
            Some((200.0, 80.0, 340.0, 140.0)),
            50.0,
            100.0
        ));
    }

    #[test]
    fn click_through_toggle_preserves_layered_and_toggles_transparent() {
        let hinst = unsafe { GetModuleHandleW(None) }.expect("hinst");
        let ex = WS_EX_NOREDIRECTIONBITMAP
            | WS_EX_LAYERED
            | WS_EX_TRANSPARENT
            | WS_EX_TOOLWINDOW
            | WS_EX_NOACTIVATE;
        let hwnd = unsafe {
            CreateWindowExW(
                ex,
                w!("STATIC"),
                w!("bug-pet-rs test"),
                WS_POPUP,
                0,
                0,
                64,
                64,
                None,
                None,
                hinst,
                None,
            )
        }
        .expect("test window");

        unsafe {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)
                .expect("SetLayeredWindowAttributes");
        }

        let initial_ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        assert_ne!(initial_ex & (WS_EX_LAYERED.0 as isize), 0);
        assert_ne!(initial_ex & (WS_EX_TRANSPARENT.0 as isize), 0);

        apply_click_through(hwnd, false);
        let solid_ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        assert_ne!(solid_ex & (WS_EX_LAYERED.0 as isize), 0);
        assert_eq!(solid_ex & (WS_EX_TRANSPARENT.0 as isize), 0);

        apply_click_through(hwnd, true);
        let ct_ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        assert_ne!(ct_ex & (WS_EX_LAYERED.0 as isize), 0);
        assert_ne!(ct_ex & (WS_EX_TRANSPARENT.0 as isize), 0);

        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}
