#!/usr/bin/env python3
"""Minimal MCP client for the Computer Use bridge.

Connects directly to the bridge's Streamable HTTP endpoint
(http://<PC tailnet IP>:18081/mcp) from any machine on the same
tailnet. No credentials: the bridge binds only to the PC's tailnet
address, so tailnet membership is the access control.

Configuration (first match wins):
  --host / --port command-line options
  CU_BRIDGE_HOST / CU_BRIDGE_PORT environment variables
  defaults: 127.0.0.1 / 18081 (set CU_BRIDGE_HOST to your PC's
  `tailscale ip -4` address for real use)

Usage:
  client.py list-tools
  client.py screenshot [--out <png-path>] [--judge "<target>"]
  client.py call-tool --name <tool> --arguments '<json>'
  client.py call-tool --name <tool> --arguments-file <path-to-json>

Mouse coordinates are in the scaled screenshot's pixel space
(max width 1280); the server converts them to real pixels.

--judge additionally asks Jev (TypeSafe AI's System One judgment
model, see ../judge/judge.py) whether a target is visible in the
screenshot and roughly where. It needs a TypeSafe API key via the
TYPESAFE_API_KEY environment variable or --judge-key-stdin.
"""

import argparse
import base64
import json
import os
import socket
import sys

MCP_PATH = "/mcp"
PROTOCOL_VERSION = "2025-03-26"
CLIENT_INFO = {"name": "computer-use-client", "version": "1.0.0"}


class HttpConnection:
    """Minimal HTTP/1.1 client over a plain TCP socket."""

    def __init__(self, host, port):
        self.host = host
        self.port = port
        self.sock = socket.create_connection((host, port), timeout=20)
        self.buf = b""

    def _read_until(self, marker):
        while marker not in self.buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise RuntimeError("connection closed while reading response")
            self.buf += chunk
        idx = self.buf.index(marker) + len(marker)
        head, self.buf = self.buf[:idx], self.buf[idx:]
        return head

    def _read_exact(self, n):
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise RuntimeError("connection closed while reading body")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def _read_line(self):
        return self._read_until(b"\r\n")[:-2]

    def request(self, method, path, body=None, headers=None, timeout=240):
        self.sock.settimeout(timeout)
        hdrs = {
            "Host": f"{self.host}:{self.port}",
            "Accept": "application/json, text/event-stream",
            "Connection": "keep-alive",
        }
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            hdrs["Content-Type"] = "application/json"
            hdrs["Content-Length"] = str(len(data))
        if headers:
            hdrs.update(headers)
        lines = [f"{method} {path} HTTP/1.1"] + [f"{k}: {v}" for k, v in hdrs.items()]
        self.sock.sendall(("\r\n".join(lines) + "\r\n\r\n").encode() + (data or b""))

        head = self._read_until(b"\r\n\r\n").decode("iso-8859-1")
        status = int(head.split(" ", 2)[1])
        resp_headers = {}
        for line in head.split("\r\n")[1:]:
            if ": " in line:
                k, v = line.split(": ", 1)
                resp_headers[k.lower()] = v
        if resp_headers.get("transfer-encoding", "").lower() == "chunked":
            body_bytes = b""
            while True:
                size = int(self._read_line().strip(), 16)
                if size == 0:
                    self._read_line()  # trailing CRLF / trailers end
                    break
                body_bytes += self._read_exact(size)
                self._read_exact(2)
        elif "content-length" in resp_headers:
            body_bytes = self._read_exact(int(resp_headers["content-length"]))
        else:
            body_bytes = b""
        return status, resp_headers, body_bytes


def parse_rpc_payloads(body_bytes):
    """Return JSON-RPC messages from a JSON or SSE response body."""
    text = body_bytes.decode("utf-8", "replace").strip()
    if not text:
        return []
    if text.startswith("{"):
        return [json.loads(text)]
    messages = []
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("data:"):
            payload = line[5:].strip()
            if payload:
                messages.append(json.loads(payload))
    return messages


class McpClient:
    def __init__(self, host, port, token=None):
        self.http = HttpConnection(host, port)
        self.token = token
        self.session_id = None
        self._next_id = 1

    def _rpc(self, method, params=None, is_notification=False, timeout=240):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        rpc_id = None
        if not is_notification:
            rpc_id = self._next_id
            self._next_id += 1
            msg["id"] = rpc_id
        headers = {}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        if self.session_id:
            headers["Mcp-Session-Id"] = self.session_id
        status, resp_headers, body = self.http.request(
            "POST", MCP_PATH, body=msg, headers=headers, timeout=timeout
        )
        if "mcp-session-id" in resp_headers:
            self.session_id = resp_headers["mcp-session-id"]
        if is_notification:
            return None
        if status >= 400:
            raise RuntimeError(f"HTTP {status}: {body[:500]!r}")
        for payload in parse_rpc_payloads(body):
            if payload.get("id") == rpc_id:
                if "error" in payload:
                    raise RuntimeError(f"JSON-RPC error: {json.dumps(payload['error'])}")
                return payload.get("result")
        raise RuntimeError(f"no response for id {rpc_id}: {body[:500]!r}")

    def initialize(self):
        result = self._rpc(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": CLIENT_INFO,
            },
        )
        self._rpc("notifications/initialized", is_notification=True)
        return result

    def list_tools(self):
        tools = []
        cursor = None
        while True:
            params = {}
            if cursor:
                params["cursor"] = cursor
            result = self._rpc("tools/list", params)
            tools.extend(result.get("tools", []))
            cursor = result.get("nextCursor")
            if not cursor:
                return tools

    def call_tool(self, name, arguments, timeout=240):
        return self._rpc(
            "tools/call", {"name": name, "arguments": arguments}, timeout=timeout
        )


def find_judge():
    """Locate judge/judge.py: $JUDGE_PATH, else ../judge/judge.py
    relative to this file (the repository layout)."""
    env = os.environ.get("JUDGE_PATH")
    if env:
        return env
    candidate = os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..", "judge", "judge.py"
    )
    return candidate if os.path.exists(candidate) else None


def main():
    parser = argparse.ArgumentParser(description="Computer Use MCP bridge client")
    parser.add_argument("--host", default=os.environ.get("CU_BRIDGE_HOST", "127.0.0.1"))
    parser.add_argument(
        "--port", type=int, default=int(os.environ.get("CU_BRIDGE_PORT", "18081"))
    )
    parser.add_argument(
        "--token",
        default=os.environ.get("CU_BRIDGE_TOKEN"),
        help="bearer token, when the server has auth_token set (or env CU_BRIDGE_TOKEN)",
    )
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("list-tools")
    shot = sub.add_parser("screenshot")
    shot.add_argument("--out")
    shot.add_argument("--timeout", type=int, default=240)
    shot.add_argument(
        "--judge",
        metavar="TARGET",
        help="also ask Jev (TypeSafe System One) whether TARGET is visible and where it is",
    )
    shot.add_argument(
        "--judge-key-stdin",
        action="store_true",
        help="read the TypeSafe API key from stdin and forward it to judge.py (transient, no storage)",
    )
    call = sub.add_parser("call-tool")
    call.add_argument("--name", required=True)
    call.add_argument("--arguments-file")
    call.add_argument("--arguments")
    call.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()

    client = McpClient(args.host, args.port, token=args.token)
    init = client.initialize()
    if args.command == "list-tools":
        tools = client.list_tools()
        print(
            json.dumps(
                {
                    "server": init.get("serverInfo"),
                    "protocolVersion": init.get("protocolVersion"),
                    "tools": tools,
                },
                ensure_ascii=False,
                indent=2,
            )
        )
    elif args.command == "call-tool":
        if args.arguments_file:
            with open(args.arguments_file, encoding="utf-8") as f:
                arguments = json.load(f)
        elif args.arguments:
            arguments = json.loads(args.arguments)
        else:
            arguments = {}
        result = client.call_tool(args.name, arguments, timeout=args.timeout)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        if isinstance(result, dict) and result.get("isError"):
            sys.exit(1)
    elif args.command == "screenshot":
        import datetime

        result = client.call_tool("screenshot", {}, timeout=args.timeout)
        image_data = None
        info_text = None
        for block in result.get("content", []):
            if block.get("type") == "image":
                image_data = block.get("data")
            elif block.get("type") == "text":
                info_text = block.get("text")
        if not image_data:
            print(json.dumps({"error": "no image in result", "raw": result}))
            sys.exit(1)
        if args.out:
            out_path = args.out
        else:
            stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
            out_dir = os.path.join(os.getcwd(), "shots")
            os.makedirs(out_dir, exist_ok=True)
            out_path = os.path.join(out_dir, f"shot-{stamp}.png")
        with open(out_path, "wb") as f:
            f.write(base64.b64decode(image_data))
        out = {"saved": out_path, "info": info_text}
        if args.judge:
            import subprocess

            judge_py = find_judge()
            if not judge_py:
                out["judge"] = {
                    "error": "judge.py not found; set JUDGE_PATH or keep the repo layout (client/ next to judge/)"
                }
            else:
                judge_cmd = [
                    sys.executable,
                    judge_py,
                    "--image",
                    out_path,
                    "--target",
                    args.judge,
                ]
                judge_input = None
                if args.judge_key_stdin:
                    judge_cmd.append("--key-stdin")
                    judge_input = sys.stdin.readline()
                proc = subprocess.run(
                    judge_cmd,
                    input=judge_input,
                    capture_output=True,
                    text=True,
                    timeout=180,
                )
                try:
                    out["judge"] = json.loads(proc.stdout)
                except json.JSONDecodeError:
                    out["judge"] = {
                        "error": proc.stdout[-500:],
                        "stderr": proc.stderr[-500:],
                    }
        print(json.dumps(out, ensure_ascii=False))


if __name__ == "__main__":
    main()
