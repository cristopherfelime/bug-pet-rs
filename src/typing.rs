use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
    RAWINPUTHEADER, RIDEV_INPUTSINK, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::WindowsAndMessaging::RI_KEY_BREAK;

/// Zero-overhead native Win32 Raw Input global keyboard listener (`keys.js` + `bug-clicker-typing.ps1` replacement)
/// with 0 child PowerShell processes and 0 polling timers.
pub struct TypingWatcher {
    count: u32,
    last_trigger_s: f64,
}

impl TypingWatcher {
    pub fn register(hwnd: HWND) -> Result<Self, String> {
        let rid = [RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x06,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        }];
        unsafe {
            RegisterRawInputDevices(&rid, std::mem::size_of::<RAWINPUTDEVICE>() as u32)
                .map_err(|e| format!("RegisterRawInputDevices failed: {e}"))?;
        }
        Ok(Self {
            count: 0,
            last_trigger_s: -1.0,
        })
    }

    pub fn handle_wm_input(
        &mut self,
        lparam_isize: isize,
        key_every: u32,
        now_s: f64,
    ) -> bool {
        let mut raw = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        let hdr_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;
        let res = unsafe {
            GetRawInputData(
                HRAWINPUT(lparam_isize as *mut _),
                RID_INPUT,
                Some(&mut raw as *mut RAWINPUT as *mut _),
                &mut size,
                hdr_size,
            )
        };
        if res == u32::MAX {
            return false;
        }
        if raw.header.dwType != RIM_TYPEKEYBOARD.0 {
            return false;
        }
        let flags = unsafe { raw.data.keyboard.Flags };
        if (flags & RI_KEY_BREAK as u16) != 0 {
            return false;
        }

        self.count = self.count.saturating_add(1);
        let every = key_every.max(1);
        if self.count < every {
            return false;
        }
        if self.last_trigger_s >= 0.0 && (now_s - self.last_trigger_s) < 0.060 {
            return false;
        }
        self.count = 0;
        self.last_trigger_s = now_s;
        true
    }
}
