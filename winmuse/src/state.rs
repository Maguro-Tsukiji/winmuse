use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, RwLock};
use std::time::Instant;

use crate::config::Config;

/// Screenshot scale state, mirroring the Python server's `_state`:
/// mouse coordinates arrive in the scaled screenshot's pixel space
/// and are converted back to real pixels with the most recent
/// screenshot's scale.
#[derive(Default)]
pub struct ScaleState {
    pub scale: f64, // 0.0 = no screenshot taken yet
    pub real_w: u32,
    pub real_h: u32,
    pub scaled_w: u32,
    pub scaled_h: u32,
}

/// What the operation overlay is currently showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OverlayMode {
    Active,
    Waiting,
    Cancelled,
}

impl OverlayMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OverlayMode::Active => "active",
            OverlayMode::Waiting => "waiting",
            OverlayMode::Cancelled => "cancelled",
        }
    }
}

/// Operation session state (the Python server's `_op` dict).
pub struct OpState {
    pub active: bool,    // an operation session is in progress
    pub cancelled: bool, // ESC pressed during the operation
    pub armed: bool,        // F9 pressed (only used with require_local_arm)
    pub awaiting_arm: bool, // operation_begin is waiting for F9
    pub last_call: Option<Instant>,
    pub flash_until: Option<Instant>, // show the "cancelled" toast until then
    pub suspend: u32,                 // >0 while a screenshot is being captured
    pub hwnd: isize,
    pub visible: bool,
    pub shown_mode: Option<OverlayMode>,
    pub label: String,
}

impl Default for OpState {
    fn default() -> Self {
        Self {
            active: false,
            cancelled: false,
            armed: false,
            awaiting_arm: false,
            last_call: None,
            flash_until: None,
            suspend: 0,
            hwnd: 0,
            visible: false,
            shown_mode: None,
            label: String::new(),
        }
    }
}

pub const CANCELLED_MSG: &str = "CANCELLED: the user pressed ESC and stopped the operation. \
Do not retry any action. Report the cancellation to the user; \
call operation_begin only for a new, user-approved operation.";

pub const IDLE_TIMEOUT_S: f64 = 600.0; // auto-end an operation after 10 idle minutes

/// Start/stop control for the HTTP server, flipped from the tray
/// menu and executed by the supervisor task in main.
pub struct ServerControl {
    pub desired: AtomicBool,
    pub running: AtomicBool,
}

impl Default for ServerControl {
    fn default() -> Self {
        Self {
            desired: AtomicBool::new(true),
            running: AtomicBool::new(false),
        }
    }
}

pub struct Shared {
    pub scale: Mutex<ScaleState>,
    pub op: Mutex<OpState>,
    pub config: RwLock<Config>,
    pub audit_path: PathBuf,
    pub control: ServerControl,
    /// The address the server is currently bound to, for status.
    pub listen: RwLock<Option<String>>,
}

impl Shared {
    pub fn new(config: Config, audit_path: PathBuf) -> Self {
        Self {
            scale: Mutex::new(ScaleState::default()),
            op: Mutex::new(OpState::default()),
            config: RwLock::new(config),
            audit_path,
            control: ServerControl::default(),
            listen: RwLock::new(None),
        }
    }

    pub fn to_real(&self, x: i64, y: i64) -> (i32, i32) {
        let st = self.scale.lock().unwrap();
        let s = if st.scale > 0.0 { st.scale } else { 1.0 };
        (
            (x as f64 / s).round() as i32,
            (y as f64 / s).round() as i32,
        )
    }

    /// Mark activity; auto-begin an operation on any tool call
    /// (unless the user has cancelled — only operation_begin
    /// re-arms — or local arming is required and has not happened).
    pub fn op_touch(&self) {
        let require_arm = self.config.read().unwrap().require_local_arm;
        let mut op = self.op.lock().unwrap();
        op.last_call = Some(Instant::now());
        if !op.cancelled && (!require_arm || op.armed) {
            op.active = true;
        }
    }

    /// Touch, then return the cancellation message if the user has
    /// cancelled with ESC.
    pub fn op_gate(&self) -> Option<&'static str> {
        self.op_touch();
        let op = self.op.lock().unwrap();
        if op.cancelled {
            Some(CANCELLED_MSG)
        } else {
            None
        }
    }
}
