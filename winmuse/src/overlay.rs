//! Operation overlay: a click-through, always-on-top layered window
//! showing a soft blue glow frame around the whole screen plus a
//! toast at the top centre ("muse is operating — press ESC to
//! cancel"). ESC is a real cancel: this thread watches it with
//! GetAsyncKeyState, flips the shared state to cancelled, flashes a
//! red toast, and action tools refuse until operation_begin re-arms.
//!
//! Rendering is fully software-side: the glow is a per-pixel function
//! of the distance to the nearest screen edge (softer and cheaper
//! than the Python version's stacked strokes + Gaussian blur), the
//! toast pill is a rounded-rect SDF, and only the glyphs come from
//! GDI (drawn in color over black, then recovered as coverage).
//! The buffer is straight-alpha RGBA; it is premultiplied only when
//! presented through UpdateLayeredWindow. A preview PNG of each mode
//! is saved under %TEMP%\winmuse\.

use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC,
    DeleteObject, GetDC, GetTextExtentPoint32W, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    TextOutW, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassExW,
    SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, HWND_TOPMOST, MSG,
    PM_REMOVE, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};

use crate::capture::screen_size;
use crate::state::{OverlayMode, Shared, IDLE_TIMEOUT_S};

const VK_ESCAPE: i32 = 0x1B;
const VK_F9: i32 = 0x78;

pub fn spawn(shared: Arc<Shared>) {
    std::thread::spawn(move || run(shared));
}

fn frame_color(mode: OverlayMode) -> (u8, u8, u8) {
    match mode {
        OverlayMode::Active => (59, 130, 246),
        OverlayMode::Waiting => (245, 158, 11),
        OverlayMode::Cancelled => (248, 113, 113),
    }
}

fn accent_color(mode: OverlayMode) -> (u8, u8, u8) {
    match mode {
        OverlayMode::Active => (96, 165, 250),
        OverlayMode::Waiting => (251, 191, 36),
        OverlayMode::Cancelled => (248, 113, 113),
    }
}

/// Toast texts, in the configured language (Japanese/English
/// presets; English is the default on non-Japanese Windows). The
/// active/waiting sub-line carries the permission profile, as the
/// permissions design requires (e.g. 権限: guided). Custom title/sub
/// from the config override the operating toast's preset.
fn texts(mode: OverlayMode, cfg: &crate::config::Config) -> (String, String) {
    let profile = cfg.profile.as_str();
    let ja = crate::config::effective_lang(cfg) == "ja";
    match mode {
        OverlayMode::Active => {
            let (mut title, mut sub) = if ja {
                (
                    "muse が操作しています".to_string(),
                    format!("ESC キーでキャンセル ・ 権限: {profile}"),
                )
            } else {
                (
                    "muse is operating".to_string(),
                    format!("Press ESC to cancel - Profile: {profile}"),
                )
            };
            if !cfg.overlay_title.is_empty() {
                title = cfg.overlay_title.clone();
            }
            if !cfg.overlay_sub.is_empty() {
                sub = cfg.overlay_sub.clone();
            }
            (title, sub)
        }
        OverlayMode::Waiting => {
            if ja {
                (
                    "操作の許可待ちです".to_string(),
                    format!("F9 キーで許可 ・ ESC で拒否 ・ 権限: {profile}"),
                )
            } else {
                (
                    "Waiting for your OK".to_string(),
                    format!("Press F9 to allow - ESC to deny - Profile: {profile}"),
                )
            }
        }
        OverlayMode::Cancelled => {
            if ja {
                (
                    "キャンセルしました".to_string(),
                    "muse の操作を停止しました".to_string(),
                )
            } else {
                ("Cancelled".to_string(), "muse has stopped".to_string())
            }
        }
    }
}

/// Source-over blend of a straight-alpha src pixel into dst (RGBA).
fn over(dst: &mut [u8], sr: u8, sg: u8, sb: u8, sa: u8) {
    if sa == 0 {
        return;
    }
    if sa == 255 {
        dst[0] = sr;
        dst[1] = sg;
        dst[2] = sb;
        dst[3] = 255;
        return;
    }
    let s = sa as f32 / 255.0;
    let da = dst[3] as f32 / 255.0;
    let out_a = s + da * (1.0 - s);
    if out_a <= 0.0 {
        dst[0] = 0;
        dst[1] = 0;
        dst[2] = 0;
        dst[3] = 0;
        return;
    }
    let src = [sr, sg, sb];
    for i in 0..3 {
        let sc = src[i] as f32 / 255.0;
        let dc = dst[i] as f32 / 255.0;
        dst[i] = (((sc * s + dc * da * (1.0 - s)) / out_a) * 255.0).round() as u8;
    }
    dst[3] = (out_a * 255.0).round() as u8;
}

fn sd_round_box(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let dx = (px - cx).abs() - (hw - r);
    let dy = (py - cy).abs() - (hh - r);
    let ax = dx.max(0.0);
    let ay = dy.max(0.0);
    (ax * ax + ay * ay).sqrt() + dx.max(dy).min(0.0) - r
}

fn make_font(px: i32, weight: i32) -> HFONT {
    let face: Vec<u16> = "Yu Gothic\0".encode_utf16().collect();
    unsafe {
        CreateFontW(
            -px,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            PCWSTR::from_raw(face.as_ptr()),
        )
    }
}

/// Render the two toast text lines with GDI into a small RGBA buffer
/// (title at (0,0) in white, sub at (0,42) in slate). GDI draws in
/// color over a black background, so coverage is recovered from the
/// brightest channel of each band; the color itself is known.
fn render_text_block(mode: OverlayMode, cfg: &crate::config::Config) -> Option<(Vec<u8>, u32, u32)> {
    let (title, sub) = texts(mode, cfg);
    let (title, sub) = (title.as_str(), sub.as_str());
    let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    unsafe {
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let tmp = CreateCompatibleBitmap(screen_dc, 1, 1);
        let old_bmp = SelectObject(mem_dc, HGDIOBJ(tmp.0));
        let f_title = make_font(34, 700);
        let f_sub = make_font(23, 500);

        let mut sz = SIZE::default();
        let old_font = SelectObject(mem_dc, HGDIOBJ(f_title.0));
        let _ = GetTextExtentPoint32W(mem_dc, &wide(title), &mut sz);
        let title_w = sz.cx;
        let _ = SelectObject(mem_dc, HGDIOBJ(f_sub.0));
        let mut sz2 = SIZE::default();
        let _ = GetTextExtentPoint32W(mem_dc, &wide(sub), &mut sz2);
        let sub_w = sz2.cx;
        let _ = SelectObject(mem_dc, old_font);

        let bw = title_w.max(sub_w).max(1) as u32;
        let bh = 42 + 30;
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: bw as i32,
                biHeight: -(bh as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(
            Some(screen_dc),
            &mut bmi,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
        .ok()?;
        std::ptr::write_bytes(bits as *mut u8, 0, (bw * bh * 4) as usize);
        let old2 = SelectObject(mem_dc, HGDIOBJ(dib.0));
        let _ = SetBkMode(mem_dc, TRANSPARENT);
        let of = SelectObject(mem_dc, HGDIOBJ(f_title.0));
        let _ = SetTextColor(mem_dc, COLORREF(0x00FF_FFFF));
        let _ = TextOutW(mem_dc, 0, 0, &wide(title));
        let _ = SelectObject(mem_dc, HGDIOBJ(f_sub.0));
        let _ = SetTextColor(mem_dc, COLORREF(0x00E1_D5CB)); // (203,213,225) as 0x00BBGGRR
        let _ = TextOutW(mem_dc, 0, 42, &wide(sub));
        let _ = SelectObject(mem_dc, of);

        let src = std::slice::from_raw_parts(bits as *const u8, (bw * bh * 4) as usize);
        let mut out = vec![0u8; (bw * bh * 4) as usize];
        for i in 0..(bw * bh) as usize {
            let (b, g, r) = (src[i * 4] as u32, src[i * 4 + 1] as u32, src[i * 4 + 2] as u32);
            let _ = g;
            let y = (i as u32 / bw) as i32;
            if y < 42 {
                if r > 0 {
                    out[i * 4] = 255;
                    out[i * 4 + 1] = 255;
                    out[i * 4 + 2] = 255;
                    out[i * 4 + 3] = r as u8;
                }
            } else if b > 0 {
                let cov = (b as f32 / 225.0).min(1.0);
                out[i * 4] = 203;
                out[i * 4 + 1] = 213;
                out[i * 4 + 2] = 225;
                out[i * 4 + 3] = (cov * 255.0) as u8;
            }
        }
        let _ = SelectObject(mem_dc, old2);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteObject(HGDIOBJ(f_title.0));
        let _ = DeleteObject(HGDIOBJ(f_sub.0));
        let _ = SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(HGDIOBJ(tmp.0));
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        Some((out, bw, bh))
    }
}

/// Render the full-screen overlay bitmap (straight RGBA) for a mode,
/// and save a preview PNG under %TEMP%\winmuse\.
fn render_bitmap(mode: OverlayMode, cfg: &crate::config::Config, w: i32, h: i32) -> Vec<u8> {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let (fr, fg, fb) = frame_color(mode);
    let (br, bg, bb) = accent_color(mode);

    // --- glow frame: alpha falls off with distance from the edge ---
    for y in 0..h {
        for x in 0..w {
            let d = x.min(y).min(w - 1 - x).min(h - 1 - y) as f32;
            let mut a = if d < 78.0 {
                206.0 * (1.0 - d / 78.0).powf(1.7)
            } else {
                0.0
            };
            if (2.0..=4.0).contains(&d) {
                a = a.max(190.0); // crisp inner edge line
            }
            if a >= 1.0 {
                let i = ((y * w + x) * 4) as usize;
                buf[i] = fr;
                buf[i + 1] = fg;
                buf[i + 2] = fb;
                buf[i + 3] = a.round() as u8;
            }
        }
    }

    // --- toast pill, top centre ---
    if let Some((text, tw, th)) = render_text_block(mode, cfg) {
        let pill_w = tw as i32 + 46 + 58;
        let pill_h = 96;
        let x0 = (w - pill_w) / 2;
        let y0 = 26;
        let cx = x0 as f32 + pill_w as f32 / 2.0;
        let cy = y0 as f32 + pill_h as f32 / 2.0;
        let hw = pill_w as f32 / 2.0;
        let hh = pill_h as f32 / 2.0;
        for y in (y0 - 2)..(y0 + pill_h + 2) {
            for x in (x0 - 2)..(x0 + pill_w + 2) {
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let d = sd_round_box(x as f32 + 0.5, y as f32 + 0.5, cx, cy, hw, hh, 20.0);
                if d < 1.0 {
                    let i = ((y * w + x) * 4) as usize;
                    let cov = (0.5 - d).clamp(0.0, 1.0);
                    over(&mut buf[i..i + 4], 8, 14, 32, (218.0 * cov) as u8);
                    let ring = 1.0 - (d.abs() / 1.5).min(1.0);
                    if ring > 0.0 {
                        over(&mut buf[i..i + 4], br, bg, bb, (255.0 * ring) as u8);
                    }
                }
            }
        }
        // status dot
        let (dcx, dcy) = (x0 + 34, y0 + 48);
        for y in (dcy - 12)..=(dcy + 12) {
            for x in (dcx - 12)..=(dcx + 12) {
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let dist = (((x - dcx) as f32).powi(2) + ((y - dcy) as f32).powi(2)).sqrt();
                if dist < 11.0 {
                    let a = ((10.5 - dist).clamp(0.0, 1.0) * 255.0) as u8;
                    let i = ((y * w + x) * 4) as usize;
                    over(&mut buf[i..i + 4], br, bg, bb, a);
                }
            }
        }
        // text
        for ty in 0..th as i32 {
            for tx in 0..tw as i32 {
                let si = ((ty * tw as i32 + tx) * 4) as usize;
                let a = text[si + 3];
                if a == 0 {
                    continue;
                }
                let (dx, dy) = (x0 + 58 + tx, y0 + 14 + ty);
                if dx < 0 || dy < 0 || dx >= w || dy >= h {
                    continue;
                }
                let di = ((dy * w + dx) * 4) as usize;
                over(&mut buf[di..di + 4], text[si], text[si + 1], text[si + 2], a);
            }
        }
    }

    // preview copy for the owner / for muse to show what it looks like
    let dir = std::env::temp_dir().join("winmuse");
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, buf.clone()) {
            let _ = img.save(dir.join(format!("cu-overlay-preview-{}.png", mode.as_str())));
        }
    }
    buf
}

/// Push an RGBA bitmap into the layered window (premultiplied BGRA).
fn present(hwnd: HWND, rgba: &[u8], w: i32, h: i32) -> bool {
    let mut bgra = vec![0u8; rgba.len()];
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        let a = px[3] as u32;
        bgra[i * 4] = (px[2] as u32 * a / 255) as u8;
        bgra[i * 4 + 1] = (px[1] as u32 * a / 255) as u8;
        bgra[i * 4 + 2] = (px[0] as u32 * a / 255) as u8;
        bgra[i * 4 + 3] = px[3];
    }
    unsafe {
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(Some(screen_dc), &mut bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(d) => d,
            Err(_) => {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(None, screen_dc);
                return false;
            }
        };
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
        let old = SelectObject(mem_dc, HGDIOBJ(dib.0));
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1, // AC_SRC_ALPHA
        };
        let ok = UpdateLayeredWindow(
            hwnd,
            Some(screen_dc),
            Some(&POINT { x: 0, y: 0 }),
            Some(&SIZE { cx: w, cy: h }),
            Some(mem_dc),
            Some(&POINT { x: 0, y: 0 }),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
        .is_ok();
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        ok
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn run(shared: Arc<Shared>) {
    let (w, h) = screen_size();
    if w <= 0 || h <= 0 {
        return;
    }
    let hwnd = unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(m) => HINSTANCE(m.0),
            Err(_) => return,
        };
        let class_name = windows::core::w!("WinmuseOverlay");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        let exstyle = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW;
        let hwnd = match CreateWindowExW(
            exstyle,
            class_name,
            windows::core::w!(""),
            WS_POPUP,
            0,
            0,
            w,
            h,
            None,
            None,
            Some(hinst),
            None,
        ) {
            Ok(h) => h,
            Err(_) => return,
        };
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, w, h, SWP_NOACTIVATE);
        hwnd
    };
    {
        let mut op = shared.op.lock().unwrap();
        op.hwnd = hwnd.0 as isize;
    }

    let mut bitmaps: Vec<((OverlayMode, String), Vec<u8>)> = Vec::new();
    let mut esc_was_down = false;
    let mut f9_was_down = false;
    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let now = Instant::now();

        // --- ESC cancel (only while an operation is running) ---
        // 0x8000 = key currently down; 0x0001 = pressed since the last
        // call (a latch, so even a very short tap is never missed).
        let ks = unsafe { GetAsyncKeyState(VK_ESCAPE) } as u16;
        let esc_down = ks & 0x8000 != 0;
        let esc_hit = esc_down || ks & 0x0001 != 0;
        // --- F9 arm (only while operation_begin waits for consent) ---
        let ks9 = unsafe { GetAsyncKeyState(VK_F9) } as u16;
        let f9_down = ks9 & 0x8000 != 0;
        let f9_hit = f9_down || ks9 & 0x0001 != 0;
        {
            let mut op = shared.op.lock().unwrap();
            if f9_hit && !f9_was_down && op.awaiting_arm {
                op.armed = true;
                op.awaiting_arm = false;
                op.active = true;
                op.last_call = Some(now);
            }
        }
        f9_was_down = f9_down || f9_hit;
        {
            let mut op = shared.op.lock().unwrap();
            if esc_hit && !esc_was_down && (op.active || op.awaiting_arm) && !op.cancelled {
                op.cancelled = true;
                op.awaiting_arm = false;
                op.armed = false;
                op.flash_until = Some(now + Duration::from_millis(1800));
            }
            // --- idle safety: never leave the frame stuck on ---
            if op.active && !op.cancelled {
                if let Some(lc) = op.last_call {
                    if lc.elapsed().as_secs_f64() > IDLE_TIMEOUT_S {
                        op.active = false;
                    }
                }
            }
        }
        esc_was_down = esc_down || esc_hit;

        let want = {
            let op = shared.op.lock().unwrap();
            let m = if op.flash_until.map_or(false, |f| now < f) {
                Some(OverlayMode::Cancelled)
            } else if op.awaiting_arm {
                Some(OverlayMode::Waiting)
            } else if op.active && !op.cancelled {
                Some(OverlayMode::Active)
            } else {
                None
            };
            let overlay_on = shared.config.read().unwrap().overlay_enabled;
            if op.suspend > 0 || !overlay_on {
                None
            } else {
                m
            }
        };
        let (visible, shown) = {
            let op = shared.op.lock().unwrap();
            (op.visible, op.shown_mode)
        };
        match want {
            None => {
                if visible {
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    }
                    let mut op = shared.op.lock().unwrap();
                    op.visible = false;
                    op.shown_mode = None;
                }
            }
            Some(mode) => {
                let cfg = shared.config.read().unwrap().clone();
                let key = (
                    mode,
                    format!(
                        "{}|{}|{}|{}",
                        cfg.profile.as_str(),
                        crate::config::effective_lang(&cfg),
                        cfg.overlay_title,
                        cfg.overlay_sub
                    ),
                );
                if !visible || shown != Some(mode) || !bitmaps.iter().any(|(k, _)| *k == key) {
                    if !bitmaps.iter().any(|(k, _)| *k == key) {
                        let bmp = render_bitmap(mode, &cfg, w, h);
                        bitmaps.push((key.clone(), bmp));
                    }
                    let bmp = &bitmaps.iter().find(|(k, _)| *k == key).unwrap().1;
                    if present(hwnd, bmp, w, h) {
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        }
                        let mut op = shared.op.lock().unwrap();
                        op.visible = true;
                        op.shown_mode = Some(mode);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
