@echo off
rem =====================================================================
rem  Computer Use bridge launcher (Windows)
rem
rem  Serves server.py as Streamable HTTP via mcp-proxy.
rem
rem  CONFIGURATION: set HOST to this PC's Tailscale IPv4 address
rem  (run `tailscale ip -4` to find it). NEVER bind 0.0.0.0 or a
rem  LAN/public address: anything that can reach this port can
rem  control this PC with the mouse and keyboard.
rem =====================================================================
set HOST=100.x.x.x
set PORT=18081

set HERE=%~dp0
set LOG=%HERE%bridge.log
echo START %DATE% %TIME% > %LOG%
mcp-proxy --host %HOST% --port %PORT% --log-level INFO -- uv run --python 3.13 --with fastmcp --with pyautogui --with mss --with pywin32 --with pillow python "%HERE%server.py" >> %LOG% 2>&1
echo BRIDGE_EXIT=%ERRORLEVEL% >> %LOG%
