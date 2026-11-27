//! The settings window (`winmuse settings`): an egui form over
//! `%LOCALAPPDATA%\winmuse\config.toml`. The resident process
//! hot-reloads whatever this writes — no IPC, and this window can
//! crash or close without touching the server. Jev settings are
//! deliberately NOT here: the judge runs on the agent machine.
//!
//! UI language follows the OS: Japanese on Japanese Windows,
//! English elsewhere (the bundled egui fonts have no Japanese
//! glyphs, so English is also the safe fallback when no Japanese
//! system font is installed).

use std::collections::BTreeSet;

use eframe::egui;

use crate::config::{self, Config, Profile};
use crate::win;

/// Allowlisting one of these deserves a warning: through them the
/// agent can reach local files.
const RISKY: [&str; 10] = [
    "windowsterminal.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "explorer.exe",
    "code.exe",
    "1password.exe",
    "keepass.exe",
    "keepassxc.exe",
    "bitwarden.exe",
];

/// All user-visible strings, in the OS language.
struct Txt {
    window_title: &'static str,
    welcome: &'static str,
    status_serving: &'static str, // {addr} {profile} {state}
    status_stopped: &'static str, // {profile}
    status_down: &'static str,
    profile_heading: &'static str,
    profile_observe: &'static str,
    profile_guided: &'static str,
    profile_full: &'static str,
    allow_heading: &'static str,
    allow_desc: &'static str,
    not_running_suffix: &'static str,
    manual_add: &'static str,
    add: &'static str,
    refresh: &'static str,
    risky_warn: &'static str,
    safety_heading: &'static str,
    block_win_keys: &'static str,
    require_local_arm: &'static str,
    audit_heading: &'static str,
    audit_enable: &'static str,
    audit_open: &'static str,
    audit_path_prefix: &'static str,
    bridge_heading: &'static str,
    endpoint_prefix: &'static str,
    token_label: &'static str,
    token_gen: &'static str,
    token_clear: &'static str,
    token_note: &'static str,
    token_client: &'static str,
    overlay_heading: &'static str,
    overlay_enable: &'static str,
    toast_lang_label: &'static str,
    toast_lang_opts: [&'static str; 3],
    toast_title: &'static str,
    toast_sub: &'static str,
    toast_note: &'static str,
    save: &'static str,
    close: &'static str,
    saved_ok: &'static str,
    saved_err_prefix: &'static str,
}

const JA: Txt = Txt {
    window_title: "winmuse 設定",
    welcome: "ようこそ。最初に権限プロファイルを選んで保存してください。選ぶまでは読み取り専用（observe）で動きます。",
    status_serving: "常駐プロセス: 応答中（{} ・ プロファイル {} ・ 状態 {}）",
    status_stopped: "常駐プロセス: 起動していますがサーバー停止中（プロファイル {}）",
    status_down: "常駐プロセス: 動いていません（トレイから起動できます）",
    profile_heading: "権限プロファイル",
    profile_observe: "observe — 読み取りのみ。スクリーンショットと窓一覧だけが使えます",
    profile_guided: "guided — 下で許可したアプリの中だけ操作できます",
    profile_full: "full — 制限なし（従来の Python 版ブリッジと同じ）",
    allow_heading: "許可アプリ（guided）",
    allow_desc: "いま画面に出ているアプリから選べます。ポインタ操作は「座標の下のアプリ」、キー操作は「前面のアプリ」で判定します。",
    not_running_suffix: "（起動していません）",
    manual_add: "手動追加:",
    add: "追加",
    refresh: "一覧を更新",
    risky_warn: "注意: ターミナルや Explorer を許可すると、エージェントがそのアプリ経由でローカルファイルに到達できます。",
    safety_heading: "操作の安全",
    block_win_keys: "Windows キーのショートカットをブロックする（guided）",
    require_local_arm: "操作の前に PC 側で F9 キーの許可を必須にする",
    audit_heading: "監査ログ",
    audit_enable: "すべてのツール呼び出しを記録する",
    audit_open: "監査ログを開く",
    audit_path_prefix: "保存先: ",
    bridge_heading: "ブリッジ",
    endpoint_prefix: "接続先: ",
    token_label: "Bearer トークン:",
    token_gen: "生成",
    token_clear: "クリア",
    token_note: "トークンを設定すると、クライアントはヘッダー Authorization: Bearer <token> を送る必要があります。",
    token_client: "クライアント側: client.py --token <設定したトークン>（トークン自体はここには表示しません）",
    overlay_heading: "オーバーレイ",
    overlay_enable: "操作中オーバーレイ（発光フレーム＋トースト）を表示する",
    toast_lang_label: "トーストの言語",
    toast_lang_opts: ["おまかせ（OS の言語）", "日本語", "English"],
    toast_title: "トースト題（操作中）:",
    toast_sub: "トースト副題（操作中）:",
    toast_note: "題・副題は空欄なら言語プリセットが使われます。",
    save: "保存",
    close: "閉じる",
    saved_ok: "保存しました。数秒で常駐プロセスに自動反映されます。",
    saved_err_prefix: "保存に失敗しました: ",
};

const EN: Txt = Txt {
    window_title: "winmuse Settings",
    welcome: "Welcome. Choose a permission profile and save. Until you do, the server runs read-only (observe).",
    status_serving: "Resident: serving ({} - profile {} - state {})",
    status_stopped: "Resident: running, but the server is stopped (profile {})",
    status_down: "Resident: not running (you can start it from the tray)",
    profile_heading: "Permission profile",
    profile_observe: "observe — read-only. Screenshots and window lists only",
    profile_guided: "guided — input only inside the apps allowed below",
    profile_full: "full — unrestricted (same as the legacy Python bridge)",
    allow_heading: "Allowed apps (guided)",
    allow_desc: "Pick from the apps currently on screen. Pointer input is judged by the app under the point; keyboard input by the foreground app.",
    not_running_suffix: " (not running)",
    manual_add: "Add manually:",
    add: "Add",
    refresh: "Refresh list",
    risky_warn: "Warning: allowing a terminal or Explorer lets the agent reach local files through that app.",
    safety_heading: "Input safety",
    block_win_keys: "Block Windows-key shortcuts (guided)",
    require_local_arm: "Require a local F9 press before operating",
    audit_heading: "Audit log",
    audit_enable: "Record every tool call",
    audit_open: "Open audit log",
    audit_path_prefix: "Location: ",
    bridge_heading: "Bridge",
    endpoint_prefix: "Endpoint: ",
    token_label: "Bearer token:",
    token_gen: "Generate",
    token_clear: "Clear",
    token_note: "With a token set, clients must send the header Authorization: Bearer <token>.",
    token_client: "Client: client.py --token <your token> (the token itself is not shown here)",
    overlay_heading: "Overlay",
    overlay_enable: "Show the operation overlay (glow frame + toast)",
    toast_lang_label: "Toast language",
    toast_lang_opts: ["Auto (OS language)", "日本語", "English"],
    toast_title: "Toast title (operating):",
    toast_sub: "Toast subtitle (operating):",
    toast_note: "Leave title/subtitle empty to use the language preset.",
    save: "Save",
    close: "Close",
    saved_ok: "Saved. The resident picks it up within a few seconds.",
    saved_err_prefix: "Save failed: ",
};

fn txt() -> &'static Txt {
    if config::os_is_japanese() {
        &JA
    } else {
        &EN
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 700.0])
            .with_min_inner_size([660.0, 540.0])
            .with_title(txt().window_title),
        ..Default::default()
    };
    eframe::run_native(
        "winmuse settings",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::load()))
        }),
    )?;
    Ok(())
}

/// egui's bundled fonts have no Japanese glyphs; load Yu Gothic
/// (or another Japanese system font) from the Windows fonts folder.
/// On a system without any of them the bundled fonts stay in
/// effect, which is fine for the English UI.
fn install_fonts(ctx: &egui::Context) {
    let candidates = [
        "C:\\Windows\\Fonts\\YuGothM.ttc",
        "C:\\Windows\\Fonts\\YuGothB.ttc",
        "C:\\Windows\\Fonts\\msgothic.ttc",
        "C:\\Windows\\Fonts\\meiryo.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "jp".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "jp".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("jp".to_owned());
            ctx.set_fonts(fonts);
            return;
        }
    }
}

struct SettingsApp {
    loaded: Config,
    profile: Profile,
    allow: BTreeSet<String>,
    running: Vec<(String, String)>,
    manual_add: String,
    block_win_keys: bool,
    require_local_arm: bool,
    audit_enabled: bool,
    token: String,
    overlay_enabled: bool,
    overlay_lang: usize, // 0 auto, 1 ja, 2 en
    overlay_title: String,
    overlay_sub: String,
    status: String,
    status_checked: std::time::Instant,
    saved_note: String,
    first_run: bool,
}

impl SettingsApp {
    fn load() -> Self {
        let cfg = config::load(&config::config_path()).unwrap_or_default();
        let mut app = Self {
            profile: cfg.profile,
            allow: cfg.allow_processes.iter().cloned().collect(),
            running: win::running_apps(),
            manual_add: String::new(),
            block_win_keys: cfg.block_win_keys,
            require_local_arm: cfg.require_local_arm,
            audit_enabled: cfg.audit_enabled,
            token: cfg.auth_token.clone().unwrap_or_default(),
            overlay_enabled: cfg.overlay_enabled,
            overlay_lang: match cfg.overlay_lang.as_str() {
                "ja" => 1,
                "en" => 2,
                _ => 0,
            },
            overlay_title: cfg.overlay_title.clone(),
            overlay_sub: cfg.overlay_sub.clone(),
            status: String::new(),
            status_checked: std::time::Instant::now() - std::time::Duration::from_secs(10),
            saved_note: String::new(),
            first_run: !cfg.profile_chosen,
            loaded: cfg,
        };
        app.refresh_status();
        app
    }

    fn refresh_status(&mut self) {
        let t = txt();
        self.status_checked = std::time::Instant::now();
        let path = config::data_dir().join("status.json");
        self.status = match std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        {
            Some(v) => {
                let serving = v["serving"].as_bool().unwrap_or(false);
                let addr = v["addr"].as_str().unwrap_or("—");
                let profile = v["profile"].as_str().unwrap_or("—");
                let state = v["op_state"].as_str().unwrap_or("—");
                if serving {
                    // The template has three placeholders; fill by
                    // simple replacement to keep the strings table
                    // free of format-arg ordering concerns.
                    t.status_serving
                        .replacen("{}", addr, 1)
                        .replacen("{}", profile, 1)
                        .replacen("{}", state, 1)
                } else {
                    t.status_stopped.replacen("{}", profile, 1)
                }
            }
            None => t.status_down.to_string(),
        };
    }

    fn collect_config(&self) -> Config {
        let mut cfg = self.loaded.clone();
        cfg.profile = self.profile;
        cfg.profile_chosen = true;
        cfg.allow_processes = self.allow.iter().cloned().collect();
        cfg.block_win_keys = self.block_win_keys;
        cfg.require_local_arm = self.require_local_arm;
        cfg.audit_enabled = self.audit_enabled;
        let token = self.token.trim().to_string();
        cfg.auth_token = if token.is_empty() { None } else { Some(token) };
        cfg.overlay_enabled = self.overlay_enabled;
        cfg.overlay_lang = match self.overlay_lang {
            1 => "ja".to_string(),
            2 => "en".to_string(),
            _ => String::new(),
        };
        cfg.overlay_title = self.overlay_title.clone();
        cfg.overlay_sub = self.overlay_sub.clone();
        cfg
    }

    fn save(&mut self) {
        let t = txt();
        match config::save(&config::config_path(), &self.collect_config()) {
            Ok(()) => {
                self.first_run = false;
                self.saved_note = t.saved_ok.to_string();
            }
            Err(e) => {
                self.saved_note = format!("{}{e}", t.saved_err_prefix);
            }
        }
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.status_checked.elapsed() > std::time::Duration::from_secs(2) {
            self.refresh_status();
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        let t = txt();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(t.window_title);
            ui.label(&self.status);
            if self.first_run {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(66, 52, 10))
                    .inner_margin(egui::Margin::same(10))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.label(t.welcome);
                    });
            }
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading(t.profile_heading);
                for (p, desc) in [
                    (Profile::Observe, t.profile_observe),
                    (Profile::Guided, t.profile_guided),
                    (Profile::Full, t.profile_full),
                ] {
                    ui.radio_value(&mut self.profile, p, desc);
                }

                ui.add_space(10.0);
                ui.heading(t.allow_heading);
                ui.label(t.allow_desc);
                let mut toggle: Vec<(String, bool)> = Vec::new();
                for (proc, title) in self.running.clone() {
                    let mut checked = self.allow.contains(&proc.to_lowercase());
                    let label = if title.is_empty() {
                        proc.clone()
                    } else {
                        format!("{proc} — {title}")
                    };
                    if ui.checkbox(&mut checked, label).changed() {
                        toggle.push((proc.to_lowercase(), checked));
                    }
                }
                let running_lower: BTreeSet<String> = self
                    .running
                    .iter()
                    .map(|(p, _)| p.to_lowercase())
                    .collect();
                for proc in self.allow.clone() {
                    if !running_lower.contains(&proc) {
                        let mut checked = true;
                        if ui
                            .checkbox(
                                &mut checked,
                                format!("{proc}{}", t.not_running_suffix),
                            )
                            .changed()
                        {
                            toggle.push((proc.clone(), checked));
                        }
                    }
                }
                for (proc, on) in toggle {
                    if on {
                        self.allow.insert(proc);
                    } else {
                        self.allow.remove(&proc);
                    }
                }
                ui.horizontal(|ui| {
                    ui.label(t.manual_add);
                    ui.text_edit_singleline(&mut self.manual_add);
                    if ui.button(t.add).clicked() {
                        let name = self.manual_add.trim().to_lowercase();
                        if !name.is_empty() {
                            self.allow.insert(name);
                            self.manual_add.clear();
                        }
                    }
                    if ui.button(t.refresh).clicked() {
                        self.running = win::running_apps();
                    }
                });
                if self.allow.iter().any(|p| RISKY.contains(&p.as_str())) {
                    ui.colored_label(
                        egui::Color32::from_rgb(248, 180, 60),
                        t.risky_warn,
                    );
                }

                ui.add_space(10.0);
                ui.heading(t.safety_heading);
                ui.checkbox(&mut self.block_win_keys, t.block_win_keys);
                ui.checkbox(&mut self.require_local_arm, t.require_local_arm);

                ui.add_space(10.0);
                ui.heading(t.audit_heading);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.audit_enabled, t.audit_enable);
                    if ui.button(t.audit_open).clicked() {
                        let path = config::audit_path();
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if !path.exists() {
                            let _ = std::fs::write(&path, "");
                        }
                        let _ = std::process::Command::new("notepad.exe").arg(&path).spawn();
                    }
                });
                ui.label(format!(
                    "{}{}",
                    t.audit_path_prefix,
                    config::audit_path().display()
                ));

                ui.add_space(10.0);
                ui.heading(t.bridge_heading);
                ui.label(format!("{}{}", t.endpoint_prefix, self.status));
                ui.horizontal(|ui| {
                    ui.label(t.token_label);
                    ui.add(egui::TextEdit::singleline(&mut self.token).password(true).desired_width(260.0));
                    if ui.button(t.token_gen).clicked() {
                        let bytes: [u8; 16] = rand::random();
                        self.token = bytes.iter().map(|b| format!("{b:02x}")).collect();
                    }
                    if ui.button(t.token_clear).clicked() {
                        self.token.clear();
                    }
                });
                ui.label(t.token_note);
                if !self.token.trim().is_empty() {
                    // Never render the raw token: it would leak into
                    // screenshots and screen shares. A placeholder
                    // tells the user everything they need.
                    ui.label(t.token_client);
                }

                ui.add_space(10.0);
                ui.heading(t.overlay_heading);
                ui.checkbox(&mut self.overlay_enabled, t.overlay_enable);
                egui::ComboBox::from_label(t.toast_lang_label)
                    .selected_text(t.toast_lang_opts[self.overlay_lang])
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.overlay_lang, 0, t.toast_lang_opts[0]);
                        ui.selectable_value(&mut self.overlay_lang, 1, t.toast_lang_opts[1]);
                        ui.selectable_value(&mut self.overlay_lang, 2, t.toast_lang_opts[2]);
                    });
                ui.horizontal(|ui| {
                    ui.label(t.toast_title);
                    ui.text_edit_singleline(&mut self.overlay_title);
                });
                ui.horizontal(|ui| {
                    ui.label(t.toast_sub);
                    ui.text_edit_singleline(&mut self.overlay_sub);
                });
                ui.label(t.toast_note);
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(t.save).clicked() {
                    self.save();
                }
                if ui.button(t.close).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ui.label(&self.saved_note);
            });
        });
    }
}
