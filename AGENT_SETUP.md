# Installing winmuse — a guide for the AI agent

The user asked you to install winmuse. Do the work yourself; the
user should only have to answer a few questions. Do not hand them a
list of manual steps.

## 0. Check the environment (on the Windows PC)

Run these and fix what is missing before continuing:

```powershell
cargo --version          # Rust present? If not, install via https://rustup.rs/
tailscale status         # Tailscale running and signed in?
tailscale ip -4          # note the PC's tailnet address
```

winmuse must end up running in the PC's **interactive desktop
session**. If you are installing through a remote shell (SSH lands
in session 0, where the desktop is invisible), never launch
`winmuse.exe` from that shell — use the logon task in step 4 to
start it instead.

## 1. Build

```powershell
cd winmuse
cargo build --release    # first build takes several minutes
```

## 2. Interview the user (ask; do not decide silently)

winmuse has no silent default profile on purpose. Ask these
questions in plain language, in one short message:

1. **How much may the agent do?**
   - *Full access* — operate any app (profile `full`). Right for a
     personal PC the user controls.
   - *Only chosen apps* — input lands only in apps the user picks
     (profile `guided`). The safer default.
   - *Look only* — screenshots and window lists, no input at all
     (profile `observe`).
2. **If guided:** which apps? List what is currently running —
   `Get-Process | Where-Object MainWindowTitle | Select-Object
   ProcessName, MainWindowTitle` — and let the user pick from real
   names. Warn once if they include a terminal, Explorer, or a
   password manager: input there can reach local files.
3. **Access token?** If the tailnet is shared with other people,
   recommend yes; on a single-user tailnet, optional. If yes,
   generate one (step 3) — never ask the user to invent one.
4. **Start automatically at logon?** (installs a scheduled task)
5. **Use the Jev screenshot judge?** An optional client-side
   helper that pre-screens a screenshot with an external judgment
   model (TypeSafe AI "System One") — "is this target on screen?"
   — before the agent acts. It is a gate, never a click
   coordinate. Two things the user must understand before saying
   yes: judging **sends the screenshot to api.typesafe.ai**, and
   it needs a **TypeSafe API key** (from console.typesafe.ai).
   Default is no; it can be enabled later at any time.

If a config already exists at `%LOCALAPPDATA%\winmuse\config.toml`,
do not overwrite it silently: tell the user the current profile and
ask whether to change anything.

## 3. Write the config

Create `%LOCALAPPDATA%\winmuse\` if needed and write `config.toml`
with the answers (omit blocks that do not apply):

```toml
profile = "guided"            # full | guided | observe
# auth_token = "<generated>"  # only if the user said yes

[guided]
allow_processes = ["chrome.exe"]
block_win_keys = true

[consent]
require_local_arm = false

[audit]
enabled = true

[overlay]
enabled = true
```

Token generation (PowerShell):

```powershell
$b = New-Object byte[] 16
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b)
[BitConverter]::ToString($b).Replace('-', '').ToLower()
```

### If the user chose Jev

Jev is configured on the **agent machine**, not on the PC — there
is no server setting for it.

- The judge runs as `judge/judge.py`, or through the client:
  `client.py screenshot --judge "<target>"`.
- It reads its API key from the `TYPESAFE_API_KEY` environment
  variable on the agent machine, or per call via `--key-stdin`.
  Have the user supply the key through whatever secret mechanism
  their agent platform provides and set the environment variable
  there. **Never write the key to a file** — not into
  `config.toml`, not into this repository, not into a script.
- If the user has no TypeSafe key yet, point them at
  console.typesafe.ai and leave Jev off until they have one;
  winmuse works fully without it.

## 4. Start it

Preferred (also covers remote-shell installs):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\install-task.ps1
Start-ScheduledTask -TaskName Winmuse
```

If the user declined autostart and you are at the PC's own desktop,
just launch `winmuse\target\release\winmuse.exe`.

## 5. Verify, then report

1. On the PC, read `%LOCALAPPDATA%\winmuse\status.json`: `serving`
   is true and `addr` is the tailnet address (not `127.0.0.1` —
   if it is, Tailscale was down at start; restart winmuse).
2. From the agent machine:
   `python client/client.py --host <addr> list-tools` (add
   `--token <token>` if one was set) — 13 tools should answer.
3. Take a screenshot through the client and **show it to the
   user** as proof, then `operation_end`.
4. Report in a few lines: the endpoint address, the profile you
   set, whether a token is in use, and how to change things later
   (run `winmuse settings`, or edit the config — it hot-reloads)
   and how to stop it (`scripts\stop-winmuse.ps1`, or
   `scripts\uninstall-task.ps1` to remove the logon task).
