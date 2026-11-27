"""Computer Use MCP server: GUI control of this Windows PC.

Runs in the user's interactive desktop session (a session-0 process
cannot see or drive the desktop). Serve it behind mcp-proxy as
Streamable HTTP bound to the PC's Tailscale address, so only devices
on your tailnet can reach it (see start-bridge.cmd in this folder).

Coordinate convention: screenshots are returned scaled to at most
MAX_WIDTH px wide. All mouse coordinates are given in that scaled
image's pixel space; the server converts them back to real pixels
using the scale of the most recent screenshot.

Operation overlay: while the agent is operating, a click-through,
always-on-top layered window draws a soft blue glow frame around
the whole screen plus a toast at the top centre ("muse is
operating - press ESC to cancel"). Pressing ESC cancels the
operation: action tools then refuse to run until operation_begin
is called again. Screenshots suspend the overlay briefly so
captures stay clean.
"""

import ctypes
import io
import json
import os
import tempfile
import threading
import time

# Per-monitor DPI awareness, so screenshot pixels (mss) and mouse
# coordinates (pyautogui) share one physical coordinate space even
# when Windows display scaling is not 100%.
try:
    ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        pass

import mss
import pyautogui
import win32api
import win32clipboard
import win32con
import win32gui
from fastmcp import FastMCP
from fastmcp.utilities.types import Image as MCPImage
from PIL import Image as PILImage
from PIL import ImageDraw, ImageFilter, ImageFont

pyautogui.FAILSAFE = True  # mouse slammed to a corner aborts with an error
pyautogui.PAUSE = 0.15

MAX_WIDTH = 1280

_state = {"scale": 1.0, "real_w": 0, "real_h": 0, "scaled_w": 0, "scaled_h": 0}

mcp = FastMCP("ComputerUse")


# ---------------------------------------------------------------------------
# Operation overlay: blue glow frame + "muse is operating" toast, ESC cancel
# ---------------------------------------------------------------------------

CANCELLED_MSG = (
    "CANCELLED: the user pressed ESC and stopped the operation. "
    "Do not retry any action. Report the cancellation to the user; "
    "call operation_begin only for a new, user-approved operation."
)

_op = {
    "active": False,          # an operation session is in progress
    "cancelled": False,       # ESC pressed during the operation
    "last_call": 0.0,         # time of the last tool call
    "flash_until": 0.0,       # show the "cancelled" toast until this time
    "suspend": 0,             # >0 while a screenshot is being captured
    "hwnd": 0,
    "visible": False,
    "shown_mode": None,       # "active" | "cancelled" | None
    "label": "",
}
_op_lock = threading.Lock()

IDLE_TIMEOUT_S = 600.0        # auto-end an operation after 10 idle minutes
VK_ESCAPE = 0x1B

_FONT_CANDIDATES = [
    r"C:\Windows\Fonts\YuGothB.ttc",
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\meiryob.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
]


def _load_font(size):
    for path in _FONT_CANDIDATES:
        try:
            return ImageFont.truetype(path, size)
        except Exception:
            continue
    return ImageFont.load_default()


def _render_overlay_bitmap(mode, w, h):
    """Full-screen RGBA image: soft blue glow frame + top-centre toast.

    mode is "active" or "cancelled". Also saved as a preview PNG so
    muse can show the user what the overlay looks like.
    """
    img = PILImage.new("RGBA", (w, h), (0, 0, 0, 0))

    # --- glow frame: stacked inset strokes, then blurred soft ---
    frame = PILImage.new("RGBA", (w, h), (0, 0, 0, 0))
    fd = ImageDraw.Draw(frame)
    band = 54
    if mode == "active":
        fr, fg, fb = 59, 130, 246
    else:
        fr, fg, fb = 248, 113, 113
    for i in range(band):
        t = i / band
        alpha = int(200 * (1.0 - t) ** 1.6) + 6
        fd.rectangle([i, i, w - 1 - i, h - 1 - i],
                     outline=(fr, fg, fb, alpha))
    frame = frame.filter(ImageFilter.GaussianBlur(14))
    img.alpha_composite(frame)
    d = ImageDraw.Draw(img)
    # crisp inner edge line on top of the blur
    d.rectangle([2, 2, w - 3, h - 3], outline=(fr, fg, fb, 190), width=3)

    # --- toast pill, top centre ---
    if mode == "active":
        title = "muse が操作しています"
        sub = "キャンセルしたいときは ESC キーを押してください"
        border = (96, 165, 250, 255)
        dot = (96, 165, 250, 255)
    else:
        title = "キャンセルしました"
        sub = "muse の操作を停止しました"
        border = (248, 113, 113, 255)
        dot = (248, 113, 113, 255)
    f_title = _load_font(34)
    f_sub = _load_font(23)

    def tw(draw, text, font):
        box = draw.textbbox((0, 0), text, font=font)
        return box[2] - box[0]

    text_w = max(tw(d, title, f_title), tw(d, sub, f_sub))
    pill_w = text_w + 46 + 58          # dot + gaps + padding
    pill_h = 96
    x0 = (w - pill_w) // 2
    y0 = 26
    x1, y1 = x0 + pill_w, y0 + pill_h
    d.rounded_rectangle([x0, y0, x1, y1], radius=20,
                        fill=(8, 14, 32, 218), outline=border, width=2)
    cy = y0 + pill_h // 2
    d.ellipse([x0 + 24, cy - 10, x0 + 44, cy + 10], fill=dot)
    tx = x0 + 58
    d.text((tx, y0 + 14), title, font=f_title, fill=(255, 255, 255, 255))
    d.text((tx, y0 + 56), sub, font=f_sub, fill=(203, 213, 225, 255))

    try:
        img.save(os.path.join(tempfile.gettempdir(), f"cu-overlay-preview-{mode}.png"))
    except Exception:
        pass
    return img


class _BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int32),
        ("biHeight", ctypes.c_int32), ("biPlanes", ctypes.c_uint16),
        ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
        ("biSizeImage", ctypes.c_uint32), ("biXPelsPerMeter", ctypes.c_int32),
        ("biYPelsPerMeter", ctypes.c_int32), ("biClrUsed", ctypes.c_uint32),
        ("biClrImportant", ctypes.c_uint32),
    ]


class _BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", _BITMAPINFOHEADER),
                ("bmiColors", ctypes.c_uint32 * 3)]


class _POINT(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int32), ("y", ctypes.c_int32)]


class _SIZE(ctypes.Structure):
    _fields_ = [("cx", ctypes.c_int32), ("cy", ctypes.c_int32)]


class _BLENDFUNCTION(ctypes.Structure):
    _fields_ = [("BlendOp", ctypes.c_uint8), ("BlendFlags", ctypes.c_uint8),
                ("SourceConstantAlpha", ctypes.c_uint8),
                ("AlphaFormat", ctypes.c_uint8)]


def _present_overlay(hwnd, img):
    """Push a PIL RGBA image into the layered window (per-pixel alpha)."""
    w, h = img.size
    # PIL gives RGBA byte order; DIB sections want BGRA.
    r, g, b, a = img.split()
    bgra = PILImage.merge("RGBA", (b, g, r, a)).tobytes()

    user32 = ctypes.windll.user32
    gdi32 = ctypes.windll.gdi32
    bmi = _BITMAPINFO()
    bmi.bmiHeader.biSize = ctypes.sizeof(_BITMAPINFOHEADER)
    bmi.bmiHeader.biWidth = w
    bmi.bmiHeader.biHeight = -h          # top-down
    bmi.bmiHeader.biPlanes = 1
    bmi.bmiHeader.biBitCount = 32
    bmi.bmiHeader.biCompression = 0      # BI_RGB
    bits = ctypes.c_void_p()
    screen_dc = user32.GetDC(0)
    mem_dc = gdi32.CreateCompatibleDC(screen_dc)
    hbmp = gdi32.CreateDIBSection(screen_dc, ctypes.byref(bmi), 0,
                                  ctypes.byref(bits), None, 0)
    if not hbmp:
        return False
    ctypes.memmove(bits, bgra, len(bgra))
    old = gdi32.SelectObject(mem_dc, hbmp)
    blend = _BLENDFUNCTION(0, 0, 255, 1)  # AC_SRC_OVER, AC_SRC_ALPHA
    ok = user32.UpdateLayeredWindow(
        hwnd, 0, ctypes.byref(_POINT(0, 0)), ctypes.byref(_SIZE(w, h)),
        mem_dc, ctypes.byref(_POINT(0, 0)), 0, ctypes.byref(blend), 2,
    )  # ULW_ALPHA = 2
    gdi32.SelectObject(mem_dc, old)
    gdi32.DeleteObject(hbmp)
    gdi32.DeleteDC(mem_dc)
    user32.ReleaseDC(0, screen_dc)
    return bool(ok)


def _overlay_wndproc(hwnd, msg, wparam, lparam):
    return win32gui.DefWindowProc(hwnd, msg, wparam, lparam)


def _overlay_thread():
    user32 = ctypes.windll.user32
    w = user32.GetSystemMetrics(0)
    h = user32.GetSystemMetrics(1)
    if w <= 0 or h <= 0:
        return
    try:
        wc = win32gui.WNDCLASS()
        wc.lpszClassName = "MuseComputerUseOverlay"
        wc.lpfnWndProc = _overlay_wndproc
        wc.hInstance = win32api.GetModuleHandle(None)
        win32gui.RegisterClass(wc)
    except Exception:
        pass  # already registered (e.g. thread restarted)
    exstyle = (win32con.WS_EX_LAYERED | win32con.WS_EX_TRANSPARENT
               | win32con.WS_EX_TOPMOST | win32con.WS_EX_NOACTIVATE
               | win32con.WS_EX_TOOLWINDOW)
    hwnd = win32gui.CreateWindowEx(
        exstyle, "MuseComputerUseOverlay", "", win32con.WS_POPUP,
        0, 0, w, h, 0, 0, win32api.GetModuleHandle(None), None,
    )
    win32gui.SetWindowPos(hwnd, win32con.HWND_TOPMOST, 0, 0, w, h,
                          win32con.SWP_NOACTIVATE)
    with _op_lock:
        _op["hwnd"] = hwnd
    bitmaps = {}
    esc_was_down = False
    SW_SHOWNOACTIVATE = 4
    while True:
        try:
            win32gui.PumpWaitingMessages()
        except Exception:
            pass
        now = time.time()

        # --- ESC cancel (only while an operation is running) ---
        # 0x8000 = key currently down; 0x0001 = pressed since this
        # thread's last call (a latch, so even a very short tap is
        # never missed between polls).
        esc_state = user32.GetAsyncKeyState(VK_ESCAPE)
        esc_down = bool(esc_state & 0x8000)
        esc_hit = esc_down or bool(esc_state & 0x0001)
        with _op_lock:
            if esc_hit and not esc_was_down and _op["active"] \
                    and not _op["cancelled"]:
                _op["cancelled"] = True
                _op["flash_until"] = now + 1.8
        esc_was_down = esc_down or esc_hit

        # --- idle safety: never leave the frame stuck on ---
        with _op_lock:
            if _op["active"] and not _op["cancelled"] \
                    and now - _op["last_call"] > IDLE_TIMEOUT_S:
                _op["active"] = False

        with _op_lock:
            if now < _op["flash_until"]:
                want = "cancelled"
            elif _op["active"] and not _op["cancelled"]:
                want = "active"
            else:
                want = None
            if _op["suspend"] > 0:
                want = None
            visible = _op["visible"]
            shown = _op["shown_mode"]

        if want is None:
            if visible:
                win32gui.ShowWindow(hwnd, win32con.SW_HIDE)
                with _op_lock:
                    _op["visible"] = False
                    _op["shown_mode"] = None
        else:
            if (not visible) or shown != want:
                if want not in bitmaps:
                    bitmaps[want] = _render_overlay_bitmap(want, w, h)
                if _present_overlay(hwnd, bitmaps[want]):
                    win32gui.ShowWindow(hwnd, SW_SHOWNOACTIVATE)
                    with _op_lock:
                        _op["visible"] = True
                        _op["shown_mode"] = want
        time.sleep(0.05)


def _op_touch():
    """Mark activity; auto-begin an operation on any tool call
    (unless the user has cancelled — only operation_begin re-arms)."""
    with _op_lock:
        _op["last_call"] = time.time()
        if not _op["cancelled"]:
            _op["active"] = True


def _op_gate():
    """Return the cancellation message if the user has cancelled."""
    _op_touch()
    with _op_lock:
        if _op["cancelled"]:
            return CANCELLED_MSG
    return None


@mcp.tool()
def operation_begin(label: str = "") -> str:
    """Begin an operation session: clears any ESC cancellation and
    shows the on-screen overlay (blue glow frame + toast) so the user
    can see that muse is operating. Call before a run of actions."""
    with _op_lock:
        _op["cancelled"] = False
        _op["active"] = True
        _op["flash_until"] = 0.0
        _op["last_call"] = time.time()
        _op["label"] = label
    return "operation begun; overlay shown; ESC cancels"


@mcp.tool()
def operation_end() -> str:
    """End the operation session and hide the overlay."""
    with _op_lock:
        _op["active"] = False
        was_cancelled = _op["cancelled"]
    return "operation ended (was cancelled)" if was_cancelled \
        else "operation ended; overlay hidden"


@mcp.tool()
def operation_status() -> str:
    """Operation/overlay state: active, cancelled, overlay visible."""
    with _op_lock:
        return json.dumps({
            "active": _op["active"],
            "cancelled": _op["cancelled"],
            "overlay_visible": _op["visible"],
            "shown_mode": _op["shown_mode"],
            "hwnd": _op["hwnd"],
            "label": _op["label"],
            "seconds_since_last_call":
                round(time.time() - _op["last_call"], 1)
                if _op["last_call"] else None,
        })


threading.Thread(target=_overlay_thread, daemon=True,
                 name="cu-overlay").start()


# ---------------------------------------------------------------------------
# Screen capture and input tools
# ---------------------------------------------------------------------------

def _grab_scaled_png():
    with mss.mss() as sct:
        shot = sct.grab(sct.monitors[1])  # primary monitor
    img = PILImage.frombytes("RGB", shot.size, shot.rgb)
    real_w, real_h = img.size
    scale = 1.0
    if real_w > MAX_WIDTH:
        scale = MAX_WIDTH / real_w
        img = img.resize(
            (MAX_WIDTH, round(real_h * scale)), PILImage.LANCZOS
        )
    _state.update(
        scale=scale,
        real_w=real_w,
        real_h=real_h,
        scaled_w=img.size[0],
        scaled_h=img.size[1],
    )
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def _grab_clean_png():
    """Grab a screenshot with the operation overlay briefly hidden,
    so captures never contain the frame/toast."""
    with _op_lock:
        _op["suspend"] += 1
    try:
        time.sleep(0.18)  # let the overlay thread hide the window
        return _grab_scaled_png()
    finally:
        with _op_lock:
            _op["suspend"] -= 1


def _to_real(x, y):
    s = _state["scale"] or 1.0
    return round(x / s), round(y / s)


@mcp.tool()
def screenshot():
    """Capture the primary display.

    Returns a PNG (at most 1280 px wide) plus a line of text with the
    real and scaled sizes. Give mouse coordinates in the returned
    image's pixel space.
    """
    _op_touch()
    data = _grab_clean_png()
    info = (
        f"real={_state['real_w']}x{_state['real_h']} "
        f"scaled={_state['scaled_w']}x{_state['scaled_h']} "
        f"scale={_state['scale']:.4f}"
    )
    return [MCPImage(data=data, format="png"), info]


@mcp.tool()
def screen_info() -> str:
    """Real display size, current screenshot scale, cursor position."""
    _op_touch()
    if not _state["real_w"]:
        _grab_clean_png()
    cx, cy = pyautogui.position()
    return json.dumps(
        {
            "real_width": _state["real_w"],
            "real_height": _state["real_h"],
            "scaled_width": _state["scaled_w"],
            "scaled_height": _state["scaled_h"],
            "scale": _state["scale"],
            "cursor_real": [cx, cy],
        }
    )


@mcp.tool()
def mouse_move(x: int, y: int) -> str:
    """Move the cursor to (x, y) in scaled screenshot coordinates."""
    cancelled = _op_gate()
    if cancelled:
        return cancelled
    rx, ry = _to_real(x, y)
    pyautogui.moveTo(rx, ry)
    return f"cursor at real ({rx}, {ry})"


@mcp.tool()
def click(x: int, y: int, button: str = "left", clicks: int = 1) -> str:
    """Click at (x, y) in scaled screenshot coordinates."""
    cancelled = _op_gate()
    if cancelled:
        return cancelled
    rx, ry = _to_real(x, y)
    pyautogui.click(rx, ry, clicks=clicks, button=button)
    return f"clicked {button} x{clicks} at real ({rx}, {ry})"


@mcp.tool()
def drag(x1: int, y1: int, x2: int, y2: int, duration: float = 0.5) -> str:
    """Drag from (x1, y1) to (x2, y2), scaled screenshot coordinates."""
    cancelled = _op_gate()
    if cancelled:
        return cancelled
    rx1, ry1 = _to_real(x1, y1)
    rx2, ry2 = _to_real(x2, y2)
    pyautogui.moveTo(rx1, ry1)
    pyautogui.dragTo(rx2, ry2, duration=duration, button="left")
    return f"dragged real ({rx1}, {ry1}) -> ({rx2}, {ry2})"


@mcp.tool()
def scroll(amount: int, x: int = -1, y: int = -1) -> str:
    """Scroll the wheel. Positive amount scrolls up, negative down.
    Optionally move to (x, y) first (scaled coordinates)."""
    cancelled = _op_gate()
    if cancelled:
        return cancelled
    if x >= 0 and y >= 0:
        rx, ry = _to_real(x, y)
        pyautogui.moveTo(rx, ry)
    pyautogui.scroll(amount)
    return f"scrolled {amount}"


@mcp.tool()
def key_press(keys: str) -> str:
    """Press a key or shortcut, e.g. 'enter', 'ctrl+c', 'alt+tab',
    'win+r', 'ctrl+shift+esc'. Key names follow pyautogui conventions.
    Note: pressing ESC also triggers the user's cancel, by design."""
    cancelled = _op_gate()
    if cancelled:
        return cancelled
    parts = [p.strip().lower() for p in keys.split("+") if p.strip()]
    if not parts:
        return "no keys given"
    if len(parts) == 1:
        pyautogui.press(parts[0])
    else:
        pyautogui.hotkey(*parts)
    return f"pressed {'+'.join(parts)}"


@mcp.tool()
def type_text(text: str) -> str:
    """Type text at the current focus. Pasted via the clipboard
    (Ctrl+V), so Japanese and other Unicode text works. Note: this
    replaces the clipboard contents."""
    cancelled = _op_gate()
    if cancelled:
        return cancelled
    win32clipboard.OpenClipboard()
    try:
        win32clipboard.EmptyClipboard()
        win32clipboard.SetClipboardText(text, win32con.CF_UNICODETEXT)
    finally:
        win32clipboard.CloseClipboard()
    pyautogui.hotkey("ctrl", "v")
    return f"pasted {len(text)} chars"


@mcp.tool()
def list_windows() -> str:
    """List visible windows with non-empty titles (hwnd + title)."""
    _op_touch()
    out = []

    def cb(hwnd, _):
        if win32gui.IsWindowVisible(hwnd):
            title = win32gui.GetWindowText(hwnd)
            if title:
                out.append({"hwnd": hwnd, "title": title})

    win32gui.EnumWindows(cb, None)
    return json.dumps(out, ensure_ascii=False)


@mcp.tool()
def focus_window(title: str) -> str:
    """Bring the first visible window whose title contains `title`
    (case-insensitive) to the foreground."""
    cancelled = _op_gate()
    if cancelled:
        return cancelled
    needle = title.lower()
    found = []

    def cb(hwnd, _):
        if win32gui.IsWindowVisible(hwnd):
            t = win32gui.GetWindowText(hwnd)
            if t and needle in t.lower():
                found.append((hwnd, t))

    win32gui.EnumWindows(cb, None)
    if not found:
        return f"no window matching '{title}'"
    hwnd, t = found[0]
    if win32gui.IsIconic(hwnd):
        win32gui.ShowWindow(hwnd, win32con.SW_RESTORE)
    win32gui.SetForegroundWindow(hwnd)
    return f"focused '{t}' (hwnd {hwnd})"


if __name__ == "__main__":
    mcp.run()
