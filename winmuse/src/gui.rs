//! The settings window (`winmuse settings`): an egui form over
//! `%LOCALAPPDATA%\winmuse\config.toml`. The resident process
//! hot-reloads whatever this writes — no IPC, and this window can
//! crash or close without touching the server. Jev settings are
//! deliberately NOT here: the judge runs on the agent machine.

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

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 700.0])
            .with_min_inner_size([660.0, 540.0])
            .with_title("winmuse 設定"),
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
                    format!("常駐プロセス: 応答中（{addr} ・ プロファイル {profile} ・ 状態 {state}）")
                } else {
                    format!("常駐プロセス: 起動していますがサーバー停止中（プロファイル {profile}）")
                }
            }
            None => "常駐プロセス: 動いていません（トレイから起動できます）".to_string(),
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
        match config::save(&config::config_path(), &self.collect_config()) {
            Ok(()) => {
                self.first_run = false;
                self.saved_note = "保存しました。数秒で常駐プロセスに自動反映されます。".to_string();
            }
            Err(e) => {
                self.saved_note = format!("保存に失敗しました: {e}");
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

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("winmuse 設定");
            ui.label(&self.status);
            if self.first_run {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(66, 52, 10))
                    .inner_margin(egui::Margin::same(10))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.label("ようこそ。最初に権限プロファイルを選んで保存してください。選ぶまでは読み取り専用（observe）で動きます。");
                    });
            }
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("権限プロファイル");
                for (p, desc) in [
                    (Profile::Observe, "observe — 読み取りのみ。スクリーンショットと窓一覧だけが使えます"),
                    (Profile::Guided, "guided — 下で許可したアプリの中だけ操作できます"),
                    (Profile::Full, "full — 制限なし（従来の Python 版ブリッジと同じ）"),
                ] {
                    ui.radio_value(&mut self.profile, p, desc);
                }

                ui.add_space(10.0);
                ui.heading("許可アプリ（guided）");
                ui.label("いま画面に出ているアプリから選べます。ポインタ操作は「座標の下のアプリ」、キー操作は「前面のアプリ」で判定します。");
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
                            .checkbox(&mut checked, format!("{proc}（起動していません）"))
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
                    ui.label("手動追加:");
                    ui.text_edit_singleline(&mut self.manual_add);
                    if ui.button("追加").clicked() {
                        let name = self.manual_add.trim().to_lowercase();
                        if !name.is_empty() {
                            self.allow.insert(name);
                            self.manual_add.clear();
                        }
                    }
                    if ui.button("一覧を更新").clicked() {
                        self.running = win::running_apps();
                    }
                });
                if self.allow.iter().any(|p| RISKY.contains(&p.as_str())) {
                    ui.colored_label(
                        egui::Color32::from_rgb(248, 180, 60),
                        "注意: ターミナルや Explorer を許可すると、エージェントがそのアプリ経由でローカルファイルに到達できます。",
                    );
                }

                ui.add_space(10.0);
                ui.heading("操作の安全");
                ui.checkbox(&mut self.block_win_keys, "Windows キーのショートカットをブロックする（guided）");
                ui.checkbox(&mut self.require_local_arm, "操作の前に PC 側で F9 キーの許可を必須にする");

                ui.add_space(10.0);
                ui.heading("監査ログ");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.audit_enabled, "すべてのツール呼び出しを記録する");
                    if ui.button("監査ログを開く").clicked() {
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
                ui.label(format!("保存先: {}", config::audit_path().display()));

                ui.add_space(10.0);
                ui.heading("ブリッジ");
                ui.label(format!("接続先: {}", self.status));
                ui.horizontal(|ui| {
                    ui.label("Bearer トークン:");
                    ui.add(egui::TextEdit::singleline(&mut self.token).password(true).desired_width(260.0));
                    if ui.button("生成").clicked() {
                        let bytes: [u8; 16] = rand::random();
                        self.token = bytes.iter().map(|b| format!("{b:02x}")).collect();
                    }
                    if ui.button("クリア").clicked() {
                        self.token.clear();
                    }
                });
                ui.label("トークンを設定すると、クライアントはヘッダー Authorization: Bearer <token> を送る必要があります。");
                if !self.token.trim().is_empty() {
                    ui.label(format!("クライアント側: client.py --token {}", self.token.trim()));
                }

                ui.add_space(10.0);
                ui.heading("オーバーレイ");
                ui.checkbox(&mut self.overlay_enabled, "操作中オーバーレイ（発光フレーム＋トースト）を表示する");
                egui::ComboBox::from_label("トーストの言語")
                    .selected_text(["おまかせ（OS の言語）", "日本語", "English"][self.overlay_lang])
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.overlay_lang, 0, "おまかせ（OS の言語）");
                        ui.selectable_value(&mut self.overlay_lang, 1, "日本語");
                        ui.selectable_value(&mut self.overlay_lang, 2, "English");
                    });
                ui.horizontal(|ui| {
                    ui.label("トースト題（操作中）:");
                    ui.text_edit_singleline(&mut self.overlay_title);
                });
                ui.horizontal(|ui| {
                    ui.label("トースト副題（操作中）:");
                    ui.text_edit_singleline(&mut self.overlay_sub);
                });
                ui.label("題・副題は空欄なら言語プリセットが使われます。");
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("保存").clicked() {
                    self.save();
                }
                if ui.button("閉じる").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ui.label(&self.saved_note);
            });
        });
    }
}
