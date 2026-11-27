use image::imageops::FilterType;
use std::ffi::OsStr;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE, HWND, LPARAM, POINT, TRUE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    PostMessageW, SetForegroundWindow, TrackPopupMenuEx, HICON, HMENU, ICONINFO, MF_CHECKED,
    MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_APP, WM_NULL,
};

pub const WM_TRAYICON: u32 = WM_APP + 1;
pub const TRAY_ICON_ID: u32 = 1;

const RUN_SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("BugPetRs");

/// Size presets (px) offered in the per-pet `Size` submenu.
const SIZE_PRESETS: [f64; 7] = [120.0, 150.0, 180.0, 190.0, 220.0, 260.0, 300.0];

/// Plain snapshot of one live pet, used to render its quick-tweak menu.
#[derive(Debug, Clone)]
pub struct PetMenuSnapshot {
    pub id: String,
    pub char_id: String,
    pub size: f64,
    pub flip: bool,
    pub lock: bool,
    pub click_through: bool,
    pub always_on_top: bool,
    pub mute: bool,
    pub music: bool,
    pub typing: bool,
    pub key_bg: String,
    /// `(part_id, display_label, enabled)`
    pub parts: Vec<(String, String, bool)>,
}

/// Action chosen from a single pet's quick-tweak menu.
#[derive(Debug, Clone, PartialEq)]
pub enum PetQuickAction {
    SetSize(f64),
    DeltaSize(f64),
    ToggleFlip,
    ToggleLock,
    ToggleClickThrough,
    ToggleTop,
    ToggleMute,
    ToggleMusic,
    ToggleTyping,
    SetKeyBg(String),
    TogglePart(String),
    SetAllParts(bool),
    ClosePet,
}

/// Everything the tray context menu needs to render its current state.
pub struct GlobalMenuState<'a> {
    pub characters: &'a [String],
    pub pets: &'a [PetMenuSnapshot],
    pub pets_disabled: bool,
    pub ghost: bool,
    pub launch_at_login: bool,
    pub react: &'a crate::config::ReactPrefs,
    pub ui: &'a crate::config::UiPrefs,
}

/// Action chosen from the tray context menu.
#[derive(Debug, Clone, PartialEq)]
pub enum TrayAction {
    TogglePetsDisabled,
    SpawnCharacter(String),
    GatherAll,
    ToggleGhost,
    ToggleLaunchAtLogin,
    ReloadFromSource,
    Quit,
    PetQuick {
        pet_id: String,
        action: PetQuickAction,
    },
    SetReactSense(f64),
    SetReactSound(String),
    SetReactGain(f64),
    SetKeyEvery(u32),
    ToggleRestIdle,
    SetRestMin(f64),
    SetBubbleFont(f64),
    ToggleReduceMotion,
    SetLang(String),
}

pub struct TrayIcon {
    hwnd: HWND,
    uid: u32,
    hicon: HICON,
    added: bool,
}

impl TrayIcon {
    pub fn new(
        hwnd: HWND,
        data_dir: Option<&Path>,
        pet_count: usize,
        paused: bool,
        lang: &str,
    ) -> Result<Self, String> {
        let (w, h, rgba) = load_tray_rgba(data_dir);
        let hicon = rgba_to_hicon(w, h, &rgba)?;

        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ICON_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            hIcon: hicon,
            ..Default::default()
        };
        write_tip(&mut nid.szTip, &format_tooltip(pet_count, paused, lang));

        let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool();
        if !ok {
            let err = unsafe { GetLastError() };
            unsafe {
                let _ = DestroyIcon(hicon);
            }
            return Err(format!("Shell_NotifyIconW(NIM_ADD) failed ({err:?})"));
        }

        Ok(Self {
            hwnd,
            uid: TRAY_ICON_ID,
            hicon,
            added: true,
        })
    }

    pub fn update_count(&self, pet_count: usize, paused: bool, lang: &str) {
        if !self.added {
            return;
        }
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: self.uid,
            uFlags: NIF_TIP,
            ..Default::default()
        };
        write_tip(&mut nid.szTip, &format_tooltip(pet_count, paused, lang));
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    pub fn show_context_menu(&self, state: &GlobalMenuState<'_>) -> Option<TrayAction> {
        show_context_menu(self.hwnd, state)
    }

    pub fn remove(&mut self) {
        if self.added {
            let nid = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: self.uid,
                ..Default::default()
            };
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            self.added = false;
        }
        if !self.hicon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(self.hicon);
            }
            self.hicon = HICON::default();
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.remove();
    }
}

pub fn format_tooltip(pet_count: usize, paused: bool, lang: &str) -> String {
    let is_ko = lang.trim().to_ascii_lowercase().starts_with("ko");
    if paused {
        if is_ko {
            format!("BUG Clicker RS · 일시 중지 ({pet_count}명)")
        } else {
            format!("BUG Clicker RS · Paused ({pet_count})")
        }
    } else if is_ko {
        format!("BUG Clicker RS · {pet_count}명")
    } else {
        format!("BUG Clicker RS · {pet_count}")
    }
}

fn write_tip(dst: &mut [u16; 128], text: &str) {
    dst.fill(0);
    for (i, ch) in text.encode_utf16().take(127).enumerate() {
        dst[i] = ch;
    }
}

fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn load_tray_rgba(data_dir: Option<&Path>) -> (u32, u32, Vec<u8>) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets").join("tray-32.png"));
            candidates.push(dir.join("assets").join("tray-16.png"));
            if let Some(p1) = dir.parent() {
                candidates.push(p1.join("assets").join("tray-32.png"));
                candidates.push(p1.join("assets").join("tray-16.png"));
                if let Some(p2) = p1.parent() {
                    candidates.push(p2.join("assets").join("tray-32.png"));
                }
            }
        }
    }
    if let Some(dd) = data_dir {
        candidates.push(dd.join("assets").join("tray-32.png"));
        candidates.push(dd.join("assets").join("tray-16.png"));
    }

    for path in candidates {
        if path.is_file() {
            if let Ok(img) = image::open(&path) {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                if w == 32 && h == 32 {
                    return (32, 32, rgba.into_raw());
                }
                let resized = image::imageops::resize(&rgba, 32, 32, FilterType::Lanczos3);
                return (32, 32, resized.into_raw());
            }
        }
    }

    (32, 32, generate_paw_heart_rgba())
}

pub fn generate_paw_heart_rgba() -> Vec<u8> {
    let Some(mut pixmap) = Pixmap::new(32, 32) else {
        return vec![255; 32 * 32 * 4];
    };

    // Rounded dark badge background
    let mut pb = PathBuilder::new();
    let r = 7.0f32;
    let x = 2.0f32;
    let y = 2.0f32;
    let w = 28.0f32;
    let h = 28.0f32;
    let k = 0.552_284_8 * r;
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
    if let Some(badge) = pb.finish() {
        let mut p = Paint::default();
        p.set_color_rgba8(0x22, 0x22, 0x28, 255);
        p.anti_alias = true;
        pixmap.fill_path(&badge, &p, FillRule::Winding, Transform::identity(), None);
    }

    // Heart-shaped main paw pad + 4 toe beans
    let mut paw_paint = Paint::default();
    paw_paint.set_color(Color::from_rgba8(255, 138, 168, 255));
    paw_paint.anti_alias = true;

    let mut hp = PathBuilder::new();
    hp.move_to(16.0, 25.0);
    hp.cubic_to(9.5, 21.5, 9.0, 15.2, 12.6, 14.5);
    hp.cubic_to(14.5, 14.1, 15.6, 15.3, 16.0, 16.4);
    hp.cubic_to(16.4, 15.3, 17.5, 14.1, 19.4, 14.5);
    hp.cubic_to(23.0, 15.2, 22.5, 21.5, 16.0, 25.0);
    hp.close();
    if let Some(heart) = hp.finish() {
        pixmap.fill_path(&heart, &paw_paint, FillRule::Winding, Transform::identity(), None);
    }

    let toes = [(8.4, 13.0, 2.3), (13.1, 9.6, 2.4), (18.9, 9.6, 2.4), (23.6, 13.0, 2.3)];
    for (tx, ty, tr) in toes {
        let mut tp = PathBuilder::new();
        if let Some(rect) = Rect::from_xywh(tx - tr, ty - tr * 1.15, tr * 2.0, tr * 2.3) {
            tp.push_oval(rect);
        }
        if let Some(toe) = tp.finish() {
            pixmap.fill_path(&toe, &paw_paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    // Un-premultiply tiny-skia buffer into straight RGBA
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a > 0 && a < 255 {
            let af = a as f32 / 255.0;
            px[0] = ((px[0] as f32 / af).round()).min(255.0) as u8;
            px[1] = ((px[1] as f32 / af).round()).min(255.0) as u8;
            px[2] = ((px[2] as f32 / af).round()).min(255.0) as u8;
        }
    }
    rgba
}

pub fn rgba_to_hicon(width: u32, height: u32, rgba: &[u8]) -> Result<HICON, String> {
    if width == 0 || height == 0 || rgba.len() != (width * height * 4) as usize {
        return Err("Invalid RGBA buffer for tray HICON".to_string());
    }

    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: -(height as i32), // top-down DIB
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };

    let mut bits_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    let hbm_color: HBITMAP = unsafe {
        CreateDIBSection(
            HDC::default(),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits_ptr,
            HANDLE::default(),
            0,
        )
    }
    .map_err(|e| format!("CreateDIBSection failed: {e}"))?;

    if bits_ptr.is_null() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbm_color.0));
        }
        return Err("CreateDIBSection returned null pixel buffer".to_string());
    }

    unsafe {
        let dst = std::slice::from_raw_parts_mut(bits_ptr as *mut u8, rgba.len());
        for (src_px, dst_px) in rgba.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            dst_px[0] = src_px[2];
            dst_px[1] = src_px[1];
            dst_px[2] = src_px[0];
            dst_px[3] = src_px[3];
        }
    }

    let mask_row_bytes = (((width + 15) / 16) * 2) as usize;
    let mask_bits = vec![0u8; mask_row_bytes * height as usize];
    let hbm_mask: HBITMAP = unsafe {
        CreateBitmap(
            width as i32,
            height as i32,
            1,
            1,
            Some(mask_bits.as_ptr() as *const core::ffi::c_void),
        )
    };
    if hbm_mask.is_invalid() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbm_color.0));
        }
        return Err("CreateBitmap mask failed".to_string());
    }

    let ii = ICONINFO {
        fIcon: TRUE,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbm_mask,
        hbmColor: hbm_color,
    };
    let res = unsafe { CreateIconIndirect(&ii) };

    unsafe {
        let _ = DeleteObject(HGDIOBJ(hbm_color.0));
        let _ = DeleteObject(HGDIOBJ(hbm_mask.0));
    }

    res.map_err(|e| format!("CreateIconIndirect failed: {e}"))
}

/// Localized menu strings (`en`, `ko`, `ja`, `zh`). Templates use `{n}` as a number placeholder.
pub struct MenuLabels {
    pets_disabled: &'static str,
    spawn_char: &'static str,
    none: &'static str,
    pet_quick: &'static str,
    quick_settings: &'static str,
    gather_all: &'static str,
    ghost: &'static str,
    reload: &'static str,
    launch_at_login: &'static str,
    quit: &'static str,
    size: &'static str,
    larger: &'static str,
    smaller: &'static str,
    flip: &'static str,
    lock: &'static str,
    click_through: &'static str,
    always_on_top: &'static str,
    mute: &'static str,
    music: &'static str,
    typing: &'static str,
    obs_bg: &'static str,
    bg_off: &'static str,
    bg_green: &'static str,
    bg_blue: &'static str,
    bg_magenta: &'static str,
    rig_parts: &'static str,
    all_on: &'static str,
    all_off: &'static str,
    close_pet: &'static str,
    music_sens: &'static str,
    sound_mode: &'static str,
    sound_music: &'static str,
    sound_all: &'static str,
    audio_gain: &'static str,
    typing_cadence: &'static str,
    every_keys: &'static str,
    rest_idle: &'static str,
    rest_timeout: &'static str,
    minutes: &'static str,
    bubble_font: &'static str,
    reduce_motion: &'static str,
    language: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuLang {
    En,
    Ko,
    Ja,
    Zh,
}

fn detect_lang(lang: &str) -> MenuLang {
    let l = lang.trim().to_ascii_lowercase();
    if l.starts_with("ko") {
        MenuLang::Ko
    } else if l.starts_with("ja") {
        MenuLang::Ja
    } else if l.starts_with("zh") {
        MenuLang::Zh
    } else {
        MenuLang::En
    }
}

pub fn menu_labels(lang: &str) -> MenuLabels {
    let l = detect_lang(lang);
    let p = |en: &'static str, ko: &'static str, ja: &'static str, zh: &'static str| -> &'static str {
        match l {
            MenuLang::En => en,
            MenuLang::Ko => ko,
            MenuLang::Ja => ja,
            MenuLang::Zh => zh,
        }
    };
    MenuLabels {
        pets_disabled: p(
            "Disable Active Pets",
            "활성 펫 비활성화 (일시 중지)",
            "アクティブなペットを無効化",
            "停用啟用中的寵物",
        ),
        spawn_char: p("Spawn Character", "캐릭터 띄우기", "キャラクターを表示", "顯示角色"),
        none: p("(None)", "(없음)", "（なし）", "（無）"),
        pet_quick: p("Pet Quick Settings", "펫 빠른 설정", "ペットのクイック設定", "寵物快速設定"),
        quick_settings: p("Quick Settings", "빠른 설정", "クイック設定", "快速設定"),
        gather_all: p("Gather All", "전부 한자리에 모으기", "すべて1か所に集める", "全部集中到一處"),
        ghost: p(
            "Ghost Click-Through",
            "마우스 통과 (캐릭터 안 눌림)",
            "マウスを通す",
            "滑鼠穿透",
        ),
        reload: p(
            "Reload from BUG Clicker",
            "BUG Clicker에서 다시 불러오기",
            "BUG Clickerから再読み込み",
            "從 BUG Clicker 重新載入",
        ),
        launch_at_login: p("Start with Windows", "컴퓨터 켤 때 같이 실행", "PC起動時に起動", "開機時一起啟動"),
        quit: p("Quit", "종료", "終了", "結束"),
        size: p("Size", "크기", "サイズ", "大小"),
        larger: p("Larger (+10px)", "더 크게 (+10px)", "大きく (+10px)", "放大 (+10px)"),
        smaller: p("Smaller (-10px)", "더 작게 (-10px)", "小さく (-10px)", "縮小 (-10px)"),
        flip: p("Flip Horizontally", "좌우 반전", "左右反転", "水平翻轉"),
        lock: p("Lock Position", "위치 고정", "位置を固定", "鎖定位置"),
        click_through: p("Click-Through", "클릭 통과", "クリック透過", "點擊穿透"),
        always_on_top: p("Always on Top", "항상 위", "常に手前に表示", "永遠置頂"),
        mute: p("Mute Sound", "소리 끄기", "サウンドをミュート", "靜音"),
        music: p("React to Music", "음악에 반응", "音楽に反応", "對音樂反應"),
        typing: p("React to Typing", "타이핑에 반응", "タイピングに反応", "對打字反應"),
        obs_bg: p("OBS Background", "OBS 배경", "OBS背景", "OBS 背景"),
        bg_off: p("Off (Transparent)", "끄기 (투명)", "オフ（透明）", "關閉（透明）"),
        bg_green: p("Green (#00ff00)", "초록 (#00ff00)", "緑 (#00ff00)", "綠色 (#00ff00)"),
        bg_blue: p("Blue (#0000ff)", "파랑 (#0000ff)", "青 (#0000ff)", "藍色 (#0000ff)"),
        bg_magenta: p(
            "Magenta (#ff00ff)",
            "마젠타 (#ff00ff)",
            "マゼンタ (#ff00ff)",
            "洋紅 (#ff00ff)",
        ),
        rig_parts: p("Rig Parts", "리그 파츠", "リグパーツ", "骨架部件"),
        all_on: p("All On", "모두 켜기", "すべてオン", "全部開啟"),
        all_off: p("All Off", "모두 끄기", "すべてオフ", "全部關閉"),
        close_pet: p("Close This Pet", "이 펫 닫기", "このペットを閉じる", "關閉此寵物"),
        music_sens: p("Music Sensitivity", "음악 민감도", "音楽の感度", "音樂靈敏度"),
        sound_mode: p("Sound Mode", "사운드 모드", "サウンドモード", "聲音模式"),
        sound_music: p("Music Only", "음악만", "音楽のみ", "僅音樂"),
        sound_all: p("All Sounds", "모든 소리", "すべての音", "所有聲音"),
        audio_gain: p("Audio Gain", "오디오 게인", "オーディオゲイン", "音訊增益"),
        typing_cadence: p("Typing Cadence", "타이핑 반응 간격", "タイピング反応間隔", "打字反應間隔"),
        every_keys: p("Every {n} keys", "{n}키마다", "{n}キーごと", "每 {n} 鍵"),
        rest_idle: p("Rest When Idle", "가만히 있으면 쉬기", "放置で休憩", "閒置時休息"),
        rest_timeout: p("Idle Rest Timeout", "휴식 대기 시간", "休憩までの時間", "休息等待時間"),
        minutes: p("{n} min", "{n}분", "{n}分", "{n} 分鐘"),
        bubble_font: p("Bubble Font Size", "말풍선 글자 크기", "吹き出しの文字サイズ", "對話框字體大小"),
        reduce_motion: p(
            "Lite / Reduce Motion",
            "라이트 / 움직임 줄이기",
            "軽量 / 動きを減らす",
            "輕量 / 減少動態",
        ),
        language: p("Language", "언어", "言語", "語言"),
    }
}

fn fmt_n(template: &str, n: u32) -> String {
    template.replace("{n}", &n.to_string())
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

/// Owns an `HMENU` and destroys it on drop unless ownership was handed to a parent menu.
/// Destroying a parent menu also destroys attached submenus, so a child is only
/// `release`d after `AppendMenuW(MF_POPUP)` succeeded.
struct MenuGuard(HMENU);

impl MenuGuard {
    fn new() -> Option<Self> {
        unsafe { CreatePopupMenu() }.ok().map(Self)
    }

    fn raw(&self) -> HMENU {
        self.0
    }

    fn release(mut self) -> HMENU {
        let h = self.0;
        self.0 = HMENU::default();
        h
    }
}

impl Drop for MenuGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DestroyMenu(self.0);
            }
        }
    }
}

/// Registers `action` in the command table and returns its 1-based command ID
/// (`base` + position), so IDs never collide no matter how many items exist.
fn push_action<T>(actions: &mut Vec<T>, base: u32, action: T) -> u32 {
    actions.push(action);
    base + actions.len() as u32
}

/// Resolves a 1-based command ID returned by `TrackPopupMenuEx` into its action.
pub fn action_from_cmd_id<T: Clone>(cmd_id: u32, table: &[T]) -> Option<T> {
    if cmd_id == 0 {
        return None;
    }
    table.get(cmd_id as usize - 1).cloned()
}

fn append_item(menu: &MenuGuard, id: u32, label: &str, checked: bool) {
    let w = to_wide_null(label);
    let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
    unsafe {
        let _ = AppendMenuW(menu.raw(), flags, id as usize, PCWSTR(w.as_ptr()));
    }
}

fn append_disabled(menu: &MenuGuard, label: &str) {
    let w = to_wide_null(label);
    unsafe {
        let _ = AppendMenuW(menu.raw(), MF_STRING | MF_GRAYED, 0, PCWSTR(w.as_ptr()));
    }
}

fn append_separator(menu: &MenuGuard) {
    unsafe {
        let _ = AppendMenuW(menu.raw(), MF_SEPARATOR, 0, PCWSTR::null());
    }
}

/// Attaches `sub` to `menu`. On failure `sub` is destroyed by its guard.
fn append_popup(menu: &MenuGuard, sub: MenuGuard, label: &str) -> bool {
    let w = to_wide_null(label);
    let ok = unsafe {
        AppendMenuW(
            menu.raw(),
            MF_POPUP,
            sub.raw().0 as usize,
            PCWSTR(w.as_ptr()),
        )
    }
    .is_ok();
    if ok {
        let _ = sub.release();
    }
    ok
}

fn build_pet_submenu_at(
    pet: &PetMenuSnapshot,
    labels: &MenuLabels,
    actions: &mut Vec<PetQuickAction>,
    base: u32,
) -> Option<MenuGuard> {
    let menu = MenuGuard::new()?;

    // Size
    let size_menu = MenuGuard::new()?;
    let id = push_action(actions, base, PetQuickAction::DeltaSize(10.0));
    append_item(&size_menu, id, labels.larger, false);
    let id = push_action(actions, base, PetQuickAction::DeltaSize(-10.0));
    append_item(&size_menu, id, labels.smaller, false);
    append_separator(&size_menu);
    for s in SIZE_PRESETS {
        let id = push_action(actions, base, PetQuickAction::SetSize(s));
        append_item(&size_menu, id, &format!("{} px", s as u32), near(pet.size, s, 2.5));
    }
    if !append_popup(&menu, size_menu, labels.size) {
        return None;
    }

    // Checkbox toggles
    let toggles = [
        (labels.flip, PetQuickAction::ToggleFlip, pet.flip),
        (labels.lock, PetQuickAction::ToggleLock, pet.lock),
        (labels.click_through, PetQuickAction::ToggleClickThrough, pet.click_through),
        (labels.always_on_top, PetQuickAction::ToggleTop, pet.always_on_top),
        (labels.mute, PetQuickAction::ToggleMute, pet.mute),
        (labels.music, PetQuickAction::ToggleMusic, pet.music),
        (labels.typing, PetQuickAction::ToggleTyping, pet.typing),
    ];
    for (label, action, on) in toggles {
        let id = push_action(actions, base, action);
        append_item(&menu, id, label, on);
    }

    // OBS background
    let bg_menu = MenuGuard::new()?;
    let bg_choices = [
        (labels.bg_off, ""),
        (labels.bg_green, "#00ff00"),
        (labels.bg_blue, "#0000ff"),
        (labels.bg_magenta, "#ff00ff"),
    ];
    for (label, value) in bg_choices {
        let id = push_action(actions, base, PetQuickAction::SetKeyBg(value.to_string()));
        append_item(&bg_menu, id, label, pet.key_bg.trim().eq_ignore_ascii_case(value));
    }
    if !append_popup(&menu, bg_menu, labels.obs_bg) {
        return None;
    }

    // Rig parts
    if !pet.parts.is_empty() {
        let parts_menu = MenuGuard::new()?;
        let id = push_action(actions, base, PetQuickAction::SetAllParts(true));
        append_item(&parts_menu, id, labels.all_on, false);
        let id = push_action(actions, base, PetQuickAction::SetAllParts(false));
        append_item(&parts_menu, id, labels.all_off, false);
        append_separator(&parts_menu);
        for (part_id, label, on) in &pet.parts {
            let text = if label.trim().is_empty() { part_id } else { label };
            let id = push_action(actions, base, PetQuickAction::TogglePart(part_id.clone()));
            append_item(&parts_menu, id, text, *on);
        }
        if !append_popup(&menu, parts_menu, labels.rig_parts) {
            return None;
        }
    }

    append_separator(&menu);
    let id = push_action(actions, base, PetQuickAction::ClosePet);
    append_item(&menu, id, labels.close_pet, false);
    Some(menu)
}

/// Builds the per-pet quick-tweak popup. Command IDs are 1-based positions in `actions`.
/// The caller owns the returned menu (destroying it also destroys its submenus).
pub fn build_pet_submenu(
    pet: &PetMenuSnapshot,
    labels: &MenuLabels,
    actions: &mut Vec<PetQuickAction>,
) -> Option<HMENU> {
    build_pet_submenu_at(pet, labels, actions, 0).map(MenuGuard::release)
}

fn build_quick_settings_menu(
    state: &GlobalMenuState<'_>,
    labels: &MenuLabels,
    actions: &mut Vec<TrayAction>,
) -> Option<MenuGuard> {
    let menu = MenuGuard::new()?;

    // Music sensitivity 1..=10
    let sub = MenuGuard::new()?;
    let cur = state.react.sense.round();
    for n in 1..=10u32 {
        let id = push_action(actions, 0, TrayAction::SetReactSense(n as f64));
        append_item(&sub, id, &n.to_string(), cur == n as f64);
    }
    if !append_popup(&menu, sub, labels.music_sens) {
        return None;
    }

    // Sound mode
    let sub = MenuGuard::new()?;
    let all = state.react.sound.trim().eq_ignore_ascii_case("all");
    let id = push_action(actions, 0, TrayAction::SetReactSound("music".to_string()));
    append_item(&sub, id, labels.sound_music, !all);
    let id = push_action(actions, 0, TrayAction::SetReactSound("all".to_string()));
    append_item(&sub, id, labels.sound_all, all);
    if !append_popup(&menu, sub, labels.sound_mode) {
        return None;
    }

    // Audio gain (dB)
    let sub = MenuGuard::new()?;
    for (g, text) in [(-6.0, "-6 dB"), (0.0, "0 dB"), (6.0, "+6 dB"), (12.0, "+12 dB")] {
        let id = push_action(actions, 0, TrayAction::SetReactGain(g));
        append_item(&sub, id, text, near(state.react.gain, g, 0.5));
    }
    if !append_popup(&menu, sub, labels.audio_gain) {
        return None;
    }

    // Typing cadence
    let sub = MenuGuard::new()?;
    for n in [1u32, 2, 3, 5] {
        let id = push_action(actions, 0, TrayAction::SetKeyEvery(n));
        append_item(&sub, id, &fmt_n(labels.every_keys, n), state.react.key_every == n);
    }
    if !append_popup(&menu, sub, labels.typing_cadence) {
        return None;
    }

    // Rest when idle + timeout
    let id = push_action(actions, 0, TrayAction::ToggleRestIdle);
    append_item(&menu, id, labels.rest_idle, state.ui.rest_idle);
    let sub = MenuGuard::new()?;
    for m in [3u32, 5, 10, 15] {
        let id = push_action(actions, 0, TrayAction::SetRestMin(m as f64));
        append_item(&sub, id, &fmt_n(labels.minutes, m), near(state.ui.rest_min, m as f64, 0.5));
    }
    if !append_popup(&menu, sub, labels.rest_timeout) {
        return None;
    }

    // Bubble font size
    let sub = MenuGuard::new()?;
    for px in [12u32, 14, 16, 18] {
        let id = push_action(actions, 0, TrayAction::SetBubbleFont(px as f64));
        append_item(&sub, id, &format!("{px} px"), near(state.ui.bubble_font, px as f64, 0.5));
    }
    if !append_popup(&menu, sub, labels.bubble_font) {
        return None;
    }

    // Lite / reduce motion
    let id = push_action(actions, 0, TrayAction::ToggleReduceMotion);
    append_item(
        &menu,
        id,
        labels.reduce_motion,
        state.ui.lite || state.ui.reduce_motion,
    );

    // Language
    let sub = MenuGuard::new()?;
    let cur_lang = detect_lang(&state.ui.lang);
    let langs = [
        ("English (en)", "en", MenuLang::En),
        ("한국어 (ko)", "ko", MenuLang::Ko),
        ("日本語 (ja)", "ja", MenuLang::Ja),
        ("中文 (zh)", "zh", MenuLang::Zh),
    ];
    for (text, code, which) in langs {
        let id = push_action(actions, 0, TrayAction::SetLang(code.to_string()));
        append_item(&sub, id, text, cur_lang == which);
    }
    if !append_popup(&menu, sub, labels.language) {
        return None;
    }

    Some(menu)
}

fn build_global_menu(
    state: &GlobalMenuState<'_>,
    labels: &MenuLabels,
    actions: &mut Vec<TrayAction>,
) -> Option<MenuGuard> {
    let menu = MenuGuard::new()?;

    // Rust-only quick pause toggle
    let id = push_action(actions, 0, TrayAction::TogglePetsDisabled);
    append_item(&menu, id, labels.pets_disabled, state.pets_disabled);
    append_separator(&menu);

    // Spawn Character
    let spawn = MenuGuard::new()?;
    if state.characters.is_empty() {
        append_disabled(&spawn, labels.none);
    } else {
        for ch in state.characters {
            let id = push_action(actions, 0, TrayAction::SpawnCharacter(ch.clone()));
            append_item(&spawn, id, ch, false);
        }
    }
    if !append_popup(&menu, spawn, labels.spawn_char) {
        return None;
    }

    // Pet Quick Settings (one submenu per active pet)
    let pets = MenuGuard::new()?;
    if state.pets.is_empty() {
        append_disabled(&pets, labels.none);
    } else {
        for pet in state.pets {
            let base = actions.len() as u32;
            let mut local: Vec<PetQuickAction> = Vec::new();
            let sub = build_pet_submenu_at(pet, labels, &mut local, base)?;
            let title = format!("{} ({})", pet.char_id, pet.id);
            if !append_popup(&pets, sub, &title) {
                return None;
            }
            for action in local {
                actions.push(TrayAction::PetQuick {
                    pet_id: pet.id.clone(),
                    action,
                });
            }
        }
    }
    if !append_popup(&menu, pets, labels.pet_quick) {
        return None;
    }

    // Quick Settings
    let quick = build_quick_settings_menu(state, labels, actions)?;
    if !append_popup(&menu, quick, labels.quick_settings) {
        return None;
    }

    append_separator(&menu);
    let id = push_action(actions, 0, TrayAction::GatherAll);
    append_item(&menu, id, labels.gather_all, false);
    let id = push_action(actions, 0, TrayAction::ToggleGhost);
    append_item(&menu, id, labels.ghost, state.ghost);
    let id = push_action(actions, 0, TrayAction::ReloadFromSource);
    append_item(&menu, id, labels.reload, false);
    append_separator(&menu);
    let id = push_action(actions, 0, TrayAction::ToggleLaunchAtLogin);
    append_item(&menu, id, labels.launch_at_login, state.launch_at_login);
    append_separator(&menu);
    let id = push_action(actions, 0, TrayAction::Quit);
    append_item(&menu, id, labels.quit, false);

    Some(menu)
}

/// Shows `menu` at the cursor and returns the chosen command ID (0 = dismissed).
fn track_popup(hwnd: HWND, menu: &MenuGuard) -> u32 {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
    }
    let ret = unsafe {
        TrackPopupMenuEx(
            menu.raw(),
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY).0,
            pt.x,
            pt.y,
            hwnd,
            None,
        )
    };
    unsafe {
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    }
    ret.0 as u32
}

/// Shows one pet's quick-tweak menu at the cursor (used by the radial Settings icon).
#[allow(dead_code)]
pub fn show_pet_settings_menu(
    hwnd: HWND,
    pet: &PetMenuSnapshot,
    lang: &str,
) -> Option<PetQuickAction> {
    let labels = menu_labels(lang);
    let mut actions: Vec<PetQuickAction> = Vec::new();
    let menu = MenuGuard(build_pet_submenu(pet, &labels, &mut actions)?);
    let cmd = track_popup(hwnd, &menu);
    drop(menu);
    action_from_cmd_id(cmd, &actions)
}

/// Shows the tray context menu at the cursor and returns the chosen action.
pub fn show_context_menu(hwnd: HWND, state: &GlobalMenuState<'_>) -> Option<TrayAction> {
    let labels = menu_labels(&state.ui.lang);
    let mut actions: Vec<TrayAction> = Vec::new();
    let menu = build_global_menu(state, &labels, &mut actions)?;
    let cmd = track_popup(hwnd, &menu);
    drop(menu);
    action_from_cmd_id(cmd, &actions)
}

pub fn is_autostart_enabled() -> bool {
    let mut hkey = HKEY::default();
    let open_err = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, 0, KEY_READ, &mut hkey) };
    if open_err != ERROR_SUCCESS {
        return false;
    }
    let mut reg_type = REG_VALUE_TYPE(0);
    let mut cb: u32 = 0;
    let q_err = unsafe {
        RegQueryValueExW(
            hkey,
            RUN_VALUE,
            None,
            Some(&mut reg_type),
            None,
            Some(&mut cb),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    q_err == ERROR_SUCCESS && reg_type == REG_SZ && cb > 2
}

pub fn set_autostart_enabled(enable: bool, data_dir: Option<&Path>) -> Result<(), String> {
    if enable {
        let exe = std::env::current_exe()
            .map_err(|e| format!("Failed to resolve current_exe: {e}"))?;
        let mut cmd = format!("\"{}\"", exe.display());
        if let Some(dir) = data_dir {
            cmd.push_str(&format!(" --data-dir \"{}\"", dir.display()));
        }
        let wide = to_wide_null(&cmd);
        let mut bytes = Vec::with_capacity(wide.len() * 2);
        for u in wide {
            bytes.extend_from_slice(&u.to_le_bytes());
        }

        let mut hkey = HKEY::default();
        let err = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                RUN_SUBKEY,
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        };
        if err != ERROR_SUCCESS {
            return Err(format!("RegCreateKeyExW failed: {}", err.0));
        }

        let set_err = unsafe { RegSetValueExW(hkey, RUN_VALUE, 0, REG_SZ, Some(&bytes)) };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        if set_err != ERROR_SUCCESS {
            return Err(format!("RegSetValueExW failed: {}", set_err.0));
        }
        Ok(())
    } else {
        let mut hkey = HKEY::default();
        let err = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, 0, KEY_WRITE, &mut hkey) };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if err != ERROR_SUCCESS {
            return Err(format!("RegOpenKeyExW failed: {}", err.0));
        }

        let del_err = unsafe { RegDeleteValueW(hkey, RUN_VALUE) };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        if del_err != ERROR_SUCCESS && del_err != ERROR_FILE_NOT_FOUND {
            return Err(format!("RegDeleteValueW failed: {}", del_err.0));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ReactPrefs, UiPrefs};

    fn pet(id: &str, char_id: &str, parts: Vec<(String, String, bool)>) -> PetMenuSnapshot {
        PetMenuSnapshot {
            id: id.to_string(),
            char_id: char_id.to_string(),
            size: 180.0,
            flip: false,
            lock: true,
            click_through: false,
            always_on_top: true,
            mute: false,
            music: true,
            typing: true,
            key_bg: String::new(),
            parts,
        }
    }

    fn sample_parts() -> Vec<(String, String, bool)> {
        vec![
            ("ear".to_string(), "Ear".to_string(), true),
            ("tail".to_string(), String::new(), false),
        ]
    }

    fn pet_actions(p: &PetMenuSnapshot) -> Vec<PetQuickAction> {
        let labels = menu_labels("en");
        let mut v = Vec::new();
        let menu = build_pet_submenu(p, &labels, &mut v).expect("pet menu");
        unsafe {
            let _ = DestroyMenu(menu);
        }
        v
    }

    #[test]
    fn tooltip_formats_ko_and_en() {
        assert_eq!(format_tooltip(2, false, "en"), "BUG Clicker RS · 2");
        assert_eq!(format_tooltip(3, false, "ko"), "BUG Clicker RS · 3명");
        assert_eq!(format_tooltip(1, false, "ko-KR"), "BUG Clicker RS · 1명");
        assert_eq!(format_tooltip(2, true, "en"), "BUG Clicker RS · Paused (2)");
        assert_eq!(format_tooltip(2, true, "ko"), "BUG Clicker RS · 일시 중지 (2명)");
    }

    #[test]
    fn procedural_icon_builds_and_destroys_hicon() {
        let rgba = generate_paw_heart_rgba();
        assert_eq!(rgba.len(), 32 * 32 * 4);
        assert!(rgba.chunks_exact(4).any(|px| px[3] > 0));
        let hicon = rgba_to_hicon(32, 32, &rgba).expect("HICON must succeed");
        assert!(!hicon.is_invalid());
        unsafe {
            DestroyIcon(hicon).expect("DestroyIcon must succeed");
        }
    }

    #[test]
    fn cmd_id_resolution_is_one_based_and_bounds_checked() {
        let table = vec![TrayAction::GatherAll, TrayAction::Quit];
        assert_eq!(action_from_cmd_id(0, &table), None);
        assert_eq!(action_from_cmd_id(1, &table), Some(TrayAction::GatherAll));
        assert_eq!(action_from_cmd_id(2, &table), Some(TrayAction::Quit));
        assert_eq!(action_from_cmd_id(3, &table), None);
        let empty: Vec<PetQuickAction> = Vec::new();
        assert_eq!(action_from_cmd_id(1, &empty), None);
    }

    #[test]
    fn pet_submenu_table_order_without_parts() {
        let p = pet("p1", "AlphaPet", Vec::new());
        let actions = pet_actions(&p);
        let mut expected = vec![PetQuickAction::DeltaSize(10.0), PetQuickAction::DeltaSize(-10.0)];
        for s in [120.0, 150.0, 180.0, 190.0, 220.0, 260.0, 300.0] {
            expected.push(PetQuickAction::SetSize(s));
        }
        expected.extend([
            PetQuickAction::ToggleFlip,
            PetQuickAction::ToggleLock,
            PetQuickAction::ToggleClickThrough,
            PetQuickAction::ToggleTop,
            PetQuickAction::ToggleMute,
            PetQuickAction::ToggleMusic,
            PetQuickAction::ToggleTyping,
            PetQuickAction::SetKeyBg(String::new()),
            PetQuickAction::SetKeyBg("#00ff00".to_string()),
            PetQuickAction::SetKeyBg("#0000ff".to_string()),
            PetQuickAction::SetKeyBg("#ff00ff".to_string()),
            PetQuickAction::ClosePet,
        ]);
        assert_eq!(actions, expected);
        assert_eq!(action_from_cmd_id(1, &actions), Some(PetQuickAction::DeltaSize(10.0)));
        assert_eq!(
            action_from_cmd_id(actions.len() as u32, &actions),
            Some(PetQuickAction::ClosePet)
        );
        assert_eq!(action_from_cmd_id(actions.len() as u32 + 1, &actions), None);
    }

    #[test]
    fn pet_submenu_includes_rig_parts_when_present() {
        let p = pet("p1", "AlphaPet", sample_parts());
        let actions = pet_actions(&p);
        let no_parts = pet_actions(&pet("p1", "AlphaPet", Vec::new()));
        assert_eq!(actions.len(), no_parts.len() + 2 + 2);
        assert!(actions.contains(&PetQuickAction::SetAllParts(true)));
        assert!(actions.contains(&PetQuickAction::SetAllParts(false)));
        assert!(actions.contains(&PetQuickAction::TogglePart("ear".to_string())));
        assert!(actions.contains(&PetQuickAction::TogglePart("tail".to_string())));
        assert_eq!(actions.last(), Some(&PetQuickAction::ClosePet));
        assert!(!no_parts.contains(&PetQuickAction::SetAllParts(true)));
    }

    #[test]
    fn global_menu_wraps_pet_actions_without_id_collisions() {
        let chars = vec!["AlphaPet".to_string(), "BetaPet".to_string()];
        let pets = vec![
            pet("p1", "AlphaPet", sample_parts()),
            pet("p2", "BetaPet", Vec::new()),
        ];
        let react = ReactPrefs::default();
        let ui = UiPrefs::default();
        let state = GlobalMenuState {
            characters: &chars,
            pets: &pets,
            pets_disabled: false,
            ghost: false,
            launch_at_login: false,
            react: &react,
            ui: &ui,
        };
        let labels = menu_labels("en");
        let mut table: Vec<TrayAction> = Vec::new();
        let menu = build_global_menu(&state, &labels, &mut table).expect("global menu");
        drop(menu);

        // Quick disable toggle sits at index 1, followed by Spawn entries.
        assert_eq!(action_from_cmd_id(1, &table), Some(TrayAction::TogglePetsDisabled));
        assert_eq!(
            action_from_cmd_id(2, &table),
            Some(TrayAction::SpawnCharacter("AlphaPet".to_string()))
        );
        assert_eq!(
            action_from_cmd_id(3, &table),
            Some(TrayAction::SpawnCharacter("BetaPet".to_string()))
        );

        // Per-pet blocks are wrapped and contiguous, in order.
        let n1 = pet_actions(&pets[0]).len();
        let n2 = pet_actions(&pets[1]).len();
        let first = 3 + 1;
        assert_eq!(
            action_from_cmd_id(first as u32, &table),
            Some(TrayAction::PetQuick {
                pet_id: "p1".to_string(),
                action: PetQuickAction::DeltaSize(10.0),
            })
        );
        assert_eq!(
            action_from_cmd_id((first + n1 - 1) as u32, &table),
            Some(TrayAction::PetQuick {
                pet_id: "p1".to_string(),
                action: PetQuickAction::ClosePet,
            })
        );
        assert_eq!(
            action_from_cmd_id((first + n1) as u32, &table),
            Some(TrayAction::PetQuick {
                pet_id: "p2".to_string(),
                action: PetQuickAction::DeltaSize(10.0),
            })
        );
        assert_eq!(
            action_from_cmd_id((first + n1 + n2 - 1) as u32, &table),
            Some(TrayAction::PetQuick {
                pet_id: "p2".to_string(),
                action: PetQuickAction::ClosePet,
            })
        );
        let pet_quick_count = table
            .iter()
            .filter(|a| matches!(a, TrayAction::PetQuick { .. }))
            .count();
        assert_eq!(pet_quick_count, n1 + n2);

        // Tail of the table: gather, ghost, reload, login, quit.
        let tail = &table[table.len() - 5..];
        assert_eq!(
            tail,
            &[
                TrayAction::GatherAll,
                TrayAction::ToggleGhost,
                TrayAction::ReloadFromSource,
                TrayAction::ToggleLaunchAtLogin,
                TrayAction::Quit,
            ]
        );
        assert_eq!(
            action_from_cmd_id(table.len() as u32, &table),
            Some(TrayAction::Quit)
        );
        assert_eq!(action_from_cmd_id(table.len() as u32 + 1, &table), None);
        assert_eq!(action_from_cmd_id(0, &table), None);
    }

    #[test]
    fn quick_settings_table_covers_all_options() {
        let chars: Vec<String> = Vec::new();
        let pets: Vec<PetMenuSnapshot> = Vec::new();
        let react = ReactPrefs::default();
        let ui = UiPrefs::default();
        let state = GlobalMenuState {
            characters: &chars,
            pets: &pets,
            pets_disabled: true,
            ghost: true,
            launch_at_login: true,
            react: &react,
            ui: &ui,
        };
        let labels = menu_labels("en");
        let mut table: Vec<TrayAction> = Vec::new();
        drop(build_global_menu(&state, &labels, &mut table).expect("global menu"));

        for n in 1..=10 {
            assert!(table.contains(&TrayAction::SetReactSense(n as f64)));
        }
        assert!(table.contains(&TrayAction::SetReactSound("music".to_string())));
        assert!(table.contains(&TrayAction::SetReactSound("all".to_string())));
        for g in [-6.0, 0.0, 6.0, 12.0] {
            assert!(table.contains(&TrayAction::SetReactGain(g)));
        }
        for k in [1, 2, 3, 5] {
            assert!(table.contains(&TrayAction::SetKeyEvery(k)));
        }
        assert!(table.contains(&TrayAction::ToggleRestIdle));
        for m in [3.0, 5.0, 10.0, 15.0] {
            assert!(table.contains(&TrayAction::SetRestMin(m)));
        }
        for px in [12.0, 14.0, 16.0, 18.0] {
            assert!(table.contains(&TrayAction::SetBubbleFont(px)));
        }
        assert!(table.contains(&TrayAction::ToggleReduceMotion));
        for code in ["en", "ko", "ja", "zh"] {
            assert!(table.contains(&TrayAction::SetLang(code.to_string())));
        }
        // No pets/characters: nothing but quick settings and the global tail.
        assert!(!table
            .iter()
            .any(|a| matches!(a, TrayAction::SpawnCharacter(_) | TrayAction::PetQuick { .. })));
        assert_eq!(table.last(), Some(&TrayAction::Quit));
    }

    #[test]
    fn menu_labels_are_localized_and_non_empty() {
        let en = menu_labels("en");
        for lang in ["ko-KR", "ja", "zh-TW"] {
            let l = menu_labels(lang);
            assert_ne!(l.spawn_char, en.spawn_char);
            assert_ne!(l.pet_quick, en.pet_quick);
            assert_ne!(l.reload, en.reload);
            assert_ne!(l.close_pet, en.close_pet);
            assert!(l.every_keys.contains("{n}"));
            assert!(l.minutes.contains("{n}"));
        }
        assert_eq!(menu_labels("fr").quit, "Quit");
        assert_eq!(fmt_n(en.every_keys, 3), "Every 3 keys");
    }
}
