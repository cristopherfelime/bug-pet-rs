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
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
};
use windows::Win32::UI::Shell::{
    AppVisibility, IAppVisibility, SHAppBarMessage, ABM_GETTASKBARPOS, APPBARDATA,
};

const WM_MOUSELEAVE: u32 = 0x02A3;
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, EnumWindows, FindWindowExW, FindWindowW,
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, LoadCursorW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SetCursor, SetForegroundWindow,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE,
    GWLP_USERDATA, GW_HWNDPREV, HTCLIENT, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IDC_ARROW,
    IDC_HAND, LWA_ALPHA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
    SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE, WA_INACTIVE, WINDOW_EX_STYLE,
    WM_ACTIVATE, WM_CAPTURECHANGED, WM_CLOSE, WM_DISPLAYCHANGE, WM_INPUT, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WM_SETCURSOR, WM_SETTINGCHANGE,
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

pub fn pick_monitor_idx_for_phys_point(px: i32, py: i32, monitors: &[MonitorMetrics]) -> usize {
    if let Some((i, _)) = monitors.iter().enumerate().find(|(_, m)| {
        px >= m.phys_left
            && px < m.phys_left + m.phys_width
            && py >= m.phys_top
            && py < m.phys_top + m.phys_height
    }) {
        return i;
    }
    monitors
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| {
            let cx = m.phys_left as i64 + (m.phys_width as i64) / 2;
            let cy = m.phys_top as i64 + (m.phys_height as i64) / 2;
            let dx = px as i64 - cx;
            let dy = py as i64 - cy;
            dx * dx + dy * dy
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

pub fn pick_monitor_for_phys_point(px: i32, py: i32, monitors: &[MonitorMetrics]) -> MonitorMetrics {
    if monitors.is_empty() {
        return MonitorMetrics {
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
        };
    }
    monitors[pick_monitor_idx_for_phys_point(px, py, monitors)]
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
    DisplayChanged,
    TaskbarRecreated,
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
    pub smooth_y_px: Option<f64>,
    pub z_retry_after: Option<Instant>,
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
    let tb_msg = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED_MSG.store(tb_msg, Ordering::Relaxed);
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
        smooth_y_px: None,
        z_retry_after: None,
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
    pw.smooth_y_px = None;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskbarSlice {
    pub hwnd: isize,
    pub monitor_idx: usize,
    pub top_px: i32,
    pub mon_bottom_px: i32,
    pub is_shown: bool,
    pub is_primary: bool,
    pub win_rect: (i32, i32, i32, i32),
}

pub fn choose_taskbar_rect(win_rc: Option<RECT>, dwm_rc: Option<RECT>) -> Option<RECT> {
    let valid = |r: &RECT| r.right > r.left && r.bottom > r.top;
    if let Some(r) = win_rc.filter(valid) {
        Some(r)
    } else {
        dwm_rc.filter(valid)
    }
}

pub fn resolve_taskbar_top_px(rc: RECT, visible: bool, mon_top: i32, mon_bottom: i32) -> (i32, bool) {
    if !visible || rc.right <= rc.left || rc.bottom <= rc.top {
        return (mon_bottom, false);
    }
    let max_tb_h = ((mon_bottom - mon_top) / 3).max(200);
    if rc.top >= mon_bottom - 4 {
        return (mon_bottom, false);
    }
    if rc.bottom >= mon_bottom - 48 && rc.top >= mon_bottom - max_tb_h {
        return (rc.top.clamp(mon_bottom - max_tb_h, mon_bottom), true);
    }
    (mon_bottom, false)
}

unsafe extern "system" fn enum_taskbar_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buf = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut buf);
    if len > 0 {
        let cls = String::from_utf16_lossy(&buf[..len as usize]);
        if cls == "Shell_TrayWnd" || cls == "Shell_SecondaryTrayWnd" {
            let list = &mut *(lparam.0 as *mut Vec<(HWND, bool)>);
            list.push((hwnd, cls == "Shell_TrayWnd"));
        }
    }
    BOOL(1)
}

fn monitor_idx_for_hwnd(h: HWND, rc: RECT, monitors: &[MonitorMetrics]) -> usize {
    let hmon = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
    if !hmon.0.is_null() {
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(hmon, &mut mi).as_bool() } {
            let mw = mi.rcMonitor.right - mi.rcMonitor.left;
            let mh = mi.rcMonitor.bottom - mi.rcMonitor.top;
            if let Some((i, _)) = monitors.iter().enumerate().find(|(_, m)| {
                m.phys_left == mi.rcMonitor.left
                    && m.phys_top == mi.rcMonitor.top
                    && m.phys_width == mw
                    && m.phys_height == mh
            }) {
                return i;
            }
        }
    }
    let cx = rc.left + (rc.right - rc.left) / 2;
    let cy = rc.top + (rc.bottom - rc.top) / 2;
    pick_monitor_idx_for_phys_point(cx, cy, monitors)
}

pub fn is_shell_taskbar_flyout_exe(exe_name_lower: &str) -> bool {
    matches!(
        exe_name_lower,
        "startmenuexperiencehost.exe"
            | "searchhost.exe"
            | "searchapp.exe"
            | "shellhost.exe"
            | "shellexperiencehost.exe"
    )
}

pub fn merge_taskbar_hwnd(list: &mut Vec<(HWND, bool)>, hwnd: HWND, is_primary: bool) {
    if hwnd.0.is_null() {
        return;
    }
    if let Some((_, existing_pri)) = list.iter_mut().find(|(h, _)| *h == hwnd) {
        *existing_pri |= is_primary;
    } else {
        list.push((hwnd, is_primary));
    }
}

pub fn estimate_shown_taskbar_top_px(
    mon_top: i32,
    mon_bottom: i32,
    dpr: f64,
    last_shown_h: Option<i32>,
    raw_rc: RECT,
    appbar_h: Option<i32>,
) -> i32 {
    let max_tb_h = ((mon_bottom - mon_top) / 3).max(200);
    let raw_h = raw_rc.bottom - raw_rc.top;
    let fallback_h = (48.0 * dpr.max(1.0)).round() as i32;
    let chosen_h = [last_shown_h, appbar_h, Some(raw_h)]
        .into_iter()
        .flatten()
        .find(|&h| (24..=max_tb_h).contains(&h))
        .unwrap_or(fallback_h);
    (mon_bottom - chosen_h).clamp(mon_bottom - max_tb_h, mon_bottom)
}

fn query_primary_appbar_height_px(tb_hwnd: HWND) -> Option<i32> {
    if tb_hwnd.0.is_null() {
        return None;
    }
    let mut abd = APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: tb_hwnd,
        ..Default::default()
    };
    let ok = unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd) };
    if ok != 0 {
        let h = abd.rc.bottom - abd.rc.top;
        if h > 0 {
            return Some(h);
        }
    }
    None
}

fn hwnd_process_basename_lower(hwnd: HWND) -> Option<String> {
    let mut pid: u32 = 0;
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .is_ok();
    unsafe {
        let _ = CloseHandle(handle);
    }
    if !ok || len == 0 {
        return None;
    }
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    let base = full.rsplit(['\\', '/']).next().unwrap_or(&full);
    Some(base.to_ascii_lowercase())
}

fn is_hwnd_uncloaked_visible(hwnd: HWND) -> bool {
    if hwnd.0.is_null() || !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return false;
    }
    let mut cloaked: u32 = 0;
    if unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            size_of::<u32>() as u32,
        )
    }
    .is_ok()
        && cloaked != 0
    {
        return false;
    }
    let mut rc = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rc) }.is_ok() && rc.right > rc.left && rc.bottom > rc.top
}

pub struct TaskbarTracker {
    cached_hwnds: Vec<(HWND, bool)>,
    last_scan: Option<Instant>,
    app_visibility: Option<IAppVisibility>,
    app_vis_attempted: bool,
    last_launcher_check: Option<Instant>,
    last_launcher_visible: bool,
    last_fg_hwnd: isize,
    last_fg_is_flyout: bool,
    last_shown_heights: HashMap<usize, i32>,
    last_flyout_active: bool,
}

impl TaskbarTracker {
    pub fn new() -> Self {
        Self {
            cached_hwnds: Vec::new(),
            last_scan: None,
            app_visibility: None,
            app_vis_attempted: false,
            last_launcher_check: None,
            last_launcher_visible: false,
            last_fg_hwnd: 0,
            last_fg_is_flyout: false,
            last_shown_heights: HashMap::new(),
            last_flyout_active: false,
        }
    }

    pub fn invalidate(&mut self) {
        self.cached_hwnds.clear();
        self.last_scan = None;
        self.last_shown_heights.clear();
        self.last_launcher_check = None;
        self.last_fg_hwnd = 0;
        self.last_fg_is_flyout = false;
    }

    pub fn shell_flyout_active(&self) -> bool {
        self.last_flyout_active
    }

    pub fn launcher_visible(&self) -> bool {
        self.last_launcher_visible
    }

    fn detect_shell_flyout_monitor(
        &mut self,
        monitors: &[MonitorMetrics],
        now: Instant,
    ) -> (Option<usize>, bool) {
        let mut fg_mon = None;
        let fg = unsafe { GetForegroundWindow() };
        if !fg.0.is_null() {
            let fg_id = fg.0 as isize;
            if fg_id != self.last_fg_hwnd {
                self.last_fg_hwnd = fg_id;
                self.last_fg_is_flyout = hwnd_process_basename_lower(fg)
                    .as_deref()
                    .is_some_and(is_shell_taskbar_flyout_exe);
            }
            if self.last_fg_is_flyout && is_hwnd_uncloaked_visible(fg) {
                let mut fg_rc = RECT::default();
                if unsafe { GetWindowRect(fg, &mut fg_rc) }.is_ok() {
                    fg_mon = Some(monitor_idx_for_hwnd(fg, fg_rc, monitors));
                }
            }
        } else {
            self.last_fg_hwnd = 0;
            self.last_fg_is_flyout = false;
        }

        if !self.app_vis_attempted {
            self.app_vis_attempted = true;
            self.app_visibility = unsafe { CoCreateInstance(&AppVisibility, None, CLSCTX_ALL) }.ok();
        }
        if self
            .last_launcher_check
            .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_millis(100))
        {
            self.last_launcher_check = Some(now);
            self.last_launcher_visible = self
                .app_visibility
                .as_ref()
                .and_then(|av| unsafe { av.IsLauncherVisible() }.ok())
                .is_some_and(|b| b.as_bool());
        }

        (fg_mon, self.last_launcher_visible)
    }

    fn refresh_if_needed(&mut self, now: Instant) {
        self.cached_hwnds
            .retain(|(h, _)| unsafe { IsWindow(*h).as_bool() });
        let has_primary = self.cached_hwnds.iter().any(|(_, pri)| *pri);
        let stale = self
            .last_scan
            .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(2));
        if !stale && !self.cached_hwnds.is_empty() && has_primary {
            return;
        }
        if let Ok(primary) = unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) } {
            merge_taskbar_hwnd(&mut self.cached_hwnds, primary, true);
        }
        let mut prev = HWND::default();
        for _ in 0..16 {
            let Ok(next) = (unsafe {
                FindWindowExW(
                    HWND::default(),
                    prev,
                    w!("Shell_SecondaryTrayWnd"),
                    PCWSTR::null(),
                )
            }) else {
                break;
            };
            if next.0.is_null() || next == prev {
                break;
            }
            merge_taskbar_hwnd(&mut self.cached_hwnds, next, false);
            prev = next;
        }
        let mut discovered: Vec<(HWND, bool)> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(enum_taskbar_proc),
                LPARAM(&mut discovered as *mut Vec<(HWND, bool)> as isize),
            );
        }
        for (h, pri) in discovered {
            merge_taskbar_hwnd(&mut self.cached_hwnds, h, pri);
        }
        self.last_scan = Some(now);
    }

    pub fn sample(&mut self, monitors: &[MonitorMetrics], now: Instant) -> Vec<TaskbarSlice> {
        self.refresh_if_needed(now);
        let mut out = Vec::with_capacity(self.cached_hwnds.len());
        if monitors.is_empty() {
            return out;
        }
        for &(h, is_primary) in &self.cached_hwnds {
            let mut visible = unsafe { IsWindowVisible(h).as_bool() };
            if visible {
                let mut cloaked: u32 = 0;
                if unsafe {
                    DwmGetWindowAttribute(
                        h,
                        DWMWA_CLOAKED,
                        &mut cloaked as *mut u32 as *mut _,
                        size_of::<u32>() as u32,
                    )
                }
                .is_ok()
                    && cloaked != 0
                {
                    visible = false;
                }
            }
            let mut win_rc_raw = RECT::default();
            let win_rc = unsafe { GetWindowRect(h, &mut win_rc_raw) }
                .is_ok()
                .then_some(win_rc_raw);
            let mut dwm_rc_raw = RECT::default();
            let dwm_rc = unsafe {
                DwmGetWindowAttribute(
                    h,
                    DWMWA_EXTENDED_FRAME_BOUNDS,
                    &mut dwm_rc_raw as *mut RECT as *mut _,
                    size_of::<RECT>() as u32,
                )
            }
            .is_ok()
            .then_some(dwm_rc_raw);
            let Some(rc) = choose_taskbar_rect(win_rc, dwm_rc) else {
                continue;
            };
            let mon_idx = monitor_idx_for_hwnd(h, rc, monitors);
            let mon = &monitors[mon_idx];
            let mon_bottom = mon.phys_top + mon.phys_height;
            let (top_px, is_shown) = resolve_taskbar_top_px(rc, visible, mon.phys_top, mon_bottom);
            if is_shown {
                let h_px = mon_bottom - top_px;
                if h_px >= 24 {
                    self.last_shown_heights.insert(mon_idx, h_px);
                }
            }
            out.push(TaskbarSlice {
                hwnd: h.0 as isize,
                monitor_idx: mon_idx,
                top_px,
                mon_bottom_px: mon_bottom,
                is_shown,
                is_primary,
                win_rect: (rc.left, rc.top, rc.right, rc.bottom),
            });
        }

        let (fg_mon, launcher_vis) = self.detect_shell_flyout_monitor(monitors, now);
        self.last_flyout_active = fg_mon.is_some() || launcher_vis;
        let forced_mon = fg_mon.or_else(|| {
            if launcher_vis {
                out.iter()
                    .find(|s| s.is_primary)
                    .map(|s| s.monitor_idx)
                    .or_else(|| monitors.iter().position(|m| m.is_primary))
                    .or(Some(0))
            } else {
                None
            }
        });
        if let Some(target_mon) = forced_mon {
            let last_h = self.last_shown_heights.get(&target_mon).copied();
            for sl in &mut out {
                if sl.monitor_idx == target_mon && !sl.is_shown {
                    let mon = &monitors[sl.monitor_idx];
                    let appbar_h = if sl.is_primary {
                        query_primary_appbar_height_px(HWND(sl.hwnd as *mut _))
                    } else {
                        None
                    };
                    let raw_rc = RECT {
                        left: sl.win_rect.0,
                        top: sl.win_rect.1,
                        right: sl.win_rect.2,
                        bottom: sl.win_rect.3,
                    };
                    sl.top_px = estimate_shown_taskbar_top_px(
                        mon.phys_top,
                        sl.mon_bottom_px,
                        mon.dpr,
                        last_h,
                        raw_rc,
                        appbar_h,
                    );
                    sl.is_shown = true;
                }
            }
        }

        out.sort_by_key(|s| (s.monitor_idx, !s.is_shown, s.top_px, !s.is_primary, s.hwnd));
        out
    }
}

pub fn monitor_taskbar_top_px(
    slices: &[TaskbarSlice],
    monitor_idx: usize,
    mon: &MonitorMetrics,
) -> (i32, Option<TaskbarSlice>) {
    if let Some(sl) = slices
        .iter()
        .filter(|s| s.monitor_idx == monitor_idx)
        .min_by_key(|s| (!s.is_shown, s.top_px, !s.is_primary))
    {
        (sl.top_px, Some(*sl))
    } else {
        (mon.work_top + mon.work_height, None)
    }
}

pub fn is_window_above(candidate: HWND, reference: HWND) -> bool {
    if candidate.0.is_null() || reference.0.is_null() || candidate == reference {
        return false;
    }
    let mut cur = reference;
    for _ in 0..1024 {
        let Ok(prev) = (unsafe { GetWindow(cur, GW_HWNDPREV) }) else {
            break;
        };
        if prev.0.is_null() {
            break;
        }
        if prev == candidate {
            return true;
        }
        cur = prev;
    }
    false
}

pub fn ensure_pet_above_taskbar(
    pw: &mut PetWindow,
    tb_hwnd: HWND,
    now: Instant,
    force_now: bool,
) -> bool {
    if tb_hwnd.0.is_null() {
        pw.z_retry_after = None;
        return false;
    }
    let pet_lost_topmost =
        unsafe { (GetWindowLongPtrW(pw.hwnd, GWL_EXSTYLE) & (WS_EX_TOPMOST.0 as isize)) == 0 };
    let pet_needs = pet_lost_topmost || is_window_above(tb_hwnd, pw.hwnd);

    let mut ov_hwnd_to_raise: Option<HWND> = None;
    if let Some(ref ov) = pw.overlay {
        if ov.visible {
            let ov_lost_topmost = unsafe {
                (GetWindowLongPtrW(ov.hwnd, GWL_EXSTYLE) & (WS_EX_TOPMOST.0 as isize)) == 0
            };
            if ov_lost_topmost
                || is_window_above(tb_hwnd, ov.hwnd)
                || is_window_above(pw.hwnd, ov.hwnd)
            {
                ov_hwnd_to_raise = Some(ov.hwnd);
            }
        }
    }

    if !pet_needs && ov_hwnd_to_raise.is_none() {
        pw.z_retry_after = None;
        return false;
    }
    if !force_now && pw.z_retry_after.is_some_and(|t| now < t) {
        return false;
    }

    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    if pet_needs {
        unsafe {
            let _ = SetWindowPos(pw.hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
        }
        if is_window_above(tb_hwnd, pw.hwnd) {
            unsafe {
                let _ = SetWindowPos(pw.hwnd, HWND_TOP, 0, 0, 0, 0, flags);
            }
        }
    }
    if let Some(ov_hwnd) = ov_hwnd_to_raise {
        unsafe {
            let _ = SetWindowPos(ov_hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
        }
        if is_window_above(tb_hwnd, ov_hwnd) || is_window_above(pw.hwnd, ov_hwnd) {
            unsafe {
                let _ = SetWindowPos(ov_hwnd, HWND_TOP, 0, 0, 0, 0, flags);
            }
        }
    }

    if is_window_above(tb_hwnd, pw.hwnd) {
        pw.z_retry_after = Some(now + Duration::from_millis(250));
    } else {
        pw.z_retry_after = None;
    }
    true
}

pub fn pet_core_bottom_px(
    pw_rect_y: i32,
    win_h_dip: f64,
    geom_size_dip: f64,
    dpr: f64,
) -> f64 {
    let off_y_dip = (win_h_dip - geom_size_dip) * 0.5;
    pw_rect_y as f64 + (off_y_dip + geom_size_dip) * dpr
}

pub fn pet_win_y_for_taskbar_px(
    taskbar_top_px: i32,
    offset_dip: f64,
    win_h_dip: f64,
    geom_size_dip: f64,
    dpr: f64,
) -> f64 {
    let off_y_dip = (win_h_dip - geom_size_dip) * 0.5;
    taskbar_top_px as f64 + (offset_dip - geom_size_dip - off_y_dip) * dpr
}

pub fn initial_taskbar_offset_dip(core_bottom_px: f64, taskbar_top_px: i32, dpr: f64) -> f64 {
    let raw = (core_bottom_px - taskbar_top_px as f64) / dpr.max(0.5);
    if (-120.0..=120.0).contains(&raw) {
        (raw * 10.0).round() / 10.0
    } else {
        0.0
    }
}

pub fn step_smooth_y(current: f64, target: f64, dt_s: f64) -> f64 {
    if (target - current).abs() <= 0.75 {
        target
    } else {
        let alpha = 1.0 - (-dt_s.clamp(0.001, 0.05) * 32.0).exp();
        current + (target - current) * alpha
    }
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

    if msg == WM_DISPLAYCHANGE || msg == WM_SETTINGCHANGE {
        if let Ok(mut st) = shared.try_borrow_mut() {
            if !matches!(st.pending_actions.last(), Some(PendingUiAction::DisplayChanged)) {
                st.pending_actions.push(PendingUiAction::DisplayChanged);
            }
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    let tb_msg = TASKBAR_CREATED_MSG.load(Ordering::Relaxed);
    if tb_msg != 0 && msg == tb_msg {
        if let Ok(mut st) = shared.try_borrow_mut() {
            st.pending_actions.push(PendingUiAction::TaskbarRecreated);
        }
        return LRESULT(0);
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

    #[test]
    fn choose_taskbar_rect_prefers_non_empty_window_rect_and_rejects_zeros() {
        let empty = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let valid_win = RECT { left: 0, top: 1704, right: 2880, bottom: 1800 };
        let valid_dwm = RECT { left: 0, top: 1706, right: 2880, bottom: 1800 };

        assert_eq!(choose_taskbar_rect(Some(empty), Some(empty)), None);
        assert_eq!(choose_taskbar_rect(Some(empty), Some(valid_dwm)), Some(valid_dwm));
        assert_eq!(choose_taskbar_rect(Some(valid_win), Some(valid_dwm)), Some(valid_win));
        assert_eq!(choose_taskbar_rect(None, Some(empty)), None);
    }

    #[test]
    fn resolve_taskbar_top_handles_shown_sliding_and_hidden() {
        let m_top = 0;
        let m_bot = 1800;
        assert_eq!(
            resolve_taskbar_top_px(RECT { left: 0, top: 1704, right: 2880, bottom: 1800 }, true, m_top, m_bot),
            (1704, true)
        );
        assert_eq!(
            resolve_taskbar_top_px(RECT { left: 0, top: 1752, right: 2880, bottom: 1848 }, true, m_top, m_bot),
            (1752, true)
        );
        assert_eq!(
            resolve_taskbar_top_px(RECT { left: 0, top: 1680, right: 2880, bottom: 1764 }, true, m_top, m_bot),
            (1680, true)
        );
        assert_eq!(
            resolve_taskbar_top_px(RECT { left: 0, top: 0, right: 0, bottom: 0 }, true, m_top, m_bot),
            (1800, false)
        );
        assert_eq!(
            resolve_taskbar_top_px(RECT { left: 0, top: 1798, right: 2880, bottom: 1894 }, true, m_top, m_bot),
            (1800, false)
        );
        assert_eq!(
            resolve_taskbar_top_px(RECT { left: 0, top: 1802, right: 2880, bottom: 1898 }, true, m_top, m_bot),
            (1800, false)
        );
        assert_eq!(
            resolve_taskbar_top_px(RECT { left: 0, top: 1704, right: 2880, bottom: 1800 }, false, m_top, m_bot),
            (1800, false)
        );
    }

    #[test]
    fn monitor_taskbar_top_px_prefers_shown_and_primary_slice() {
        let mon = MonitorMetrics {
            phys_left: 0,
            phys_top: 0,
            phys_width: 2880,
            phys_height: 1800,
            work_left: 0,
            work_top: 0,
            work_width: 2880,
            work_height: 1800,
            dpi: 192,
            dpr: 2.0,
            is_primary: true,
        };
        let hidden_secondary = TaskbarSlice {
            hwnd: 100,
            monitor_idx: 0,
            top_px: 1800,
            mon_bottom_px: 1800,
            is_shown: false,
            is_primary: false,
            win_rect: (0, 1798, 2880, 1894),
        };
        let shown_primary = TaskbarSlice {
            hwnd: 200,
            monitor_idx: 0,
            top_px: 1704,
            mon_bottom_px: 1800,
            is_shown: true,
            is_primary: true,
            win_rect: (0, 1704, 2880, 1800),
        };
        let hidden_primary = TaskbarSlice {
            hwnd: 200,
            monitor_idx: 0,
            top_px: 1800,
            mon_bottom_px: 1800,
            is_shown: false,
            is_primary: true,
            win_rect: (0, 1798, 2880, 1894),
        };

        let (top, picked) = monitor_taskbar_top_px(&[hidden_secondary, shown_primary], 0, &mon);
        assert_eq!(top, 1704);
        assert_eq!(picked.map(|s| s.hwnd), Some(200));

        let (top_h, picked_h) = monitor_taskbar_top_px(&[hidden_secondary, hidden_primary], 0, &mon);
        assert_eq!(top_h, 1800);
        assert_eq!(picked_h.map(|s| s.hwnd), Some(200));
    }

    #[test]
    fn taskbar_offset_preserves_dangling_paws_and_snaps_midscreen_pets() {
        let dpr = 2.0;
        let win_h = 378.0;
        let geom_s = 180.0;
        let tb_shown = 1704;
        let tb_hidden = 1800;

        // Core bottom sits 18 DIP below taskbar top (dangling paws)
        let win_y = pet_win_y_for_taskbar_px(tb_shown, 18.0, win_h, geom_s, dpr);
        let core_b = pet_core_bottom_px(win_y.round() as i32, win_h, geom_s, dpr);
        let off = initial_taskbar_offset_dip(core_b, tb_shown, dpr);
        assert!((off - 18.0).abs() <= 0.6);

        // Moving taskbar down 96 px (shown -> hidden) shifts window down by exactly 96 px
        let win_y_hidden = pet_win_y_for_taskbar_px(tb_hidden, 18.0, win_h, geom_s, dpr);
        assert!((win_y_hidden - win_y - 96.0).abs() < 1e-6);

        // Far mid-screen pet snaps cleanly to 0.0 offset
        assert_eq!(initial_taskbar_offset_dip(900.0, tb_shown, dpr), 0.0);
    }

    #[test]
    fn step_smooth_y_converges_cleanly() {
        let mut y = 1146.0;
        for _ in 0..30 {
            y = step_smooth_y(y, 1242.0, 0.016);
        }
        assert_eq!(y, 1242.0);
    }

    #[test]
    fn shell_flyout_exe_detection_matches_windows_shell_hosts() {
        assert!(is_shell_taskbar_flyout_exe("startmenuexperiencehost.exe"));
        assert!(is_shell_taskbar_flyout_exe("searchhost.exe"));
        assert!(is_shell_taskbar_flyout_exe("searchapp.exe"));
        assert!(is_shell_taskbar_flyout_exe("shellhost.exe"));
        assert!(is_shell_taskbar_flyout_exe("shellexperiencehost.exe"));
        assert!(!is_shell_taskbar_flyout_exe("explorer.exe"));
        assert!(!is_shell_taskbar_flyout_exe("msedge.exe"));
        assert!(!is_shell_taskbar_flyout_exe(""));
    }

    #[test]
    fn merge_taskbar_hwnd_retains_existing_and_deduplicates() {
        let h_primary = HWND(0x4013A_isize as *mut _);
        let h_secondary = HWND(0x1C0B46_isize as *mut _);
        let mut list = vec![(h_primary, true)];

        merge_taskbar_hwnd(&mut list, HWND::default(), true);
        assert_eq!(list, vec![(h_primary, true)]);

        merge_taskbar_hwnd(&mut list, h_primary, false);
        assert_eq!(list, vec![(h_primary, true)]);

        merge_taskbar_hwnd(&mut list, h_secondary, false);
        assert_eq!(list, vec![(h_primary, true), (h_secondary, false)]);

        merge_taskbar_hwnd(&mut list, h_secondary, true);
        assert_eq!(list, vec![(h_primary, true), (h_secondary, true)]);
    }

    #[test]
    fn estimate_shown_taskbar_top_uses_priority_fallbacks() {
        let hidden_rc = RECT {
            left: 0,
            top: 1798,
            right: 2880,
            bottom: 1894,
        };
        let empty_rc = RECT::default();

        // Priority 1: last observed shown height
        assert_eq!(
            estimate_shown_taskbar_top_px(0, 1800, 2.0, Some(96), hidden_rc, Some(108)),
            1704
        );
        // Priority 2: ABM_GETTASKBARPOS height when no prior shown sample exists
        assert_eq!(
            estimate_shown_taskbar_top_px(0, 1800, 2.0, None, hidden_rc, Some(104)),
            1696
        );
        // Priority 3: raw hidden Shell_TrayWnd height (retains its 96 px taskbar thickness)
        assert_eq!(
            estimate_shown_taskbar_top_px(0, 1800, 1.0, None, hidden_rc, None),
            1704
        );
        // Priority 4: 48 DIP * DPR fallback when nothing else is valid
        assert_eq!(
            estimate_shown_taskbar_top_px(0, 1800, 2.25, None, empty_rc, Some(8)),
            1692
        );
        // Clamped within [mon_bottom - max_tb_h, mon_bottom]
        assert_eq!(
            estimate_shown_taskbar_top_px(0, 1800, 2.0, Some(900), empty_rc, None),
            1704
        );
    }
}
