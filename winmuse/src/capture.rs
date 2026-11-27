//! Screen capture via GDI BitBlt (no CAPTUREBLT flag, so layered
//! windows — including our own operation overlay — never appear in
//! captures, same as the Python server's mss path).

use std::sync::Arc;

use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::state::Shared;

pub const MAX_WIDTH: u32 = 1280;

pub fn screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// Capture the primary display, scale to at most MAX_WIDTH px wide,
/// update the shared scale state, and return PNG bytes.
pub fn grab_scaled_png(shared: &Arc<Shared>) -> Result<Vec<u8>, String> {
    let (w, h) = screen_size();
    if w <= 0 || h <= 0 {
        return Err(format!("no display (metrics {w}x{h})"));
    }
    let mut bgra = capture_bgra(w, h)?;
    // BGRA -> RGBA
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, bgra)
        .ok_or_else(|| "frame buffer size mismatch".to_string())?;

    let (sw, sh, scale) = if (w as u32) > MAX_WIDTH {
        let s = MAX_WIDTH as f64 / w as f64;
        (MAX_WIDTH, ((h as f64) * s).round() as u32, s)
    } else {
        (w as u32, h as u32, 1.0)
    };
    let out = if scale != 1.0 {
        image::imageops::resize(&img, sw, sh, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    {
        let mut st = shared.scale.lock().unwrap();
        st.scale = scale;
        st.real_w = w as u32;
        st.real_h = h as u32;
        st.scaled_w = sw;
        st.scaled_h = sh;
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(out)
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(buf.into_inner())
}

fn capture_bgra(w: i32, h: i32) -> Result<Vec<u8>, String> {
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err("GetDC failed".into());
        }
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let bmp = CreateCompatibleBitmap(screen_dc, w, h);
        let old = SelectObject(mem_dc, HGDIOBJ(bmp.0));
        let blit = BitBlt(mem_dc, 0, 0, w, h, Some(screen_dc), 0, 0, SRCCOPY);

        let mut result = Err("BitBlt failed".to_string());
        if blit.is_ok() {
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0, // BI_RGB
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
            let lines = GetDIBits(
                mem_dc,
                bmp,
                0,
                h as u32,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                &mut bmi,
                DIB_RGB_COLORS,
            );
            if lines > 0 {
                result = Ok(buf);
            } else {
                result = Err("GetDIBits failed".into());
            }
        }
        SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        result
    }
}
