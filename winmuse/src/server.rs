//! MCP server: ServerHandler implementation and tool dispatch.
//! Tool names, arguments and result shapes mirror the Python server.

use std::sync::Arc;

use base64::Engine;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{json, Map, Value};

use crate::audit;
use crate::capture;
use crate::config::{Config, Profile};
use crate::input;
use crate::state::Shared;
use crate::win;

#[derive(Clone)]
pub struct CuServer {
    shared: Arc<Shared>,
}

impl CuServer {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    fn tools() -> Vec<Tool> {
        let schema = |v: Value| v.as_object().unwrap().clone();
        let empty = || schema(json!({"type": "object", "properties": {}}));
        vec![
            Tool::new(
                "operation_begin",
                "Begin an operation session: clears any ESC cancellation and shows the on-screen overlay (blue glow frame + toast) so the user can see that muse is operating. Call before a run of actions.",
                schema(json!({"type": "object", "properties": {
                    "label": {"type": "string", "default": ""}}})),
            ),
            Tool::new(
                "operation_end",
                "End the operation session and hide the overlay.",
                empty(),
            ),
            Tool::new(
                "operation_status",
                "Operation/overlay state: active, cancelled, overlay visible.",
                empty(),
            ),
            Tool::new(
                "screenshot",
                "Capture the primary display. Returns a PNG (at most 1280 px wide) plus a line of text with the real and scaled sizes. Give mouse coordinates in the returned image's pixel space.",
                empty(),
            ),
            Tool::new(
                "screen_info",
                "Real display size, current screenshot scale, cursor position.",
                empty(),
            ),
            Tool::new(
                "mouse_move",
                "Move the cursor to (x, y) in scaled screenshot coordinates.",
                schema(json!({"type": "object", "properties": {
                    "x": {"type": "integer"}, "y": {"type": "integer"}},
                    "required": ["x", "y"]})),
            ),
            Tool::new(
                "click",
                "Click at (x, y) in scaled screenshot coordinates.",
                schema(json!({"type": "object", "properties": {
                    "x": {"type": "integer"}, "y": {"type": "integer"},
                    "button": {"type": "string", "default": "left"},
                    "clicks": {"type": "integer", "default": 1}},
                    "required": ["x", "y"]})),
            ),
            Tool::new(
                "drag",
                "Drag from (x1, y1) to (x2, y2), scaled screenshot coordinates.",
                schema(json!({"type": "object", "properties": {
                    "x1": {"type": "integer"}, "y1": {"type": "integer"},
                    "x2": {"type": "integer"}, "y2": {"type": "integer"},
                    "duration": {"type": "number", "default": 0.5}},
                    "required": ["x1", "y1", "x2", "y2"]})),
            ),
            Tool::new(
                "scroll",
                "Scroll the wheel. Positive amount scrolls up, negative down. Optionally move to (x, y) first (scaled coordinates).",
                schema(json!({"type": "object", "properties": {
                    "amount": {"type": "integer"},
                    "x": {"type": "integer", "default": -1},
                    "y": {"type": "integer", "default": -1}},
                    "required": ["amount"]})),
            ),
            Tool::new(
                "key_press",
                "Press a key or shortcut, e.g. 'enter', 'ctrl+c', 'alt+tab', 'win+r', 'ctrl+shift+esc'. Key names follow pyautogui conventions. Note: pressing ESC also triggers the user's cancel, by design.",
                schema(json!({"type": "object", "properties": {
                    "keys": {"type": "string"}}, "required": ["keys"]})),
            ),
            Tool::new(
                "type_text",
                "Type text at the current focus. Pasted via the clipboard (Ctrl+V), so Japanese and other Unicode text works. Note: this replaces the clipboard contents.",
                schema(json!({"type": "object", "properties": {
                    "text": {"type": "string"}}, "required": ["text"]})),
            ),
            Tool::new(
                "list_windows",
                "List visible windows with non-empty titles (hwnd + title).",
                empty(),
            ),
            Tool::new(
                "focus_window",
                "Bring the first visible window whose title contains `title` (case-insensitive) to the foreground.",
                schema(json!({"type": "object", "properties": {
                    "title": {"type": "string"}}, "required": ["title"]})),
            ),
        ]
    }

    /// Screenshot with the operation overlay briefly suspended, so
    /// captures never contain the frame/toast.
    fn grab_clean_png(&self) -> Result<Vec<u8>, String> {
        {
            let mut op = self.shared.op.lock().unwrap();
            op.suspend += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(180));
        let result = capture::grab_scaled_png(&self.shared);
        {
            let mut op = self.shared.op.lock().unwrap();
            op.suspend = op.suspend.saturating_sub(1);
        }
        result
    }

    /// The decision pipeline every call passes through:
    /// operation tools handle themselves; read-only tools just mark
    /// activity; action tools face, in order, the ESC cancel
    /// (CANCELLED:), the local-arm gate and the permission profile
    /// (REFUSED:). Every call is audit-logged with its decision.
    fn dispatch(&self, name: &str, args: Option<&Map<String, Value>>) -> CallToolResult {
        let cfg = self.shared.config.read().unwrap().clone();
        let summary = audit::summarize_args(name, args);
        let log = |decision: &str| {
            if cfg.audit_enabled {
                audit::log(
                    &self.shared.audit_path,
                    name,
                    cfg.profile.as_str(),
                    decision,
                    &summary,
                );
            }
        };
        const READ_ONLY: [&str; 3] = ["screenshot", "screen_info", "list_windows"];
        if matches!(name, "operation_begin" | "operation_end" | "operation_status") {
            let (result, decision) = self.dispatch_op(name, args, &cfg);
            log(decision);
            return result;
        }
        if READ_ONLY.contains(&name) {
            self.shared.op_touch();
            let result = self.dispatch_body(name, args);
            log("ok");
            return result;
        }
        // Action tools.
        if let Some(msg) = self.shared.op_gate() {
            log("cancelled");
            return CallToolResult::success(vec![ContentBlock::text(msg)]);
        }
        if cfg.require_local_arm && !self.shared.op.lock().unwrap().armed {
            log("refused");
            return CallToolResult::success(vec![ContentBlock::text(
                "REFUSED: local arm required \u{2014} the PC user must press F9 after operation_begin before input is accepted",
            )]);
        }
        if let Some(reason) = self.profile_check(name, args, &cfg) {
            log("refused");
            return CallToolResult::success(vec![ContentBlock::text(format!(
                "REFUSED: {reason}"
            ))]);
        }
        let result = self.dispatch_body(name, args);
        log("ok");
        result
    }

    fn dispatch_body(&self, name: &str, args: Option<&Map<String, Value>>) -> CallToolResult {
        let empty_map = Map::new();
        let a = args.unwrap_or(&empty_map);
        let gi = |k: &str, dflt: i64| a.get(k).and_then(|v| v.as_i64()).unwrap_or(dflt);
        let gf = |k: &str, dflt: f64| a.get(k).and_then(|v| v.as_f64()).unwrap_or(dflt);
        let gs = |k: &str, dflt: &str| {
            a.get(k).and_then(|v| v.as_str()).unwrap_or(dflt).to_string()
        };
        let text = |s: String| CallToolResult::success(vec![ContentBlock::text(s)]);
        let err = |s: String| CallToolResult::error(vec![ContentBlock::text(s)]);

        match name {
            "screenshot" => match self.grab_clean_png() {
                Ok(png) => {
                    let st = self.shared.scale.lock().unwrap();
                    let info = format!(
                        "real={}x{} scaled={}x{} scale={:.4}",
                        st.real_w, st.real_h, st.scaled_w, st.scaled_h, st.scale
                    );
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                    CallToolResult::success(vec![ContentBlock::image(b64, "image/png"), ContentBlock::text(info)])
                }
                Err(e) => err(format!("screenshot failed: {e}")),
            },
            "screen_info" => {
                {
                    let st = self.shared.scale.lock().unwrap();
                    if st.real_w == 0 {
                        drop(st);
                        let _ = self.grab_clean_png();
                    }
                }
                let st = self.shared.scale.lock().unwrap();
                let (cx, cy) = win::cursor_pos();
                text(json!({
                    "real_width": st.real_w,
                    "real_height": st.real_h,
                    "scaled_width": st.scaled_w,
                    "scaled_height": st.scaled_h,
                    "scale": st.scale,
                    "cursor_real": [cx, cy],
                })
                .to_string())
            }
            "mouse_move" => {
                let (rx, ry) = self.shared.to_real(gi("x", 0), gi("y", 0));
                match input::move_to(rx, ry) {
                    Ok(()) => text(format!("cursor at real ({rx}, {ry})")),
                    Err(e) => err(e),
                }
            }
            "click" => {
                let (rx, ry) = self.shared.to_real(gi("x", 0), gi("y", 0));
                let button = gs("button", "left");
                let clicks = gi("clicks", 1);
                match input::click(rx, ry, &button, clicks) {
                    Ok(()) => text(format!("clicked {button} x{clicks} at real ({rx}, {ry})")),
                    Err(e) => err(e),
                }
            }
            "drag" => {
                let (rx1, ry1) = self.shared.to_real(gi("x1", 0), gi("y1", 0));
                let (rx2, ry2) = self.shared.to_real(gi("x2", 0), gi("y2", 0));
                match input::drag(rx1, ry1, rx2, ry2, gf("duration", 0.5)) {
                    Ok(()) => text(format!("dragged real ({rx1}, {ry1}) -> ({rx2}, {ry2})")),
                    Err(e) => err(e),
                }
            }
            "scroll" => {
                let (x, y) = (gi("x", -1), gi("y", -1));
                let at = if x >= 0 && y >= 0 {
                    Some(self.shared.to_real(x, y))
                } else {
                    None
                };
                let amount = gi("amount", 0);
                match input::scroll(amount, at) {
                    Ok(()) => text(format!("scrolled {amount}")),
                    Err(e) => err(e),
                }
            }
            "key_press" => match input::key_press(&gs("keys", "")) {
                Ok(joined) => text(format!("pressed {joined}")),
                Err(e) => err(e),
            },
            "type_text" => match input::type_text(&gs("text", "")) {
                Ok(n) => text(format!("pasted {n} chars")),
                Err(e) => err(e),
            },
            "list_windows" => {
                let list: Vec<Value> = win::list_windows()
                    .into_iter()
                    .map(|(hwnd, title)| json!({"hwnd": hwnd, "title": title}))
                    .collect();
                text(Value::Array(list).to_string())
            }
            "focus_window" => match win::focus_window(&gs("title", "")) {
                Ok(msg) => text(msg),
                Err(e) => err(e),
            },
            _ => err(format!("unknown tool '{name}'")),
        }
    }
}

impl CuServer {
    fn dispatch_op(
        &self,
        name: &str,
        args: Option<&Map<String, Value>>,
        cfg: &Config,
    ) -> (CallToolResult, &'static str) {
        let text = |s: String| CallToolResult::success(vec![ContentBlock::text(s)]);
        match name {
            "operation_begin" => {
                if cfg.profile == Profile::Observe {
                    return (
                        text("REFUSED: profile 'observe' is read-only; operation_begin is not allowed".to_string()),
                        "refused",
                    );
                }
                let label = args
                    .and_then(|a| a.get("label"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if cfg.require_local_arm {
                    let mut op = self.shared.op.lock().unwrap();
                    op.cancelled = false;
                    op.active = false;
                    op.armed = false;
                    op.awaiting_arm = true;
                    op.flash_until = None;
                    op.last_call = Some(std::time::Instant::now());
                    op.label = label;
                    (
                        text("operation begun; waiting for the PC user to press F9 to arm (ESC denies)".to_string()),
                        "ok",
                    )
                } else {
                    let mut op = self.shared.op.lock().unwrap();
                    op.cancelled = false;
                    op.active = true;
                    op.flash_until = None;
                    op.last_call = Some(std::time::Instant::now());
                    op.label = label;
                    (
                        text("operation begun; overlay shown; ESC cancels".to_string()),
                        "ok",
                    )
                }
            }
            "operation_end" => {
                let was_cancelled = {
                    let mut op = self.shared.op.lock().unwrap();
                    op.active = false;
                    op.awaiting_arm = false;
                    op.armed = false;
                    op.cancelled
                };
                if was_cancelled {
                    (text("operation ended (was cancelled)".to_string()), "ok")
                } else {
                    (text("operation ended; overlay hidden".to_string()), "ok")
                }
            }
            _ => {
                // operation_status
                let op = self.shared.op.lock().unwrap();
                (
                    text(json!({
                        "active": op.active,
                        "cancelled": op.cancelled,
                        "armed": op.armed,
                        "awaiting_arm": op.awaiting_arm,
                        "overlay_visible": op.visible,
                        "shown_mode": op.shown_mode.map(|m| m.as_str()),
                        "hwnd": op.hwnd,
                        "label": op.label,
                        "seconds_since_last_call": op.last_call.map(|t| (t.elapsed().as_secs_f64() * 10.0).round() / 10.0),
                        "profile": cfg.profile.as_str(),
                        "profile_chosen": cfg.profile_chosen,
                        "allow_processes": cfg.allow_processes,
                        "require_local_arm": cfg.require_local_arm,
                        "config_note": cfg.source_note,
                    })
                    .to_string()),
                    "ok",
                )
            }
        }
    }

    /// None = allowed; Some(reason) = refuse (guided/observe rules).
    fn profile_check(
        &self,
        name: &str,
        args: Option<&Map<String, Value>>,
        cfg: &Config,
    ) -> Option<String> {
        match cfg.profile {
            Profile::Full => None,
            Profile::Observe => Some(format!(
                "profile 'observe' is read-only; input tool '{name}' is not allowed"
            )),
            Profile::Guided => {
                let empty_map = Map::new();
                let a = args.unwrap_or(&empty_map);
                let gi = |k: &str, dflt: i64| a.get(k).and_then(|v| v.as_i64()).unwrap_or(dflt);
                if name == "key_press" && cfg.block_win_keys {
                    let keys = a
                        .get("keys")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    // Every alias input.rs maps to the Windows key
                    // must be blocked, not just "win" — and Ctrl+Esc
                    // opens the Start menu too.
                    let parts: Vec<String> = keys
                        .split('+')
                        .map(|p| p.trim().to_lowercase())
                        .collect();
                    let has_win = parts.iter().any(|p| {
                        matches!(
                            p.as_str(),
                            "win" | "windows" | "cmd" | "command" | "meta" | "super"
                                | "lwin" | "rwin"
                        )
                    });
                    let has_ctrl = parts
                        .iter()
                        .any(|p| matches!(p.as_str(), "ctrl" | "control"));
                    let has_esc = parts
                        .iter()
                        .any(|p| matches!(p.as_str(), "esc" | "escape"));
                    if has_win || (has_ctrl && has_esc) {
                        return Some(
                            "Windows-key shortcuts are blocked in guided mode (block_win_keys)"
                                .to_string(),
                        );
                    }
                }
                match name {
                    "mouse_move" => None,
                    "click" => self.check_point(gi("x", 0), gi("y", 0), cfg, "click"),
                    "drag" => self
                        .check_point(gi("x1", 0), gi("y1", 0), cfg, "drag start")
                        .or_else(|| {
                            self.check_point(gi("x2", 0), gi("y2", 0), cfg, "drag end")
                        }),
                    "scroll" => {
                        let (x, y) = (gi("x", -1), gi("y", -1));
                        if x >= 0 && y >= 0 {
                            self.check_point(x, y, cfg, "scroll")
                        } else {
                            self.check_foreground(cfg, "scroll")
                        }
                    }
                    "key_press" | "type_text" => self.check_foreground(cfg, name),
                    "focus_window" => {
                        let title = a
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        match win::process_name_of_window_title(title) {
                            Some(p) if cfg.allows(&p) => None,
                            Some(p) => Some(format!(
                                "window '{title}' belongs to '{p}', which is not in the guided allowlist"
                            )),
                            None => None, // no such window; the tool itself reports it
                        }
                    }
                    _ => None,
                }
            }
        }
    }

    fn check_point(&self, x: i64, y: i64, cfg: &Config, what: &str) -> Option<String> {
        let (rx, ry) = self.shared.to_real(x, y);
        match win::process_name_at_point(rx, ry) {
            Some(p) if cfg.allows(&p) => None,
            Some(p) => Some(format!(
                "{what} would land in '{p}', which is not in the guided allowlist"
            )),
            None => Some(format!(
                "could not determine the process under the {what} point; refusing in guided mode"
            )),
        }
    }

    fn check_foreground(&self, cfg: &Config, what: &str) -> Option<String> {
        match win::foreground_process_name() {
            Some(p) if cfg.allows(&p) => None,
            Some(p) => Some(format!(
                "{what} would go to the foreground app '{p}', which is not in the guided allowlist"
            )),
            None => Some(format!(
                "could not determine the foreground app for {what}; refusing in guided mode"
            )),
        }
    }
}

impl ServerHandler for CuServer {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::default();
        config.capabilities = ServerCapabilities::builder().enable_tools().build();
        // from_build_env() would report rmcp's own name/version (the
        // env! macros expand in rmcp's crate), so set ours explicitly.
        let mut info = Implementation::default();
        info.name = "winmuse".to_string();
        info.version = env!("CARGO_PKG_VERSION").to_string();
        config.server_info = info;
        config.instructions = Some(
            "Computer Use for this Windows PC. Screenshots are scaled to at most 1280 px wide; give mouse coordinates in the scaled image's pixel space."
                .to_string(),
        );
        config
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut result = ListToolsResult::default();
        result.tools = Self::tools();
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let result = self.dispatch(&request.name, request.arguments.as_ref());
        Ok(CallToolResponse::Complete(result))
    }
}

// ---------------------------------------------------------------------------
// Transport: bind detection, router construction, bearer-token auth.
// ---------------------------------------------------------------------------

use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

/// The first IPv4 address in the tailnet CGNAT range
/// (100.64.0.0/10) among this machine's adapters, if any.
pub fn tailnet_ipv4() -> Option<String> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::ADDRESS_FAMILY;
    unsafe {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size: u32 = 0;
        let rc = GetAdaptersAddresses(
            2u32, // AF_INET
            flags,
            None,
            None,
            &mut size,
        );
        if rc != 111 || size == 0 {
            return None; // not ERROR_BUFFER_OVERFLOW: no adapters?
        }
        let mut buf = vec![0usize; (size as usize).div_ceil(8)];
        let rc = GetAdaptersAddresses(
            2u32,
            flags,
            None,
            Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
            &mut size,
        );
        if rc != 0 {
            return None;
        }
        let mut adapter = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !adapter.is_null() {
            // Skip adapters that are not up: a down virtual NIC or
            // another CGNAT-facing adapter must not win over the
            // real Tailscale one.
            if (*adapter).OperStatus.0 != 1 {
                adapter = (*adapter).Next;
                continue;
            }
            let mut ua = (*adapter).FirstUnicastAddress;
            while !ua.is_null() {
                let sockaddr = (*ua).Address.lpSockaddr;
                if !sockaddr.is_null() && (*sockaddr).sa_family == ADDRESS_FAMILY(2) {
                    let sin = sockaddr as *const windows::Win32::Networking::WinSock::SOCKADDR_IN;
                    // S_addr holds the address in network byte
                    // order; on little-endian Windows its LE byte
                    // representation is the dotted-quad order.
                    let octets = (*sin).sin_addr.S_un.S_addr.to_le_bytes();
                    if octets[0] == 100 && (octets[1] & 0xC0) == 0x40 {
                        return Some(format!(
                            "{}.{}.{}.{}",
                            octets[0], octets[1], octets[2], octets[3]
                        ));
                    }
                }
                ua = (*ua).Next;
            }
            adapter = (*adapter).Next;
        }
        None
    }
}

/// Where to bind: WINMUSE_HOST/WINMUSE_PORT env, then the config's
/// [server] section, then the auto-detected tailnet address on the
/// default port 18081 (loopback if no tailnet address exists).
pub fn detect_bind(cfg: &Config) -> (String, u16) {
    let host = std::env::var("WINMUSE_HOST")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| cfg.server_host.clone())
        .or_else(|| tailnet_ipv4())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = std::env::var("WINMUSE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .or(cfg.server_port)
        .unwrap_or(18081);
    (host, port)
}

/// Constant-time byte comparison for the bearer token, so the
/// check does not leak how much of a guessed token was right.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn auth(
    axum::extract::State(shared): axum::extract::State<Arc<Shared>>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let token = shared.config.read().unwrap().auth_token.clone();
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        let expected = format!("Bearer {t}");
        let ok = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map_or(false, |v| ct_eq(v.as_bytes(), expected.as_bytes()));
        if !ok {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                "unauthorized: a bearer token is required\n",
            )
                .into_response();
        }
    }
    next.run(req).await
}

/// Build the axum app for one serving run (fresh MCP sessions per
/// run; clients re-initialize after a stop/start).
pub fn build_router(shared: Arc<Shared>, host: &str, port: u16) -> axum::Router {
    let mut http_config = StreamableHttpServerConfig::default();
    // rmcp defaults Host validation to loopback only; winmuse is
    // meant to be reachable at the tailnet address.
    http_config.allowed_hosts = vec![
        host.to_string(),
        format!("{host}:{port}"),
        "localhost".to_string(),
        format!("localhost:{port}"),
        "127.0.0.1".to_string(),
        format!("127.0.0.1:{port}"),
    ];
    let factory_shared = shared.clone();
    let service = StreamableHttpService::new(
        move || Ok(CuServer::new(factory_shared.clone())),
        Arc::new(LocalSessionManager::default()),
        http_config,
    );
    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(shared, auth))
}
