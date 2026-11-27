//! winmuse: Computer Use MCP server for Windows.
//!
//! One binary, two faces:
//!   winmuse            the resident process (MCP server + tray icon
//!                      + operation overlay), single-instance
//!   winmuse settings   the settings window (separate process)
//!
//! The server binds to this PC's tailnet address (auto-detected,
//! config-overridable) on port 18081. Configuration lives in
//! %LOCALAPPDATA%\winmuse\config.toml and hot-reloads; with no
//! config the server runs read-only in the `observe` profile.

// Built for the Windows (GUI) subsystem so the resident process
// shows no console window on the desktop. Diagnostics therefore go
// to a log file (%LOCALAPPDATA%\winmuse\winmuse.log), not stdout.
#![windows_subsystem = "windows"]

mod audit;
mod capture;
mod config;
mod gui;
mod input;
mod overlay;
mod server;
mod state;
mod status;
mod tray;
mod win;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, GetLastError, WIN32_ERROR};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, ResetEvent, SetEvent, WaitForSingleObject,
    EVENT_MODIFY_STATE,
};

/// Append-only log file writer for tracing (there is no console in
/// the Windows subsystem to write to).
struct LogWriter(std::sync::Arc<std::sync::Mutex<std::fs::File>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

struct LogFile(std::sync::Arc<std::sync::Mutex<std::fs::File>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFile {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

fn signal_open_settings() {
    unsafe {
        if let Ok(ev) = OpenEventW(
            EVENT_MODIFY_STATE,
            false,
            windows::core::w!("Local\\winmuse-open-settings"),
        ) {
            let _ = SetEvent(ev);
            let _ = CloseHandle(ev);
        }
    }
}

fn spawn_open_settings_listener() {
    std::thread::spawn(|| unsafe {
        let ev = match CreateEventW(
            None,
            true,
            false,
            windows::core::w!("Local\\winmuse-open-settings"),
        ) {
            Ok(e) => e,
            Err(_) => return,
        };
        loop {
            let _ = WaitForSingleObject(ev, u32::MAX);
            let _ = ResetEvent(ev);
            tray::launch_settings();
        }
    });
}

/// Start/stop the HTTP server according to the tray's desired
/// state. Each start re-reads the live config for the bind address
/// and builds a fresh router (fresh MCP sessions).
async fn supervise(shared: Arc<state::Shared>) {
    let mut handle: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        let want = shared.control.desired.load(Ordering::Relaxed);
        match (want, handle.is_some()) {
            (true, false) => {
                let cfg = shared.config.read().unwrap().clone();
                let (host, port) = server::detect_bind(&cfg);
                match tokio::net::TcpListener::bind(format!("{host}:{port}")).await {
                    Ok(listener) => {
                        let app = server::build_router(shared.clone(), &host, port);
                        *shared.listen.write().unwrap() =
                            Some(format!("http://{host}:{port}/mcp"));
                        shared.control.running.store(true, Ordering::Relaxed);
                        tracing::info!("winmuse serving on http://{host}:{port}/mcp");
                        handle = Some(tokio::spawn(async move {
                            let _ = axum::serve(listener, app).await;
                        }));
                    }
                    Err(e) => {
                        tracing::error!("bind {host}:{port} failed: {e} (retrying)");
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
            (false, true) => {
                if let Some(h) = handle.take() {
                    h.abort();
                }
                shared.control.running.store(false, Ordering::Relaxed);
                *shared.listen.write().unwrap() = None;
                tracing::info!("winmuse serving stopped");
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Per-monitor DPI awareness first, so capture pixels and input
    // coordinates share one physical space at any display scaling.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
    if std::env::args().any(|a| a == "settings") {
        return gui::run();
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "winmuse=info,rmcp=warn".into());
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::data_dir().join("winmuse.log"));
    match log_file {
        Ok(f) => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(LogFile(std::sync::Arc::new(std::sync::Mutex::new(f))))
                .init();
        }
        Err(_) => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }

    // Single instance: a second launch just asks the running
    // instance to open Settings, then exits.
    let _mutex = unsafe { CreateMutexW(None, true, windows::core::w!("Local\\winmuse-resident")) };
    if unsafe { GetLastError() } == WIN32_ERROR(183) {
        // ERROR_ALREADY_EXISTS
        signal_open_settings();
        return Ok(());
    }

    let config_path = config::config_path();
    let initial = match config::load(&config_path) {
        Ok(cfg) => {
            tracing::info!(
                "config: profile={} ({})",
                cfg.profile.as_str(),
                cfg.source_note
            );
            cfg
        }
        Err(e) => {
            tracing::warn!("{e}; starting in observe");
            config::Config::default()
        }
    };
    let first_run = !initial.profile_chosen;
    let shared = Arc::new(state::Shared::new(initial, config::audit_path()));
    config::spawn_watcher(shared.clone(), config_path);
    spawn_open_settings_listener();
    overlay::spawn(shared.clone());
    status::spawn(shared.clone());
    tray::spawn(shared.clone());
    if first_run {
        // No profile chosen yet: the owner's first act is choosing
        // one in the settings window (there is no silent default).
        tray::launch_settings();
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(supervise(shared));
    Ok(())
}
