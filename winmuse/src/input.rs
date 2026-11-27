//! Mouse and keyboard input via Win32 SendInput (the same mechanism
//! pyautogui uses), plus clipboard paste for Unicode text.

use std::thread::sleep;
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, VkKeyScanW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, VIRTUAL_KEY,
    VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME,
    VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT,
    VK_SNAPSHOT, VK_SPACE, VK_TAB, VK_UP,
};

use crate::capture::screen_size;

fn send(inputs: &[INPUT]) -> Result<(), String> {
    let n = unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32)
    };
    if n as usize != inputs.len() {
        return Err(format!("SendInput sent {n} of {}", inputs.len()));
    }
    Ok(())
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Convert real pixel coordinates to SendInput's 0..=65535 absolute space.
fn to_abs(x: i32, y: i32) -> (i32, i32) {
    let (w, h) = screen_size();
    let nx = if w > 1 { x * 65535 / (w - 1) } else { 0 };
    let ny = if h > 1 { y * 65535 / (h - 1) } else { 0 };
    (nx, ny)
}

pub fn move_to(x: i32, y: i32) -> Result<(), String> {
    let (ax, ay) = to_abs(x, y);
    send(&[mouse_input(ax, ay, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE)])
}

fn button_flags(button: &str, down: bool) -> Result<MOUSE_EVENT_FLAGS, String> {
    match (button, down) {
        ("left", true) => Ok(MOUSEEVENTF_LEFTDOWN),
        ("left", false) => Ok(MOUSEEVENTF_LEFTUP),
        ("right", true) => Ok(MOUSEEVENTF_RIGHTDOWN),
        ("right", false) => Ok(MOUSEEVENTF_RIGHTUP),
        ("middle", true) => Ok(MOUSEEVENTF_MIDDLEDOWN),
        ("middle", false) => Ok(MOUSEEVENTF_MIDDLEUP),
        _ => Err(format!("unknown button '{button}'")),
    }
}

pub fn click(x: i32, y: i32, button: &str, clicks: i64) -> Result<(), String> {
    move_to(x, y)?;
    let down = button_flags(button, true)?;
    let up = button_flags(button, false)?;
    for _ in 0..clicks.max(1) {
        send(&[mouse_input(0, 0, 0, down)])?;
        sleep(Duration::from_millis(30));
        send(&[mouse_input(0, 0, 0, up)])?;
        sleep(Duration::from_millis(30));
    }
    Ok(())
}

pub fn drag(x1: i32, y1: i32, x2: i32, y2: i32, duration_s: f64) -> Result<(), String> {
    move_to(x1, y1)?;
    send(&[mouse_input(0, 0, 0, MOUSEEVENTF_LEFTDOWN)])?;
    let steps = 20;
    let step_ms = ((duration_s.max(0.0) * 1000.0) / steps as f64) as u64;
    for i in 1..=steps {
        let x = x1 + (x2 - x1) * i / steps;
        let y = y1 + (y2 - y1) * i / steps;
        move_to(x, y)?;
        if step_ms > 0 {
            sleep(Duration::from_millis(step_ms));
        }
    }
    send(&[mouse_input(0, 0, 0, MOUSEEVENTF_LEFTUP)])
}

pub fn scroll(amount: i64, at: Option<(i32, i32)>) -> Result<(), String> {
    if let Some((x, y)) = at {
        move_to(x, y)?;
    }
    // WHEEL_DELTA (120) per notch; positive = up, matching pyautogui.
    let data = (amount * 120) as u32;
    send(&[mouse_input(0, 0, data, MOUSEEVENTF_WHEEL)])
}

fn vk_for(name: &str) -> Option<VIRTUAL_KEY> {
    let v = match name {
        "ctrl" | "control" => VK_CONTROL,
        "shift" => VK_SHIFT,
        "alt" => VK_MENU,
        "win" | "windows" | "cmd" | "command" | "meta" => VK_LWIN,
        "enter" | "return" => VK_RETURN,
        "esc" | "escape" => VK_ESCAPE,
        "tab" => VK_TAB,
        "space" => VK_SPACE,
        "backspace" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "insert" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "capslock" => VK_CAPITAL,
        "printscreen" => VK_SNAPSHOT,
        _ => {
            if let Some(rest) = name.strip_prefix('f') {
                if let Ok(n) = rest.parse::<u16>() {
                    if (1..=24).contains(&n) {
                        return Some(VIRTUAL_KEY(VK_F1.0 + n - 1));
                    }
                }
            }
            let mut chars = name.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_alphanumeric() {
                    return Some(VIRTUAL_KEY(c.to_ascii_uppercase() as u16));
                }
                let scan = unsafe { VkKeyScanW(c as u16) };
                if scan != -1 {
                    return Some(VIRTUAL_KEY((scan & 0xff) as u16));
                }
            }
            return None;
        }
    };
    Some(v)
}

/// Press a key or shortcut: 'enter', 'ctrl+c', 'alt+tab', ...
/// Modifiers go down in order and come up in reverse (pyautogui
/// hotkey semantics). Returns the normalized combo.
pub fn key_press(spec: &str) -> Result<String, String> {
    let parts: Vec<String> = spec
        .split('+')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("no keys given".into());
    }
    let mut vks = Vec::new();
    for p in &parts {
        vks.push(vk_for(p).ok_or_else(|| format!("unknown key '{p}'"))?);
    }
    let mut inputs: Vec<INPUT> = vks.iter().map(|&vk| key_input(vk, false)).collect();
    inputs.extend(vks.iter().rev().map(|&vk| key_input(vk, true)));
    send(&inputs)?;
    Ok(parts.join("+"))
}

/// Paste text via the clipboard (Ctrl+V), like the Python server:
/// Unicode-safe, and replaces the clipboard contents.
pub fn type_text(text: &str) -> Result<usize, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    cb.set_text(text.to_string())
        .map_err(|e| format!("clipboard set: {e}"))?;
    drop(cb);
    sleep(Duration::from_millis(40));
    key_press("ctrl+v")?;
    Ok(text.chars().count())
}
