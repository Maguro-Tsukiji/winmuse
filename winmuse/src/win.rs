//! Window enumeration and focusing (EnumWindows / SetForegroundWindow).

use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow,
    ShowWindow, WindowFromPoint, GA_ROOT, SW_RESTORE,
};

pub fn cursor_pos() -> (i32, i32) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}

fn window_title(hwnd: HWND) -> Option<String> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// All visible windows with non-empty titles: (hwnd, title).
pub fn list_windows() -> Vec<(isize, String)> {
    let mut out: Vec<(isize, String)> = Vec::new();
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let v = &mut *(lparam.0 as *mut Vec<(isize, String)>);
        if let Some(title) = window_title(hwnd) {
            v.push((hwnd.0 as isize, title));
        }
        BOOL::from(true)
    }
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// Bring the first visible window whose title contains `title`
/// (case-insensitive) to the foreground.
pub fn focus_window(title: &str) -> Result<String, String> {
    let needle = title.to_lowercase();
    let found = list_windows()
        .into_iter()
        .find(|(_, t)| t.to_lowercase().contains(&needle));
    let (raw, t) = found.ok_or_else(|| format!("no window matching '{title}'"))?;
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() {
            Ok(format!("focused '{t}' (hwnd {raw})"))
        } else {
            Err(format!(
                "SetForegroundWindow failed for '{t}' (Windows foreground lock); \
                 clicking a visible area of the window raises it instead"
            ))
        }
    }
}

/// Executable file name (e.g. "chrome.exe") of a process, or None.
fn process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        ok.ok()?;
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        path.rsplit('\\').next().map(|s| s.to_string())
    }
}

/// Executable name of the process owning a window.
pub fn process_name_of_hwnd(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        process_name(pid)
    }
}

/// Executable name of the process owning the top-level window under
/// a screen point (real pixels). The click-through overlay window is
/// skipped by WindowFromPoint itself.
pub fn process_name_at_point(x: i32, y: i32) -> Option<String> {
    unsafe {
        let hwnd = WindowFromPoint(POINT { x, y });
        if hwnd.0.is_null() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        process_name_of_hwnd(if root.0.is_null() { hwnd } else { root })
    }
}

/// Executable name of the process owning the foreground window —
/// where keyboard input would land.
pub fn foreground_process_name() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        process_name_of_hwnd(hwnd)
    }
}

/// Executable name of the process owning the first visible window
/// whose title contains `title` (the focus_window target).
pub fn process_name_of_window_title(title: &str) -> Option<String> {
    let needle = title.to_lowercase();
    let found = list_windows()
        .into_iter()
        .find(|(_, t)| t.to_lowercase().contains(&needle))?;
    process_name_of_hwnd(HWND(found.0 as *mut core::ffi::c_void))
}

/// Currently running apps that own a visible window:
/// (process name, a representative window title), sorted by name.
/// Used by the settings GUI's allowlist checklist.
pub fn running_apps() -> Vec<(String, String)> {
    let mut seen: Vec<(String, String)> = Vec::new();
    for (hwnd, title) in list_windows() {
        if let Some(proc) = process_name_of_hwnd(HWND(hwnd as *mut core::ffi::c_void)) {
            if !seen.iter().any(|(p, _)| p.eq_ignore_ascii_case(&proc)) {
                seen.push((proc, title));
            }
        }
    }
    seen.sort();
    seen
}
