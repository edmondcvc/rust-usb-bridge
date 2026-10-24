use crate::{
    backend::{is_laser, ComputerInvitation, LocalRequest, LocalResponse, Snapshot},
    config::ClientConfig,
    ipc,
    presentation::{connection_request, device_state, DeviceState, PreparedConnection},
};
use anyhow::{bail, Context, Result};
use crate::enrollment;
use zeroize::Zeroize;
use eframe::egui::{self, Color32, RichText};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

const INK: Color32 = Color32::from_rgb(25, 43, 58);
const MUTED: Color32 = Color32::from_rgb(81, 101, 117);
const BLUE: Color32 = Color32::from_rgb(24, 88, 153);
const GREEN: Color32 = Color32::from_rgb(20, 110, 77);
const AMBER: Color32 = Color32::from_rgb(147, 83, 12);
const RED: Color32 = Color32::from_rgb(166, 43, 43);
struct UsbApp {
    window_hook: Option<crate::window_lifecycle::WindowHook>,
    tray: Option<tray_icon::TrayIcon>,
    tray_actions: mpsc::Receiver<bool>,
    force_device: Option<bridge_common::UsbDevice>,
    help_textures: std::collections::HashMap<String, egui::TextureHandle>,
    demo: bool,
    username: String,
    password: String,
    ssh_port: String,
    enrollment: Option<enrollment::Job>,
    confirm_host: Option<(String,String)>,
    pending_pair: bool,
    replace_host: bool,
    confirm_replace: bool,
    invitation_pending: Option<ComputerInvitation>,
    error_details: String,
    requests: mpsc::Sender<LocalRequest>,
    responses: mpsc::Receiver<Result<LocalResponse>>,
    snapshot: Option<Snapshot>,
    busy: bool,
    polling: bool,
    backend_ok: bool,
    last_snapshot: Option<Instant>,
    last_poll: Instant,
    message: String,
    failed: bool,
    success_message: String,
    host: String,
    port: String,
    server_name: String,
    name: String,
    code: String,
    pair_open: bool,
    initialized: bool,
    pending_busid: Option<String>,
    pending_connection: Option<PreparedConnection>,
    pending_logs: Option<PathBuf>,
    confirm_release: Option<(String, String)>,
    help_open: bool,
    invitation_text: String,
    invitation_open: bool,
    confirm_switch: bool,
    exit_after_release: bool,
    release_all_pending: bool,
    exit_ready: bool,
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(218, 226, 233)))
        .corner_radius(10)
        .inner_margin(20)
}
fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).strong());
        });
}
fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(BLUE)
        .min_size(egui::vec2(130.0, 38.0))
}
fn state_color(state: DeviceState) -> Color32 {
    match state {
        DeviceState::Mine | DeviceState::Available | DeviceState::Unshared => GREEN,
        DeviceState::Occupied => AMBER,
        DeviceState::Failed => RED,
        DeviceState::Connecting => BLUE,
        _ => MUTED,
    }
}

impl UsbApp {
    fn new(context: &eframe::CreationContext<'_>, target: ipc::Target, demo: bool) -> Self {
        install_chinese_fonts(&context.egui_ctx);
        let mut style = (*context.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.override_text_color = Some(INK);
        style.visuals.panel_fill = Color32::from_rgb(242, 246, 249);
        style.visuals.extreme_bg_color = Color32::from_rgb(238, 243, 247);
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 9.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(26.0));
        context.egui_ctx.set_style(style);
        let (tx, rx) = mpsc::channel::<LocalRequest>();
        let (reply_tx, reply_rx) = mpsc::channel();
        let repaint = context.egui_ctx.clone();
        std::thread::spawn(move || {
            if demo { return; }
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(value) => value,
                Err(error) => {
                    let _ = reply_tx.send(Err(error.into()));
                    repaint.request_repaint();
                    return;
                }
            };
            while let Ok(request) = rx.recv() {
                let result = runtime.block_on(ipc::request_to(target, &request));
                if reply_tx.send(result).is_err() {
                    break;
                }
                repaint.request_repaint();
            }
        });
        let defaults = ClientConfig::default();
        let (tray_tx, tray_actions) = mpsc::channel();
        let tray = create_tray(context, tray_tx);
        let tray_error = tray.as_ref().err().map(|e| format!("系统托盘初始化失败：{e}。关闭按钮将保留退出确认。"));
        Self {
            window_hook: None,
            tray: tray.ok(), tray_actions,
            force_device: None,
            help_textures: Default::default(), demo,
            username: "operator".into(), password: String::new(), ssh_port: "22".into(),
            enrollment: None, confirm_host: None, pending_pair: false, replace_host: false, confirm_replace: false, invitation_pending: None, error_details: String::new(),
            requests: tx,
            responses: reply_rx,
            snapshot: None,
            busy: false,
            polling: false,
            backend_ok: false,
            last_snapshot: None,
            last_poll: Instant::now() - Duration::from_secs(10),
            message: tray_error.clone().unwrap_or_else(|| "正在检查 Windows 后台服务…".into()),
            failed: tray_error.is_some(),
            success_message: String::new(),
            host: defaults.host,
            port: defaults.port.to_string(),
            server_name: defaults.server_name,
            name: defaults.display_name,
            code: String::new(),
            pair_open: false,
            initialized: false,
            pending_busid: None,
            pending_connection: None,
            pending_logs: None,
            confirm_release: None,
            help_open: false,
            invitation_text: String::new(), invitation_open: false,
            confirm_switch: false, exit_after_release: false,
            release_all_pending: false, exit_ready: false,
        }
    }
    fn live(&self) -> bool {
        self.backend_ok
            && self
                .last_snapshot
                .is_some_and(|t| t.elapsed() < Duration::from_secs(12))
            && self.snapshot.as_ref().is_some_and(|s| {
                s.paired
                    && !s.config.host_disconnected
                    && s.network_error.is_none()
                    && !s.server_id.is_empty()
                    && s.devices_checked_at_ms.is_none_or(|ms| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                            .saturating_sub(ms as u128)
                            < 20_000
                    })
            })
    }
    fn send(&mut self, request: LocalRequest) {
        if self.demo {
            match request {
                LocalRequest::Snapshot => { self.last_snapshot=Some(Instant::now()); },
                LocalRequest::Refresh | LocalRequest::ConnectHost => self.demo_scene("空闲"),
                LocalRequest::DisconnectHost => { let exit=self.exit_after_release; self.demo_scene("主动断开"); self.exit_ready=exit; },
                LocalRequest::Connect {..} => self.demo_scene("本机使用"),
                LocalRequest::Release {..} | LocalRequest::ReleaseAll => { let exit=self.exit_after_release; self.demo_scene("空闲"); self.exit_ready=exit; },
                _ => { self.message="示例模式：此操作不会连接真实主机。".into(); }
            }
            return;
        }
        if self.busy || self.enrollment.is_some() {
            return;
        }
        self.pending_pair = matches!(request, LocalRequest::Pair { .. });
        self.polling = matches!(request, LocalRequest::Snapshot);
        self.release_all_pending = matches!(request, LocalRequest::ReleaseAll | LocalRequest::DisconnectHost);
        self.pending_busid = match &request {
            LocalRequest::Connect { busid } => Some(busid.clone()),
            LocalRequest::SetShared {
                busid,
                shared: true,
            } if self.pending_connection.is_some() => Some(busid.clone()),
            _ => None,
        };
        self.success_message = match &request {
            LocalRequest::Snapshot => self.success_message.clone(),
            LocalRequest::ConnectHost => "Linux 已连接。".into(),
            LocalRequest::DisconnectHost => "已断开 Linux，配对已保留。".into(),
            LocalRequest::Pair { .. } => "Linux 已连接。".into(),
            LocalRequest::SetEndpoint { .. } => "新地址已验证并保存，原电脑授权继续使用。".into(),
            LocalRequest::Connect { .. } => {
                "设备已连接。".into()
            }
            LocalRequest::Release { .. } => "设备已断开。".into(),
            LocalRequest::ReleaseAll => "已释放全部设备，可以在另一台已授权电脑上点击连接。".into(),
            LocalRequest::CreateInvitation => "邀请已生成，在新电脑导入后即可保存授权。".into(),
            LocalRequest::SetShared { shared: true, .. } => "设备已准备好，正在连接设备。".into(),
            LocalRequest::SetShared { shared: false, .. } => "设备已交还 Linux 本地使用。".into(),
            LocalRequest::Refresh => "设备状态已刷新。".into(),
            _ => "设置已保存。".into(),
        };
        if !self.polling {
            self.message = match &request {
                LocalRequest::Pair { .. } => "正在保存这台电脑并读取设备，请等待…",
                LocalRequest::SetShared { shared: true, .. } => {
                    "正在准备设备，完成后会自动连接设备，请等待…"
                }
                LocalRequest::SetShared { shared: false, .. } => "正在交还 Linux 本地使用，请等待…",
                LocalRequest::Release { .. } => "正在断开连接，请等待后台确认…",
                LocalRequest::Connect { .. } => "正在连接设备到这台电脑…",
                _ => "正在操作…",
            }
            .into();
            self.failed = false;
        }
        if self.requests.send(request).is_ok() {
            self.busy = true;
        } else {
            self.pending_connection = None;
            self.pending_busid = None;
            self.failed = true;
            self.backend_ok = false;
            self.message = "后台连接已结束，请重新打开客户端。".into();
        }
    }
    fn connect_device(&mut self, busid: &str) {
        if self.busy || !self.live() {
            return;
        }
        let request = self
            .snapshot
            .as_ref()
            .context("尚未取得设备列表")
            .and_then(|s| connection_request(s, busid));
        match request {
            Ok((request, pending)) => {
                self.pending_connection = pending;
                self.send(request);
            }
            Err(error) => {
                self.failed = true;
                self.message = format!("{error:#}");
            }
        }
    }
    fn receive(&mut self) {
        while let Ok(result) = self.responses.try_recv() {
            self.busy = false;
            self.pending_busid = None;
            match result {
                Ok(reply) => {
                    if self.release_all_pending {
                        if reply.ok && reply.snapshot.as_ref().is_some_and(|s| s.sessions.is_empty()) {
                            self.exit_ready = self.exit_after_release;
                        }
                        self.release_all_pending = false;
                    }
                    if let Some(invitation) = reply.invitation.clone() {
                        self.invitation_text = invitation;
                        self.invitation_open = true;
                    }
                    let next = self
                        .pending_connection
                        .take()
                        .map(|pending| pending.followup(&reply));
                    self.backend_ok = true;
                    if !reply.ok && self.pending_pair { self.error_details=reply.error.clone().unwrap_or_default(); }
                    if !self.polling || !reply.ok {
                        self.failed = !reply.ok;
                        self.message = if let Some(error)=&reply.error {
                            self.error_details=error.clone();
                            if self.pending_pair { "没能完成主机连接。请检查设备服务端口，重新输入密码后再试。原来的连接资料不会因此清空。".into() }
                            else if self.success_message.contains("邀请已生成") { "这台主机暂时不能生成邀请。请在新电脑直接填写主机地址和密码连接。".into() }
                            else { error.clone() }
                        } else { self.error_details.clear(); self.success_message.clone() };
                    }
                    if let Some(snapshot) = reply.snapshot {
                        self.last_snapshot = Some(Instant::now());
                        if !self.initialized {
                            self.host = snapshot.config.host.clone();
                            self.port = snapshot.config.port.to_string();
                            self.server_name = snapshot.config.server_name.clone();
                            self.name = snapshot.config.display_name.clone();
                            self.pair_open = !snapshot.paired;
                            self.initialized = true;
                            if self.polling {
                                self.message = if snapshot.config.host_disconnected {
                                    "已主动断开 Linux，配对已保留。再次使用时点击连接 Linux。"
                                } else if snapshot.paired {
                                    "已读取主机设置，正在检查 Linux 设备。"
                                } else {
                                    "首次使用：填写主机地址和密码，点击“连接主机”。"
                                }
                                .into();
                            }
                        }
                        if snapshot.paired {
                            self.code.clear();
                        }
                        if reply.ok && !self.polling && self.pending_pair {
                            self.pair_open = false; self.replace_host=false;
                        }
                        self.snapshot = Some(snapshot);
                    }
                    if let Some(log) = reply.log {
                        if let Some(path) = self.pending_logs.take() {
                            match std::fs::write(&path, log) {
                                Ok(()) => self.message = format!("日志已保存：{}", path.display()),
                                Err(error) => {
                                    self.failed = true;
                                    self.message = format!("保存日志失败：{error}");
                                }
                            }
                        }
                    } else if !reply.ok {
                        self.pending_logs = None;
                    }
                    if let Some(next) = next {
                        match next {
                            Ok(request) => self.send(request),
                            Err(error) => {
                                self.failed = true;
                                self.message = format!("连接未完成：{error:#}");
                            }
                        }
                    }
                }
                Err(error) => {
                    self.backend_ok = false;
                    self.failed = true;
                    self.message = format!("无法联系 Windows 后台服务。请运行安装程序修复，或在 Windows 服务中启动 UsbBridgeRust。详情：{error:#}");
                    self.pending_logs = None;
                    self.pending_connection = None;
                }
            }
        }
    }

    fn submit_invitation(&mut self, invite: ComputerInvitation) {
        self.enrollment = None;
        self.confirm_host = None;
        self.password.zeroize();
        self.send(LocalRequest::Pair { host: invite.host, port: invite.port,
            server_name: invite.server_name, name: self.name.trim().into(), ca_pem: invite.ca_pem, code: invite.code });
    }

    fn receive_enrollment(&mut self) {
        let mut events = Vec::new();
        if let Some(job) = &self.enrollment { while let Ok(event) = job.events.try_recv() { events.push(event); } }
        for event in events {
            match event {
                enrollment::Event::Released => {
                    self.enrollment = None; self.confirm_host = None; self.force_device = None;
                    self.failed = false; self.message = "Linux 已确认原会话释放。设备不会自动连接到本机，请刷新后选择连接设备。".into();
                    self.send(LocalRequest::Refresh);
                }
                enrollment::Event::Progress { message } => { self.message=message; self.failed=false; },
                enrollment::Event::Confirm { host, fingerprint } => self.confirm_host=Some((host, fingerprint)),
                enrollment::Event::Error { kind, message } => {
                    self.enrollment=None; self.confirm_host=None; self.failed=true;
                    self.message=message; self.error_details=format!("连接准备：{kind}");
                },
                enrollment::Event::Complete { invitation } => {
                    // Validate the helper's result through the same rules as an imported invitation.
                    match serde_json::to_string(&invitation).map_err(anyhow::Error::from).and_then(|s| ComputerInvitation::parse(&s)) {
                        Ok(invite) => self.submit_invitation(invite),
                        Err(_) => { self.enrollment=None; self.confirm_host=None; self.failed=true; self.message="主机返回的连接信息不完整，请重试。".into(); }
                    }
                }
            }
        }
    }

    fn pairing_panel(&mut self, ui: &mut egui::Ui) {
        let paired = self.snapshot.as_ref().is_some_and(|s| s.paired);
        let active = self.enrollment.is_some() || (self.busy && !self.polling);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width().min(560.0));
            ui.heading(if paired && !self.replace_host { "连接设置" } else { "连接 Linux" });
            ui.add_space(8.0);
            ui.add_enabled_ui(!active, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized([64.0, 28.0], egui::Label::new("地址"));
                    ui.add_sized([320.0, 28.0], egui::TextEdit::singleline(&mut self.host).hint_text("Linux 的 IP 地址"));
                });
                if !paired || self.replace_host {
                    ui.horizontal(|ui| {
                        ui.add_sized([64.0, 28.0], egui::Label::new("用户名"));
                        ui.add_sized([320.0, 28.0], egui::TextEdit::singleline(&mut self.username));
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([64.0, 28.0], egui::Label::new("密码"));
                        ui.add_sized([320.0, 28.0], egui::TextEdit::singleline(&mut self.password).password(true));
                    });
                }
                ui.add_space(8.0);
                egui::CollapsingHeader::new("高级设置").show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("登录端口"); ui.add(egui::TextEdit::singleline(&mut self.ssh_port).desired_width(70.0));
                        ui.label("USB 端口"); ui.add(egui::TextEdit::singleline(&mut self.port).desired_width(70.0));
                    });
                    if !paired || self.replace_host {
                        ui.horizontal(|ui| { ui.label("本机名称"); ui.text_edit_singleline(&mut self.name); });
                        if ui.button("导入电脑邀请").clicked() {
                            if let Some(path) = rfd::FileDialog::new().add_filter("电脑邀请", &["json"]).pick_file() {
                                let result = std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|s| ComputerInvitation::parse(&s));
                                match result { Ok(invite) => self.invitation_pending = Some(invite), Err(e) => { self.failed = true; self.message = e.to_string(); } }
                            }
                        }
                        if let Some(invite) = &self.invitation_pending { ui.label(format!("邀请主机：{}", invite.host)); }
                        if self.invitation_pending.is_some() && ui.add_enabled(self.backend_ok && !self.busy, egui::Button::new("使用邀请连接")).clicked() {
                            if let Some(invite) = self.invitation_pending.take() { self.submit_invitation(invite); }
                        }
                    }
                });
            });
            ui.add_space(12.0);
            if active {
                ui.horizontal_wrapped(|ui| { ui.spinner(); ui.label(&self.message); });
                if self.enrollment.is_some() && ui.button("取消").clicked() {
                    if let Some(job) = &self.enrollment { let _ = job.actions.send(enrollment::Action::Cancel); }
                    self.confirm_host = None;
                }
            } else if paired && !self.replace_host {
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!self.busy, primary("保存地址")).clicked() {
                        match self.port.trim().parse::<u16>() {
                            Ok(port) if port > 0 => self.send(LocalRequest::SetEndpoint { host: self.host.trim().into(), port }),
                            _ => { self.failed = true; self.message = "端口请填写 1–65535 的数字。".into(); }
                        }
                    }
                    if ui.add_enabled(!self.busy, egui::Button::new("更换主机")).clicked() { self.confirm_replace = true; }
                    if ui.button("收起").clicked() { self.pair_open = false; }
                });
            } else {
                let ready = !self.demo && self.backend_ok && !self.busy && self.enrollment.is_none() && !self.host.trim().is_empty() && !self.name.trim().is_empty() && !self.password.is_empty();
                if ui.add_enabled(ready, primary("连接 Linux")).clicked() {
                    let result = (|| -> Result<enrollment::Job> {
                        let input = enrollment::Input { force_release: None, host: self.host.trim().into(), username: self.username.trim().into(), password: std::mem::take(&mut self.password),
                            ssh_port: self.ssh_port.trim().parse().context("登录端口无效")?, port: self.port.trim().parse().context("USB 端口无效")? };
                        let script = std::env::current_exe()?.parent().context("找不到软件目录")?.join("scripts/connect-host.ps1");
                        enrollment::start(&script, input)
                    })();
                    match result { Ok(job) => { self.enrollment = Some(job); self.message = "正在连接 Linux…".into(); self.failed = false; self.error_details.clear(); }, Err(e) => { self.message = e.to_string(); self.failed = true; } }
                }
            }
        });
    }
    fn overview(&mut self, ui: &mut egui::Ui) {
        let Some(snapshot) = self.snapshot.clone() else { ui.spinner(); return; };
        if !snapshot.paired { return; }
        let live = self.live();
        let disconnected = snapshot.config.host_disconnected;
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&snapshot.config.server_name).size(21.0).strong());
                ui.colored_label(MUTED, &snapshot.config.host);
                badge(ui, if disconnected { "已断开" } else if live { "已连接" } else { "状态未知" }, if live { GREEN } else { AMBER });
                if ui.add_enabled(self.backend_ok && !self.busy && self.enrollment.is_none(), primary(if disconnected { "连接 Linux" } else { "断开 Linux" })).clicked() {
                    if disconnected { self.send(LocalRequest::ConnectHost); }
                    else { self.confirm_switch = true; self.exit_after_release = false; }
                }
            });
            if !disconnected && !live {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(AMBER, if self.backend_ok { "暂时无法确认设备状态" } else { "Windows 后台未连接" });
                    if ui.add_enabled(self.backend_ok && !self.busy && self.enrollment.is_none(), egui::Button::new("重试")).clicked() { self.send(LocalRequest::Refresh); }
                });
            }
            if snapshot.driver_error.is_some() {
                ui.colored_label(RED, "USB 驱动未就绪，请通过安装程序修复。");
            }
        });
    }

    fn devices_panel(&mut self, ui: &mut egui::Ui) {
        let Some(snapshot) = self.snapshot.clone() else { return; };
        if !snapshot.paired { return; }
        let live = self.live();
        let disconnected = snapshot.config.host_disconnected;
        let mut devices: Vec<_> = snapshot.devices.iter().collect();
        devices.sort_by_key(|d| (!is_laser(d), d.busid.clone()));
        if devices.is_empty() {
            ui.add_space(30.0);
            ui.colored_label(MUTED, if disconnected { "连接 Linux 后查看设备" } else if live { "没有 USB 设备" } else { "等待 Linux 连接" });
            return;
        }
        ui.add_space(8.0);
        ui.label(RichText::new("USB 设备").size(19.0).strong());
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for device in devices {
                let session = snapshot.sessions.iter().find(|s| s.busid == device.busid);
                let own = session.is_some_and(|s| s.port.is_some() && s.state == "connected");
                let state = device_state(device, session, live, self.pending_busid.as_deref() == Some(&device.busid));
                let kind = if is_laser(device) { "雕刻机接口" } else { device.device_type.as_deref().unwrap_or("USB 设备") };
                let status = if disconnected { "未连接主机".into() } else { match state {
                    DeviceState::Mine => "本机使用中".into(),
                    DeviceState::Occupied => format!("{} 使用中", device.owner.as_deref().unwrap_or("其他电脑")),
                    DeviceState::Available | DeviceState::Unshared => "空闲".into(),
                    DeviceState::Offline => "未发现设备".into(),
                    _ => state.label().into(),
                }};
                ui.horizontal_wrapped(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() * 0.35).min(280.0));
                        ui.label(RichText::new(&device.name).strong());
                        ui.small(kind);
                    });
                    ui.colored_label(state_color(state), status);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("更多", |ui| {
                            if state == DeviceState::Occupied {
                                if device.session_id.is_some() {
                                    if ui.add_enabled(!self.busy && self.enrollment.is_none(), egui::Button::new("管理员强制断开")).clicked() {
                                        self.force_device = Some(device.clone()); self.password.zeroize(); self.failed = false; ui.close_menu();
                                    }
                                } else { ui.label("强制断开需要升级 Linux 服务"); }
                                ui.separator();
                            }
                            let mut enabled = snapshot.config.auto_connect.contains(&device.busid);
                            if !is_laser(device) && device.protected_reason.is_none() {
                                if ui.add_enabled(!self.busy && self.enrollment.is_none() && live, egui::Checkbox::new(&mut enabled, "开机自动连接")).changed() {
                                    self.send(LocalRequest::SetAutoConnect { busid: device.busid.clone(), enabled });
                                }
                            }
                            if device.shared && !own && live && device.owner.is_none() && device.present && device.protected_reason.is_none() {
                                if ui.add_enabled(!self.busy, egui::Button::new("交还 Linux 使用")).clicked() {
                                    self.send(LocalRequest::SetShared { busid: device.busid.clone(), shared: false }); ui.close_menu();
                                }
                            }
                            ui.separator();
                            ui.small(format!("端口 {} · {:04x}:{:04x}", device.busid, device.vendor, device.product));
                            if let Some(reason) = &device.protected_reason { ui.label(reason); }
                            if let Some(error) = session.and_then(|s| s.error.as_deref()) { ui.label(error); }
                        });
                        if own {
                            if ui.add_enabled(self.backend_ok && !self.busy && self.enrollment.is_none(), egui::Button::new("断开")).clicked() {
                                self.confirm_release = Some((device.busid.clone(), device.name.clone()));
                            }
                        } else if state != DeviceState::Occupied {
                            let usable = live && device.present && device.protected_reason.is_none() && device.owner.is_none() && state != DeviceState::Connecting && snapshot.driver_error.is_none();
                            if ui.add_enabled(usable && !self.busy && self.enrollment.is_none(), egui::Button::new("连接")).clicked() { self.connect_device(&device.busid); }
                        }
                    });
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
            }
        });
    }
    fn help(&mut self, ui: &mut egui::Ui) {
        if ui.button("打开完整图文教程").clicked() {
            let result=(|| -> Result<()> {
                let path=std::env::current_exe()?.parent().context("找不到安装目录")?.join("docs/tutorial/index.html");
                if !path.is_file() { bail!("缺少图文教程，请用新版安装包修复。"); }
                std::process::Command::new("explorer.exe").arg(path).spawn()?; Ok(())
            })();
            if let Err(e)=result { self.message=e.to_string(); self.failed=true; }
        }
        let steps: Vec<TutorialStep>=serde_json::from_str(include_str!("../../../docs/tutorial/steps.json")).unwrap_or_default();
        ui.small("截图中的设备和电脑名称均为示例；操作按钮与本软件一致。");
        for (i,step) in steps.iter().enumerate() {
            egui::CollapsingHeader::new(format!("{}  {}",i+1,step.title)).default_open(i==0).show(ui, |ui| {
                for text in &step.steps { ui.label(text); }
                if !step.image.is_empty() && !self.help_textures.contains_key(&step.image) {
                    if let Some(path)=std::env::current_exe().ok().and_then(|p| p.parent().map(|d|d.join("docs/tutorial").join(&step.image))) {
                        if let Ok(img)=image::open(path) {
                            let rgba=img.to_rgba8(); let size=[rgba.width() as usize,rgba.height() as usize];
                            let color=egui::ColorImage::from_rgba_unmultiplied(size,rgba.as_raw());
                            self.help_textures.insert(step.image.clone(),ui.ctx().load_texture(&step.image,color,Default::default()));
                        }
                    }
                }
                if let Some(texture)=self.help_textures.get(&step.image) { ui.add(egui::Image::new(texture).max_width(ui.available_width()).maintain_aspect_ratio(true)); }
                else if !step.image.is_empty() { ui.small("图片未安装完整；以上文字步骤仍可使用。请使用完整安装包修复教程。"); }
                ui.colored_label(MUTED,&step.note);
            });
        }
    }

    fn demo_scene(&mut self, scene: &str) {
        use crate::backend::SessionStatus;
        self.backend_ok=true; self.initialized=true; self.busy=false; self.polling=false;
        self.last_snapshot=Some(Instant::now()); self.failed=false; self.pending_busid=None;
        self.host="192.0.2.10".into(); self.name="我的笔记本".into(); self.password.zeroize(); self.replace_host=false;
        self.pair_open=scene=="首次连接" || scene=="密码错误";
        let config=ClientConfig { host_disconnected: scene=="主动断开", client_id: if self.pair_open { None } else { Some("demo-client".into()) }, display_name:self.name.clone(),..Default::default() };
        let device=bridge_common::UsbDevice { busid:"1-2".into(),name:"USB 摄像头（示例）".into(),vendor:0x1234,product:0x5678,serial:Some("DEMO".into()),path:"1-2".into(),speed:3,shared:true,owner:if scene=="别人使用" {Some("办公室电脑".into())} else if scene=="本机使用" {Some(self.name.clone())}else{None},protected_reason:None,present:true,device_type:Some("摄像头".into()),session_id:Some("demo-session".into()) };
        self.snapshot=Some(Snapshot { config,paired:!self.pair_open,server_id:"demo-server".into(),devices:if self.pair_open || scene=="没有设备" {vec![]}else{vec![device]},sessions:if scene=="本机使用" {vec![SessionStatus{busid:"1-2".into(),port:Some(1),state:"connected".into(),error:None}]}else{vec![]},driver_error:None,network_error:if scene=="网络中断" {Some("无法联系 Linux".into())} else {None},proxy_port:0,devices_checked_at_ms:None });
        self.message=if scene=="密码错误" {self.failed=true; "用户名或密码不对。请重新输入主机的用户名和密码。"} else {"示例模式：这里的设备和电脑名称仅用于演示，不会连接真实设备。"}.into();
    }

}

#[derive(serde::Deserialize)]
struct TutorialStep { title:String, image:String, steps:Vec<String>, note:String }

impl eframe::App for UsbApp {
    fn update(&mut self, context: &egui::Context, _: &mut eframe::Frame) {
        self.receive();
        self.receive_enrollment();
        while let Ok(exit) = self.tray_actions.try_recv() {
            context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            context.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            if exit { self.confirm_switch = true; self.exit_after_release = true; }
        }
        if context.input(|i| i.viewport().close_requested()) && !self.exit_ready {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.tray.is_some() {
                context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                self.confirm_switch = true; self.exit_after_release = true;
            }
        }
        if self.exit_ready { self.enrollment=None; self.password.zeroize(); }
        if self.exit_ready {
            if let Some(hook) = &self.window_hook { hook.allow_exit(); }
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.enrollment.is_none() && !self.busy && self.last_poll.elapsed() >= Duration::from_secs(2) {
            self.last_poll = Instant::now();
            self.send(LocalRequest::Snapshot);
        }
        context.request_repaint_after(Duration::from_millis(300));
        if self.demo {
            egui::TopBottomPanel::top("demo_controls").show(context, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(AMBER,"示例 · 隔离演示");
                    for scene in ["首次连接","空闲","本机使用","别人使用","没有设备","密码错误","主动断开","网络中断"] { if ui.button(scene).clicked() { self.demo_scene(scene); } }
                    for scale in [1.0,1.25,1.5] { if ui.button(format!("{}%",(scale*100.0) as u32)).clicked() { context.set_zoom_factor(scale); } }
                });
            });
        }
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(egui::Margin::symmetric(24, 16)))
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("USB 连接中心").size(24.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("帮助").clicked() { self.help_open = true; }
                        ui.menu_button("设置", |ui| {
                            if ui.button("Linux 连接设置").clicked() { self.pair_open = !self.pair_open; ui.close_menu(); }
                            if ui.add_enabled(!self.busy && self.enrollment.is_none() && self.snapshot.as_ref().is_some_and(|s| s.paired && !s.config.host_disconnected), egui::Button::new("刷新设备")).clicked() { self.send(LocalRequest::Refresh); ui.close_menu(); }
                            if ui.add_enabled(!self.busy && self.enrollment.is_none() && self.live(), egui::Button::new("添加其他电脑")).clicked() { self.send(LocalRequest::CreateInvitation); ui.close_menu(); }
                            ui.separator();
                            if ui.add_enabled(!self.demo, egui::Button::new("升级 Linux 服务")).clicked() {
                                let result = (|| -> Result<()> {
                                    let root = std::env::current_exe()?.parent().context("找不到客户端目录")?.to_path_buf();
                                    let script = root.join("scripts/update-linux-from-windows.ps1");
                                    if !script.is_file() { bail!("请使用完整安装包升级 Linux"); }
                                    std::process::Command::new("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
                                        .args(["-NoProfile", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"])
                                        .arg(script).arg("-LinuxHost").arg(&self.host).arg("-UserName").arg(&self.username).arg("-SshPort").arg(&self.ssh_port).spawn()?;
                                    Ok(())
                                })();
                                if let Err(e) = result { self.failed = true; self.message = format!("{e:#}"); }
                                ui.close_menu();
                            }
                            if ui.add_enabled(!self.busy && self.enrollment.is_none() && self.backend_ok, egui::Button::new("导出日志")).clicked() {
                                if let Some(path) = rfd::FileDialog::new().set_file_name("USB 转发日志.txt").save_file() { self.pending_logs = Some(path); self.send(LocalRequest::Logs); }
                                ui.close_menu();
                            }
                        });
                    });
                });
            });        if self.demo || self.failed || self.enrollment.is_some() || (self.busy && !self.polling) || !self.success_message.is_empty() {
        egui::TopBottomPanel::bottom("footer")
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::symmetric(24, 12)),
            )
            .show(context, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if self.busy && !self.polling {
                        ui.spinner();
                    }
                    ui.colored_label(if self.failed { RED } else { BLUE }, &self.message);
                });
                if self.failed && !self.error_details.is_empty() { egui::CollapsingHeader::new("查看详情").show(ui, |ui| { ui.label(&self.error_details); }); }

            });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(242, 246, 249))
                    .inner_margin(24),
            )
            .show(context, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !self
                            .snapshot
                            .as_ref()
                            .is_some_and(|s| !s.paired && self.backend_ok)
                        {
                            self.overview(ui);
                            ui.add_space(12.0);
                        }
                        if self.pair_open || self.snapshot.as_ref().is_some_and(|s| !s.paired) {
                            self.pairing_panel(ui);
                            ui.add_space(12.0);
                        }
                        self.devices_panel(ui);
                    });
            });
        if self.confirm_replace {
            egui::Modal::new(egui::Id::new("replace_host")).show(context, |ui| {
                ui.set_max_width(440.0); ui.heading("更换插着 USB 的主机？");
                ui.label("请先断开本机所有设备。新主机连接成功后，本软件改用新主机；失败则保留原来的连接资料。");
                if ui.button("返回").clicked() { self.confirm_replace=false; }
                let idle=self.snapshot.as_ref().is_some_and(|s| s.sessions.iter().all(|x| x.port.is_none() && x.state!="connecting"));
                if ui.add_enabled(idle && !self.busy,primary("继续更换主机")).clicked() { self.confirm_replace=false; self.replace_host=true; self.password.zeroize(); self.invitation_pending=None; }
            });
        }
        if let Some((host,fingerprint))=self.confirm_host.clone() {
            egui::Modal::new(egui::Id::new("confirm_host")).show(context, |ui| {
                ui.set_max_width(440.0); ui.heading("确认你的主机");
                ui.label(format!("这是你第一次连接 {host}。请确认这是插着 USB 设备的那台主机。"));
                egui::CollapsingHeader::new("查看主机身份和核对方法").show(ui, |ui| {
                    ui.label(&fingerprint); ui.label("可与这台主机屏幕或可信 FinalShell 连接中的主机指纹核对。软件会记住它，发生变化时停止连接。");
                });
                ui.horizontal_wrapped(|ui| {
                    for (label,yes) in [("返回检查地址",false),("是我的主机，继续",true)] {
                        if ui.button(label).clicked() { if let Some(job)=&self.enrollment { let _=job.actions.send(enrollment::Action::Confirm(yes)); } self.confirm_host=None; }
                    }
                });
            });
        }
        if self.help_open {
            let mut open = true;
            let mut close = false;
            egui::Window::new("使用帮助")
                .id(egui::Id::new("usage_help"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(620.0)
                .max_width((context.screen_rect().width() - 60.0).max(280.0))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(context, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height((context.screen_rect().height() - 180.0).max(180.0))
                        .show(ui, |ui| {
                            self.help(ui);
                            ui.add_space(8.0);
                            close = ui.button("知道了，关闭帮助").clicked();
                        });
                });
            self.help_open = open && !close;
        }
        if self.invitation_open {
            let mut open = true;
            egui::Window::new("添加我的电脑").open(&mut open).collapsible(false).default_width(620.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show(context, |ui| {
                    ui.label("1  保存邀请文件或复制邀请，到你自己的新电脑。");
                    ui.label("2  新电脑展开“从另一台电脑导入”，选择文件，再点击“添加这台电脑”。");
                    ui.label("邀请 15 分钟内有效，只能使用一次；授权完成后，这台电脑以后不用再输入密码。");
                    ui.horizontal(|ui| {
                        if ui.button("复制邀请").clicked() { ui.ctx().copy_text(self.invitation_text.clone()); }
                        if ui.button("保存邀请文件").clicked() {
                            if let Some(path) = rfd::FileDialog::new().set_file_name("我的电脑授权邀请.json").save_file() {
                                if let Err(e) = std::fs::write(path, &self.invitation_text) { self.message = format!("保存邀请失败：{e}"); self.failed = true; }
                            }
                        }
                    });
                    ui.small("邀请只发给自己的电脑；每台电脑单独授权，连接验证由软件自动处理。");
                });
            self.invitation_open = open;
        }
        if let Some(device) = self.force_device.clone().filter(|_| self.confirm_host.is_none()) {
            egui::Modal::new(egui::Id::new("force_release")).show(context, |ui| {
                ui.set_max_width(450.0);
                ui.heading("管理员强制断开 USB");
                ui.label(format!("设备：{}\n正在使用：{}", device.name, device.owner.as_deref().unwrap_or("未知")));
                ui.colored_label(RED, "此操作会中断对方使用，可能造成未保存数据丢失。雕刻机物理状态请现场确认。");
                ui.label("Linux 管理员用户名"); ui.text_edit_singleline(&mut self.username);
                ui.label("Linux 管理密码（仅本次使用，不保存）");
                ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                if self.enrollment.is_some() { ui.spinner(); ui.label(&self.message); }
                if self.failed { ui.colored_label(RED, &self.message); }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("取消").clicked() {
                        if self.enrollment.is_some() { self.message = "已停止等待管理员操作。若请求已提交，断开仍可能完成，请刷新设备状态确认。".into(); }
                        self.enrollment = None; self.force_device = None; self.confirm_host = None; self.password.zeroize();
                    }
                    if ui.add_enabled(!self.demo && self.live() && !self.busy && self.enrollment.is_none() && !self.password.is_empty(), primary("确认中断并验证密码")).clicked() {
                        let result = (|| -> Result<enrollment::Job> {
                            let snapshot = self.snapshot.as_ref().context("无法读取主机状态")?;
                            let request = bridge_common::ForceReleaseRequest {
                                server_id: snapshot.server_id.clone(), busid: device.busid.clone(),
                                session_id: device.session_id.clone().context("请升级 Linux 服务端")?,
                                path: device.path.clone(), vendor: device.vendor, product: device.product, serial: device.serial.clone(),
                            };
                            let input = enrollment::Input {
                                force_release: Some(request), host: snapshot.config.host.clone(), username: self.username.trim().into(),
                                password: std::mem::take(&mut self.password), ssh_port: self.ssh_port.parse().context("登录端口无效")?, port: snapshot.config.port,
                            };
                            let script = std::env::current_exe()?.parent().context("找不到软件目录")?.join("scripts/connect-host.ps1");
                            enrollment::start(&script, input)
                        })();
                        match result { Ok(job) => { self.enrollment = Some(job); self.failed = false; self.message = "正在验证 Linux 管理员身份…".into(); }, Err(e) => { self.failed = true; self.message = e.to_string(); } }
                    }
                });
            });
        }
        if self.confirm_switch {
            egui::Modal::new(egui::Id::new("switch_computer")).show(context, |ui| {
                ui.set_width(500.0);
                ui.heading(if self.exit_after_release { "退出客户端" } else { "断开 Linux" });
                ui.label("将断开本机全部 USB 并停止连接 Linux，保留配对；其他电脑不受影响。请先结束操作，U 盘先安全弹出。只关闭窗口仍保持连接。");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("返回，继续使用").clicked() { self.confirm_switch = false; self.exit_after_release = false; }
                    if self.exit_after_release && ui.add_enabled(!self.busy, egui::Button::new("保持连接，退出客户端")).clicked() { self.exit_ready = true; self.confirm_switch = false; }
                    if ui.add_enabled(!self.busy && self.enrollment.is_none() && self.backend_ok, primary(if self.exit_after_release { "断开并退出" } else { "已结束操作，断开 Linux" })).clicked() {
                        self.confirm_switch = false; self.send(LocalRequest::DisconnectHost);
                    }
                });
            });
        }
        if let Some((busid, name)) = self.confirm_release.clone() {
            egui::Modal::new(egui::Id::new("release_confirmation")).show(context, |ui| {
                ui.set_width(440.0);
                ui.heading("准备断开连接"); ui.label(format!("设备：{name}"));
                ui.label("先结束设备操作并关闭应用。U 盘请先在 Windows 安全弹出。释放后其他电脑可以使用。");
                ui.horizontal(|ui| {
                    if ui.button("继续使用，返回").clicked() { self.confirm_release = None; }
                    if ui.add_enabled(!self.busy && self.enrollment.is_none() && self.backend_ok, primary("已结束操作，断开连接")).clicked() {
                        self.confirm_release = None; self.send(LocalRequest::Release { busid });
                    }
                });
            });
        }
    }
}

fn restore_native_window(hwnd: isize, context: &egui::Context) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
    // Hidden Windows windows do not necessarily receive WM_PAINT. Wake the native
    // window inside the tray callback before relying on another egui update.
    // This HWND belongs to the root window; callbacks run on its message thread
    // and are removed in UsbApp::drop before eframe destroys the window.
    unsafe {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
        SetForegroundWindow(hwnd);
    }
    context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    context.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    context.send_viewport_cmd(egui::ViewportCommand::Focus);
    context.request_repaint();
}

#[cfg(test)]
mod tray_tests {
    use super::{create_tray_for_window, restore_native_window};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, EnumThreadWindows, GetClassNameW, IsIconic,
        IsWindowVisible, SendMessageW, ShowWindow, WM_LBUTTONUP, SW_HIDE, SW_MINIMIZE,
        WS_OVERLAPPEDWINDOW,
    };

    #[test]
    fn tray_restore_wakes_hidden_window_without_an_egui_frame() {
        // A private test HWND: no service, USB, or installed GUI is involved.
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let title: Vec<u16> = "USB tray regression\0".encode_utf16().collect();
        unsafe {
            let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW,
                0, 0, 200, 100, std::ptr::null_mut(), std::ptr::null_mut(),
                std::ptr::null_mut(), std::ptr::null());
            assert!(!hwnd.is_null());
            struct WindowGuard(windows_sys::Win32::Foundation::HWND);
            impl Drop for WindowGuard {
                fn drop(&mut self) { unsafe { DestroyWindow(self.0); } }
            }
            let _window = WindowGuard(hwnd);
            let context = eframe::egui::Context::default();
            for _ in 0..3 {
                ShowWindow(hwnd, SW_HIDE);
                assert_eq!(IsWindowVisible(hwnd), 0);
                restore_native_window(hwnd as isize, &context);
                assert_ne!(IsWindowVisible(hwnd), 0, "Must wake before the next egui frame");
                ShowWindow(hwnd, SW_MINIMIZE);
                assert_ne!(IsIconic(hwnd), 0);
                restore_native_window(hwnd as isize, &context);
                assert_eq!(IsIconic(hwnd), 0);
            }
            // Exercise the actual tray-icon 0.19.3 Windows message -> event handler
            // -> restore path too. Enumerate ONLY this test's own thread windows.
            let (sender, receiver) = std::sync::mpsc::channel();
            let _tray = create_tray_for_window(&context, hwnd as isize, sender).unwrap();
            unsafe extern "system" fn find_tray(
                window: windows_sys::Win32::Foundation::HWND,
                found: isize,
            ) -> i32 {
                let mut class = [0u16; 64];
                let len = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
                if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == "tray_icon_app" {
                    *(found as *mut windows_sys::Win32::Foundation::HWND) = window;
                    return 0;
                }
                1
            }
            let mut tray_window: windows_sys::Win32::Foundation::HWND = std::ptr::null_mut();
            EnumThreadWindows(windows_sys::Win32::System::Threading::GetCurrentThreadId(),
                Some(find_tray), (&mut tray_window as *mut windows_sys::Win32::Foundation::HWND) as isize);
            assert!(!tray_window.is_null());
            for _ in 0..3 {
                ShowWindow(hwnd, SW_HIDE);
                // 6002 is WM_USER_TRAYICON in the pinned tray-icon dependency.
                SendMessageW(tray_window, 6002, 0, WM_LBUTTONUP as isize);
                assert_eq!(receiver.recv_timeout(std::time::Duration::from_secs(1)).unwrap(), false);
                assert_ne!(IsWindowVisible(hwnd), 0, "The real tray click callback must reveal the HWND");
            }
            tray_icon::TrayIconEvent::set_event_handler(None::<fn(tray_icon::TrayIconEvent)>);
            tray_icon::menu::MenuEvent::set_event_handler(None::<fn(tray_icon::menu::MenuEvent)>);
        }
    }
}

impl Drop for UsbApp {
    fn drop(&mut self) {
        tray_icon::TrayIconEvent::set_event_handler(None::<fn(tray_icon::TrayIconEvent)>);
        tray_icon::menu::MenuEvent::set_event_handler(None::<fn(tray_icon::menu::MenuEvent)>);
        self.window_hook.take();
        self.tray.take();
    }
}

fn create_tray(creation: &eframe::CreationContext<'_>, actions: mpsc::Sender<bool>) -> Result<tray_icon::TrayIcon> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let hwnd = match creation.window_handle()?.as_raw() {
        RawWindowHandle::Win32(handle) => handle.hwnd.get(),
        _ => bail!("无法取得 Windows 主窗口句柄"),
    };
    create_tray_for_window(&creation.egui_ctx, hwnd, actions)
}

fn create_tray_for_window(context: &egui::Context, hwnd: isize, actions: mpsc::Sender<bool>) -> Result<tray_icon::TrayIcon> {
    use tray_icon::{menu::{Menu, MenuEvent, MenuItem}, Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let menu = Menu::new();
    let show = MenuItem::new("打开 USB 连接中心", true, None);
    let exit = MenuItem::new("退出客户端…", true, None);
    menu.append_items(&[&show, &exit])?;
    // A high contrast USB trident, drawn at tray resolution without external assets.
    let mut pixels = vec![0u8; 32 * 32 * 4];
    for y in 0..32usize {
        for x in 0..32usize {
            let stem = (14..=17).contains(&x) && (5..=25).contains(&y);
            let arrow = (3..=9).contains(&y) && x.abs_diff(15) <= (y - 3) / 2;
            let branches = ((8..=15).contains(&x) && (17..=19).contains(&y))
                || ((16..=24).contains(&x) && (13..=15).contains(&y))
                || ((7..=9).contains(&x) && (11..=19).contains(&y))
                || ((22..=24).contains(&x) && (8..=15).contains(&y));
            let terminals = ((x as i32 - 15).pow(2) + (y as i32 - 26).pow(2) <= 12)
                || ((x as i32 - 8).pow(2) + (y as i32 - 10).pow(2) <= 8)
                || ((21..=25).contains(&x) && (5..=9).contains(&y));
            let color = if stem || arrow || branches || terminals { [255, 255, 255, 255] } else { [24, 88, 153, 255] };
            pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].copy_from_slice(&color);
        }
    }
    let tray = TrayIconBuilder::new()
        .with_tooltip("USB 连接中心 · 点击打开窗口")
        .with_icon(Icon::from_rgba(pixels, 32, 32)?)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    let repaint = context.clone();
    let click_actions = actions.clone();
    let tray_id = tray.id().clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click { id, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
            if id == tray_id {
                let _ = click_actions.send(false);
                restore_native_window(hwnd, &repaint);
            }
        }
    }));
    let repaint = context.clone();
    let show_id = show.id().clone();
    let exit_id = exit.id().clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == show_id || event.id == exit_id {
            let _ = actions.send(event.id == exit_id);
            restore_native_window(hwnd, &repaint);
        }
    }));
    Ok(tray)
}

fn install_chinese_fonts(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in [r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simhei.ttf"] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("chinese".into(), egui::FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "chinese".into());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("chinese".into());
            break;
        }
    }
    context.set_fonts(fonts);
}

pub fn run(target: ipc::Target, demo: bool) -> Result<()> {
    let Some(_instance) = crate::window_lifecycle::Instance::acquire(demo)? else { return Ok(()); };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([940.0, 640.0])
            .with_min_inner_size([760.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        if demo { "USB 连接中心 · 示例" } else if matches!(target, ipc::Target::Diagnostic(_)) {
            "USB 连接中心 · 诊断模式"
        } else {
            "USB 连接中心"
        },
        options,
        Box::new(move |context| { let mut app=UsbApp::new(context, target, demo);
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let RawWindowHandle::Win32(handle) = context.window_handle()?.as_raw() else { return Err("无法取得主窗口".into()); };
            app.window_hook = Some(crate::window_lifecycle::WindowHook::install(handle.hwnd.get(), demo, app.tray.is_some())?);
            if demo { app.demo_scene("首次连接"); } Ok(Box::new(app)) }),
    )
    .map_err(|e| anyhow::anyhow!("启动客户端窗口失败：{e}"))
}

