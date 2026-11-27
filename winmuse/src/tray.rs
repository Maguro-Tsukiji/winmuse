//! System tray icon: the resident process's always-visible
//! presence, and the owner's local control surface.
//!
//! Hand-rolled Shell_NotifyIcon on its own thread with a hidden
//! window (the architecture's fallback route, taken deliberately to
//! keep the dependency count at zero). The icon's color mirrors the
//! operation state; the right-click menu opens Settings, starts /
//! stops serving, opens the audit log and the config folder, and
//! quits. Double-click opens Settings.

use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIF_ICON, NIF_MESSAGE,
    NIF_TIP, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, KillTimer,
    PostQuitMessage, RegisterClassExW, SetForegroundWindow, SetTimer, TrackPopupMenu, TranslateMessage,
    HICON, ICONINFO, MSG, MF_SEPARATOR, MF_STRING, TPM_RIGHTBUTTON, WNDCLASSEXW,
    WM_COMMAND, WM_DESTROY, WM_LBUTTONDBLCLK, WM_RBUTTONUP, WM_TIMER,
};

use crate::state::Shared;
use crate::status::op_state_name;

const CALLBACK_MSG: u32 = 0x8001; // WM_APP + 1
const TIMER_ID: usize = 1;
const ID_SETTINGS: usize = 1001;
const ID_TOGGLE: usize = 1002;
const ID_AUDIT: usize = 1003;
const ID_FOLDER: usize = 1004;
const ID_QUIT: usize = 1005;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WM_CONTEXTMENU_MSG: u32 = 0x007B;

struct Ctx {
    shared: Arc<Shared>,
    icon: HICON,
    state_key: String,
}

// The tray icon handle is only touched while holding the Ctx
// mutex, from the tray thread's message handlers.
unsafe impl Send for Ctx {}
unsafe impl Sync for Ctx {}

static CTX: OnceLock<std::sync::Mutex<Ctx>> = OnceLock::new();

fn ctx() -> std::sync::MutexGuard<'static, Ctx> {
    CTX.get().unwrap().lock().unwrap()
}

fn state_color(state: &str, serving: bool) -> (u8, u8, u8) {
    if !serving {
        (107, 114, 128) // stopped: gray
    } else {
        match state {
            "operating" => (59, 130, 246),
            "waiting" => (245, 158, 11),
            "cancelled" => (248, 113, 113),
            _ => (34, 197, 94), // idle: green
        }
    }
}

fn state_label(state: &str, serving: bool) -> &'static str {
    let ja = crate::config::os_is_japanese();
    if !serving {
        if ja { "停止中" } else { "Stopped" }
    } else {
        match (state, ja) {
            ("operating", true) => "操作中",
            ("operating", false) => "Operating",
            ("waiting", true) => "許可待ち",
            ("waiting", false) => "Waiting for OK",
            ("cancelled", true) => "キャンセル済み",
            ("cancelled", false) => "Cancelled",
            (_, true) => "待機中",
            (_, false) => "Idle",
        }
    }
}

fn tip_text(state: &str, serving: bool, profile: &str) -> String {
    if crate::config::os_is_japanese() {
        format!(
            "winmuse — {} ・ 権限: {}",
            state_label(state, serving),
            profile
        )
    } else {
        format!(
            "winmuse — {} - Profile: {}",
            state_label(state, serving),
            profile
        )
    }
}

/// Render a simple filled-circle icon in the state color.
fn make_icon(rgb: (u8, u8, u8)) -> HICON {
    const S: i32 = 32;
    unsafe {
        let screen_dc = GetDC(None);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: S,
                biHeight: -S,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = match CreateDIBSection(Some(screen_dc), &mut bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => {
                ReleaseDC(None, screen_dc);
                return HICON::default();
            }
        };
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (S * S * 4) as usize);
        for y in 0..S {
            for x in 0..S {
                let dx = x as f32 - 15.5;
                let dy = y as f32 - 15.5;
                let dist = (dx * dx + dy * dy).sqrt();
                let a = ((14.5 - dist).clamp(0.0, 1.0) * 255.0) as u8;
                let i = ((y * S + x) * 4) as usize;
                px[i] = rgb.2;
                px[i + 1] = rgb.1;
                px[i + 2] = rgb.0;
                px[i + 3] = a;
            }
        }
        let mut mask_bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mask = CreateDIBSection(Some(screen_dc), &mut bmi, DIB_RGB_COLORS, &mut mask_bits, None, 0)
            .unwrap_or_default();
        std::ptr::write_bytes(mask_bits as *mut u8, 0, (S * S * 4) as usize);
        let info = ICONINFO {
            fIcon: windows::core::BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        ReleaseDC(None, screen_dc);
        icon
    }
}

fn notify_data(hwnd: HWND, icon: HICON, tip: &str) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = CALLBACK_MSG;
    data.hIcon = icon;
    for (i, u) in tip.encode_utf16().take(127).enumerate() {
        data.szTip[i] = u;
    }
    data
}

fn refresh_icon(hwnd: HWND) {
    let (key, icon, tip) = {
        let c = ctx();
        let serving = c.shared.control.running.load(Ordering::Relaxed);
        let state = op_state_name(&c.shared);
        let profile = c.shared.config.read().unwrap().profile.as_str().to_string();
        let key = format!("{serving}|{state}|{profile}");
        if key == c.state_key {
            return;
        }
        let icon = make_icon(state_color(state, serving));
        let tip = tip_text(state, serving, &profile);
        (key, icon, tip)
    };
    let data = notify_data(hwnd, icon, &tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
    let mut c = ctx();
    let old = std::mem::replace(&mut c.icon, icon);
    c.state_key = key;
    if !old.0.is_null() {
        unsafe {
            let _ = DestroyIcon(old);
        }
    }
}

pub fn launch_settings() {
    if let Ok(exe) = std::env::current_exe() {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new(exe)
            .arg("settings")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

fn show_menu(hwnd: HWND) {
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        let running = ctx().shared.control.running.load(Ordering::Relaxed);
        let item = |id: usize, text: &str| {
            let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                id,
                windows::core::PCWSTR::from_raw(wide.as_ptr()),
            );
        };
        let ja = crate::config::os_is_japanese();
        item(ID_SETTINGS, if ja { "設定を開く…" } else { "Open settings…" });
        item(
            ID_TOGGLE,
            match (running, ja) {
                (true, true) => "サーバーを停止",
                (true, false) => "Stop server",
                (false, true) => "サーバーを開始",
                (false, false) => "Start server",
            },
        );
        item(ID_AUDIT, if ja { "監査ログを開く" } else { "Open audit log" });
        item(ID_FOLDER, if ja { "設定フォルダを開く" } else { "Open config folder" });
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
        item(ID_QUIT, if ja { "終了" } else { "Quit" });
        let _ = SetForegroundWindow(hwnd);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

fn on_command(hwnd: HWND, id: usize) {
    match id {
        ID_SETTINGS => launch_settings(),
        ID_TOGGLE => {
            let c = ctx();
            let want = c.shared.control.desired.load(Ordering::Relaxed);
            c.shared.control.desired.store(!want, Ordering::Relaxed);
        }
        ID_AUDIT => {
            let path = ctx().shared.audit_path.clone();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if !path.exists() {
                let _ = std::fs::write(&path, "");
            }
            let _ = std::process::Command::new("notepad.exe").arg(&path).spawn();
        }
        ID_FOLDER => {
            let dir = crate::config::data_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
        }
        ID_QUIT => {
            // process::exit skips the message loop's exit path, so
            // remove the icon here — otherwise a ghost icon stays
            // in the notification area.
            let data = notify_data(hwnd, HICON::default(), "");
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
            std::process::exit(0);
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            refresh_icon(hwnd);
            LRESULT(0)
        }
        x if x == CALLBACK_MSG => {
            // NOTIFYICON_VERSION_4 packs the event in the low word
            // of lparam and the icon id in the high word.
            let event = lparam.0 as u32 & 0xFFFF;
            match event {
                WM_RBUTTONUP | WM_CONTEXTMENU_MSG => show_menu(hwnd),
                WM_LBUTTONDBLCLK => launch_settings(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            on_command(hwnd, wparam.0 & 0xFFFF);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn spawn(shared: Arc<Shared>) {
    let _ = CTX.set(std::sync::Mutex::new(Ctx {
        shared,
        icon: HICON::default(),
        state_key: String::new(),
    }));
    std::thread::spawn(move || unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(m) => HINSTANCE(m.0),
            Err(_) => return,
        };
        let class_name = windows::core::w!("WinmuseTray");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        let hwnd = match CreateWindowExW(
            Default::default(),
            class_name,
            windows::core::w!("winmuse"),
            Default::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst),
            None,
        ) {
            Ok(h) => h,
            Err(_) => return,
        };
        // Initial icon.
        let (icon, tip) = {
            let c = ctx();
            let serving = c.shared.control.running.load(Ordering::Relaxed);
            let state = op_state_name(&c.shared);
            let profile = c.shared.config.read().unwrap().profile.as_str().to_string();
            let icon = make_icon(state_color(state, serving));
            let tip = tip_text(state, serving, &profile);
            (icon, tip)
        };
        {
            let mut c = ctx();
            c.icon = icon;
        }
        let data = notify_data(hwnd, icon, &tip);
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
        // Version 4 behavior (event packed in the low word of
        // lparam), matching the modern notification area.
        let mut vdata = notify_data(hwnd, icon, &tip);
        vdata.Anonymous.uVersion = 4; // NOTIFYICON_VERSION_4
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &vdata);
        let _ = SetTimer(Some(hwnd), TIMER_ID, 500, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = KillTimer(Some(hwnd), TIMER_ID);
        let data = notify_data(hwnd, HICON::default(), "");
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    });
}
