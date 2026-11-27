# winmuse

**winmuse** lets an AI agent (an MCP client) on **another machine** see
and operate a real Windows desktop — screenshots, mouse, keyboard,
window management — reached **only over Tailscale**, while the person
at the PC always knows it is happening and can stop it instantly.

Design principles:

- **Visible.** While the agent operates, a glowing frame and a toast
  ("muse is operating — press ESC to cancel") are shown on screen.
  Screenshots taken by the agent never contain the overlay, so the
  agent sees the clean desktop.
- **Stoppable.** Pressing **ESC** cancels the operation immediately.
  Action tools then refuse with `CANCELLED:` until the agent starts a
  new, user-approved operation.
- **Permissioned.** A profile system decides which apps the agent may
  touch. Denials return `REFUSED: <reason>` — distinct from cancels.
  There is no silent default: with no configuration, winmuse starts
  in the read-only `observe` profile and says so.
- **Local.** One tray-resident Rust process with a native MCP server
  (no proxy). By default it binds only to the machine's Tailscale
  address (100.64.0.0/10), never to the LAN. An optional bearer token
  adds a second factor.

The original Python implementation is kept under `server/` as the
reference the Rust version was ported from; a dependency-light Python
client (`client/`) and an optional screenshot judge (`judge/`) are
included.

## Architecture

```
 agent machine (any OS, on your tailnet)          Windows PC
 ┌───────────────────────────────┐                 ┌──────────────────────────────┐
 │ client/client.py              │  Streamable     │ winmuse.exe (tray-resident)  │
 │  - or any MCP client          │  HTTP / MCP     │  native MCP server (:18081,  │
 │                               │ ───────────────▶│  bound to the tailnet IP)    │
 │ judge/judge.py (optional)     │                 │  overlay + ESC/F9 watch      │
 │  - Jev screenshot gate        │                 │  permission profiles, audit  │
 │  - calls api.typesafe.ai      │                 │  runs in the interactive     │
 └───────────────────────────────┘                 │  desktop session (logon task)│
                                                   └──────────────────────────────┘
```

## Quick start

```powershell
cd winmuse
cargo build --release
.\target\release\winmuse.exe
```

On first run (no config yet) the Settings window opens automatically
and the server starts in `observe`. Pick a profile, tick the apps the
agent may control, and save — the resident process picks the new
config up within seconds, no restart needed.

To start winmuse automatically at logon:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install-task.ps1
```

The task matters: winmuse must run in your **interactive session**.
A session-0 service cannot see or drive the desktop.

From the agent machine (Python 3, standard library only):

```bash
python client/client.py --host <tailscale-ip-of-the-pc> list-tools
python client/client.py --host <tailscale-ip-of-the-pc> screenshot --out shot.png
# if a token is set in the config:
python client/client.py --host <tailscale-ip-of-the-pc> --token <token> list-tools
```

Or point any MCP client at `http://<tailscale-ip>:18081/mcp`
(Streamable HTTP, protocol version 2025-03-26).

## Tools (13)

| Tool | Kind | Notes |
|---|---|---|
| `screenshot` | read | PNG, scaled to ≤1280 px wide; sets the coordinate scale |
| `screen_info` | read | real size, scaled size, scale, cursor position |
| `list_windows` | read | visible windows with titles and geometry |
| `operation_begin` | op | starts an operation (shows the overlay) |
| `operation_end` | op | ends the operation (hides the overlay) |
| `operation_status` | op | state, profile, allowlist, overlay visibility |
| `mouse_move` | action | scaled coordinates |
| `click` | action | left/right/middle, multi-click |
| `drag` | action | interpolated path |
| `scroll` | action | at a point, or at the cursor |
| `key_press` | action | `+`-separated combos, e.g. `ctrl+shift+esc` |
| `type_text` | action | pastes via the clipboard (Unicode-safe) |
| `focus_window` | action | raise a window by title substring |

**Coordinate convention:** screenshots are scaled to at most 1280 px
wide. All mouse coordinates are given in that scaled image's pixel
space; the server converts them back to real pixels using the scale
of the most recent screenshot. The server is DPI-aware, so this
holds under Windows display scaling ≠ 100%.

Any tool call implicitly begins an operation (the overlay appears).
An idle operation ends itself after 10 minutes.

## Permission profiles

Set in `%LOCALAPPDATA%\winmuse\config.toml` or via the Settings GUI.
Example: [`server/config.example.toml`](server/config.example.toml).

| Profile | Behavior |
|---|---|
| `observe` | Read-only tools work. Every action tool returns `REFUSED`. |
| `guided` | Pointer actions are allowed only when the window under the target point belongs to an allowlisted process. Keyboard actions are checked against the foreground window's process. `focus_window` refuses non-allowlisted targets. |
| `full` | Everything is allowed (the classic behavior). |

Options:

- `block_win_keys` (default `true`) — in `guided`, key presses
  containing the Windows key are refused: the Start menu / shell
  belongs to no allowlisted process.
- `require_local_arm` (default `false`) — `operation_begin` shows a
  "waiting for your OK" overlay; the person at the PC presses **F9**
  to arm the operation (ESC denies it). Until armed, actions return
  `REFUSED`.
- **Audit log** — every tool call is appended as one JSON line to
  `%LOCALAPPDATA%\winmuse\audit.jsonl` with the profile and the
  decision (`ok` / `refused` / `cancelled`). `type_text` is logged as
  a character count plus a short SHA-256 prefix, never as raw text.
  Screenshots are not stored.

## Tray icon and Settings

The tray icon mirrors the state: gray = serving stopped, green =
idle, blue = operating, amber = waiting for local arm, red =
cancelled. Its menu offers Settings, Start/Stop serving, the audit
log, the config folder, and Quit. Double-clicking the icon — or
launching `winmuse.exe` again while it is resident — opens Settings.

`winmuse settings` opens the Settings GUI directly. It edits the
profile, the allowlist (a checklist of *currently running apps* — no
typing executable names), the safety toggles, the audit log, the
optional bearer token (with a generator), and the overlay (on/off,
language, custom toast text). The GUI and the resident process are
the same executable; they communicate through the config file
(hot-reloaded) and a `status.json` heartbeat in
`%LOCALAPPDATA%\winmuse\`.

## The Jev gate (optional)

`judge/judge.py` sends a screenshot to TypeSafe AI's Jev — its
*System One* judgment model, not a generative LLM — with two
questions: *is the target visible?* (`noul`, a calibrated
probability) and *where is it?* (`box`, a region + confidence), and
prints a normalized JSON verdict `{present, box, center}`.

```
set TYPESAFE_API_KEY=<your key from console.typesafe.ai>
python client/client.py screenshot --judge "the pink New Chat button"
```

The key is read from the `TYPESAFE_API_KEY` environment variable or,
with `--key-stdin`, from one line of stdin. It is never written to
disk by this code.

Treat Jev as a **gate** ("worth looking here"), not as click
coordinates: measured box centers can be off by ~13 px. The judge is
a replaceable component — anything that emits the same JSON shape
can stand in for it.

> **Privacy:** judging sends the screenshot to `api.typesafe.ai`.
> Do not judge screenshots showing passwords or private content you
> have not agreed to send to a third party.

## ⚠ Security

- **Anything that can reach the endpoint can operate the PC** —
  mouse, keyboard, clipboard — within the active profile. That is
  the point of the tool, and why the default bind is the tailnet
  address only. Never bind `0.0.0.0`, never port-forward it.
- `full` profile + no token + a non-tailnet bind is effectively
  remote desktop control for the network. Don't.
- Run it only on a tailnet you trust. Stop the `Winmuse` scheduled
  task (or Quit from the tray) to revoke access.
- The overlay is a notification, not a security boundary; the
  profile checks and the ESC cancel are the enforcement.

## Limitations

- Cannot cross the Windows **lock screen** or **UAC** secure desktop.
- Elevated (administrator) windows do not accept input from the
  non-elevated bridge (Windows UIPI).
- Primary monitor only.
- Your own mouse and keyboard compete with the agent while it drives.
- `type_text` pastes through the clipboard and **replaces its contents**.
- `focus_window` can occasionally fail with a Windows foreground-lock
  error; clicking a harmless visible area of the target window raises
  it just as well.

## Repository layout

```
winmuse/   the Rust server (tray-resident, MCP over Streamable HTTP)
client/    dependency-light Python client (stdlib only)
judge/     optional screenshot judge
server/    original Python implementation (reference) + config example
scripts/   logon-task install / uninstall / stop (PowerShell)
```

## License

MIT — see [LICENSE](LICENSE).
