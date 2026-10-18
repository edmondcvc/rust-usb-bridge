use crate::{
    config::{self, ClientConfig},
    driver, network,
    proxy::{Endpoint, Proxy, ProxyEvent},
};
use anyhow::{bail, Context, Result};
use bridge_common::{ControlRequest, UsbDevice};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, watch, Mutex, RwLock};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LocalRequest {
    ConnectHost,
    DisconnectHost,
    Snapshot,
    Refresh,
    CreateInvitation,
    ReleaseAll,
    SetEndpoint { host: String, port: u16 },
    Pair {
        host: String,
        port: u16,
        server_name: String,
        name: String,
        ca_pem: String,
        code: String,
    },
    Connect {
        busid: String,
    },
    Release {
        busid: String,
    },
    SetShared {
        busid: String,
        shared: bool,
    },
    SetAutoConnect {
        busid: String,
        enabled: bool,
    },
    Logs,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionStatus {
    pub busid: String,
    pub port: Option<i32>,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub devices_checked_at_ms: Option<u64>,
    pub config: ClientConfig,
    pub paired: bool,
    pub server_id: String,
    pub devices: Vec<UsbDevice>,
    pub sessions: Vec<SessionStatus>,
    pub driver_error: Option<String>,
    pub network_error: Option<String>,
    pub proxy_port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalResponse {
    pub ok: bool,
    pub error: Option<String>,
    pub snapshot: Option<Snapshot>,
    pub log: Option<String>,
    #[serde(default)]
    pub invitation: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ComputerInvitation {
    pub version: u32,
    pub host: String,
    pub port: u16,
    pub server_name: String,
    pub ca_pem: String,
    pub code: String,
    pub expires_at: u64,
}

impl ComputerInvitation {
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > 32_000 { bail!("邀请内容过长"); }
        let invite: Self = serde_json::from_str(text.trim()).context("请粘贴完整的电脑邀请内容")?;
        if invite.version != 1 || invite.code.is_empty() || invite.code.len() > 128 { bail!("邀请格式不支持"); }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        if invite.expires_at <= now { bail!("邀请已过期，请在已授权电脑上重新生成"); }
        let mut config = ClientConfig::default();
        config.host = invite.host.clone(); config.port = invite.port; config.server_name = invite.server_name.clone();
        config.validate()?;
        network::certificates(invite.ca_pem.as_bytes())?;
        Ok(invite)
    }
}

impl LocalResponse {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            snapshot: None,
            log: None,
            invitation: None,
        }
    }
}

struct Session {
    id: u64,
    laser: bool,
    status: SessionStatus,
    cancellation: watch::Sender<bool>,
}
pub struct Backend {
    devices_checked_at_ms: AtomicU64,
    config_path: PathBuf,
    config: RwLock<ClientConfig>,
    devices: RwLock<Vec<UsbDevice>>,
    server_id: RwLock<String>,
    network_error: RwLock<Option<String>>,
    driver_error: RwLock<Option<String>>,
    sessions: Mutex<BTreeMap<String, Session>>,
    mutations: Mutex<()>,
    proxy: Proxy,
}

impl Backend {
    pub async fn start(config_path: PathBuf) -> Result<Arc<Self>> {
        let mut config = config::load(&config_path)?;
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let proxy = Proxy::start(config.proxy_port, event_tx).await?;
        // Initial allocation is saved before attach, then kept across restarts.
        if config.proxy_port == 0 {
            config.proxy_port = proxy.port;
            config::save(&config_path, &config)?;
        }
        let credentials_error = if config.client_id.is_some() {
            match network::paired_tls(&config_path) {
                Ok(tls) => {
                    proxy
                        .set_endpoint(Some(Endpoint {
                            config: config.clone(),
                            tls,
                        }))
                        .await;
                    None
                }
                Err(error) => Some(format!("配对证书无法读取，请重新配对：{error:#}")),
            }
        } else {
            None
        };
        let backend = Arc::new(Self {
            devices_checked_at_ms: AtomicU64::new(0),
            config_path,
            config: RwLock::new(config),
            devices: RwLock::new(Vec::new()),
            server_id: RwLock::new(String::new()),
            network_error: RwLock::new(credentials_error),
            driver_error: RwLock::new(None),
            sessions: Mutex::new(BTreeMap::new()),
            mutations: Mutex::new(()),
            proxy,
        });
        backend.cleanup_owned_orphans().await;
        let events = Arc::downgrade(&backend);
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let Some(backend) = events.upgrade() {
                    backend.proxy_ended(event).await;
                } else {
                    break;
                }
            }
        });
        let poll = Arc::downgrade(&backend);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                let Some(backend) = poll.upgrade() else {
                    break;
                };
                backend.refresh_driver().await;
                let _guard = backend.mutations.lock().await;
                if backend.config.read().await.client_id.is_some() && !backend.config.read().await.host_disconnected {
                    if let Err(error) = backend.refresh().await {
                        *backend.network_error.write().await = Some(format!("{error:#}"));
                    }
                }
            }
        });
        // Opt-in only; laser is excluded again at connect time. This performs
        // one startup attempt, never repeats old URBs or retries after a drop.
        let startup = backend.clone();
        tokio::spawn(async move {
            let _guard = startup.mutations.lock().await;
            if startup.config.read().await.host_disconnected { return; }
            if startup.config.read().await.client_id.is_none() {
                return;
            }
            if startup.refresh().await.is_err() {
                return;
            }
            let config = startup.config.read().await.clone();
            let server_id = startup.server_id.read().await.clone();
            let ids = config.auto_connect.clone();
            for busid in ids {
                let devices = startup.devices.read().await;
                let eligible = devices.iter().any(|d| {
                    d.busid == busid
                        && !is_laser(d)
                        && config
                            .auto_connect_identities
                            .get(&busid)
                            .is_some_and(|identity| identity.matches(&server_id, d))
                });
                drop(devices);
                if eligible {
                    if let Err(error) = startup.connect(&busid).await {
                        tracing::warn!(%busid, %error, "开机连接失败");
                    }
                }
            }
        });
        Ok(backend)
    }

    async fn cleanup_owned_orphans(&self) {
        let port = self.proxy.port.to_string();
        let imported = tokio::task::spawn_blocking(driver::imported).await;
        if let Ok(Ok(imported)) = imported {
            for device in imported {
                if device.host == "127.0.0.1" && device.service == port {
                    let result =
                        tokio::task::spawn_blocking(move || driver::detach(device.port)).await;
                    tracing::info!(?result, "清理上次服务退出后残留的本机端口");
                }
            }
        }
    }

    async fn refresh_driver(&self) {
        let result = tokio::task::spawn_blocking(driver::diagnostics).await;
        *self.driver_error.write().await = match result {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some(format!("{error:#}")),
            Err(error) => Some(format!("驱动检查任务失败：{error}")),
        };
    }

    pub async fn snapshot(&self) -> Snapshot {
        let config = self.config.read().await.clone();
        Snapshot {
            devices_checked_at_ms: match self.devices_checked_at_ms.load(Ordering::Relaxed) {
                0 => None,
                value => Some(value),
            },
            paired: config.client_id.is_some(),
            config,
            server_id: self.server_id.read().await.clone(),
            devices: self.devices.read().await.clone(),
            sessions: self
                .sessions
                .lock()
                .await
                .values()
                .map(|s| s.status.clone())
                .collect(),
            driver_error: self.driver_error.read().await.clone(),
            network_error: self.network_error.read().await.clone(),
            proxy_port: self.proxy.port,
        }
    }

    pub async fn handle(&self, request: LocalRequest) -> LocalResponse {
        if matches!(request, LocalRequest::CreateInvitation) {
            let _guard = self.mutations.lock().await;
            let result: Result<String> = async {
                let response = self.control(&ControlRequest::CreateInvitation).await
                    .context("此功能需要升级 Linux 服务端；已有授权会保留")?;
                let invite = response.invitation.context("Linux 未返回邀请，请升级 Linux 服务端")?;
                let config = self.config.read().await.clone();
                let ca_pem = std::fs::read_to_string(config::active_credentials_dir(&self.config_path)?.join("ca.pem"))?;
                Ok(serde_json::to_string(&ComputerInvitation { version: 1, host: config.host, port: config.port, server_name: config.server_name, ca_pem, code: invite.code, expires_at: invite.expires_at })?)
            }.await;
            return match result {
                Ok(invitation) => LocalResponse { ok: true, error: None, snapshot: Some(self.snapshot().await), log: None, invitation: Some(invitation) },
                Err(error) => LocalResponse::error(format!("{error:#}")),
            };
        }
        let result: Result<Option<String>> = async {
            match request {
                LocalRequest::Snapshot => Ok(None),
                LocalRequest::Refresh => {
                    let _guard = self.mutations.lock().await;
                    self.refresh().await?;
                    Ok(None)
                }
                LocalRequest::Logs => Ok(Some(self.log_export()?)),
                mutation => {
                    let _guard = self.mutations.lock().await;
                    match mutation {
                        LocalRequest::ConnectHost => {
                            let mut candidate = self.config.read().await.clone();
                            if candidate.client_id.is_none() { bail!("请先填写 Linux 地址和密码完成配对"); }
                            candidate.host_disconnected = false;
                            config::save(&self.config_path, &candidate)?;
                            *self.config.write().await = candidate;
                            self.refresh().await?;
                        }
                        LocalRequest::DisconnectHost => self.disconnect_host().await?,
                        LocalRequest::Pair {
                            host,
                            port,
                            server_name,
                            name,
                            ca_pem,
                            code,
                        } => {
                            self.pair(host, port, server_name, name, ca_pem, code)
                                .await?
                        }
                        LocalRequest::Connect { busid } => self.connect(&busid).await?,
                        LocalRequest::SetEndpoint { host, port } => self.set_endpoint(host, port).await?,
                        LocalRequest::Release { busid } => self.release(&busid).await?,
                        LocalRequest::ReleaseAll => {
                            // Disable opt-in reconnect before releasing so this computer
                            // cannot reclaim a device intended for another computer.
                            let mut candidate = self.config.read().await.clone();
                            candidate.auto_connect.clear();
                            candidate.auto_connect_identities.clear();
                            config::save(&self.config_path, &candidate)?;
                            *self.config.write().await = candidate;
                            let busids: Vec<String> = self.sessions.lock().await.keys().cloned().collect();
                            for busid in busids { self.release(&busid).await?; }
                            self.refresh().await?;
                        }
                        LocalRequest::SetShared { busid, shared } => {
                            config::validate_busid(&busid)?;
                            let response = self
                                .control(&ControlRequest::SetShared { busid, shared })
                                .await?;
                            // The durable mutation response already contains the
                            // new list. Do not add a second network operation.
                            self.apply_device_response(response).await;
                        }
                        LocalRequest::SetAutoConnect { busid, enabled } => {
                            self.set_auto_connect(&busid, enabled).await?
                        }
                        _ => unreachable!(),
                    }
                    Ok(None)
                }
            }
        }
        .await;
        match result {
            Ok(log) => LocalResponse {
                ok: true,
                error: None,
                snapshot: Some(self.snapshot().await),
                log,
                invitation: None,
            },
            Err(error) => {
                tracing::warn!(error = %error, "客户端操作失败");
                LocalResponse {
                    ok: false,
                    error: Some(format!("{error:#}")),
                    snapshot: Some(self.snapshot().await),
                    log: None,
                    invitation: None,
                }
            }
        }
    }

    async fn control(&self, request: &ControlRequest) -> Result<bridge_common::ControlResponse> {
        let config = self.config.read().await.clone();
        if config.host_disconnected { bail!("已主动断开 Linux，请点击连接 Linux"); }
        let tls = network::paired_tls(&self.config_path)?;
        network::control(&config, tls, request).await
    }

    pub async fn refresh(&self) -> Result<()> {
        let response = match self.control(&ControlRequest::ListDevices).await {
            Ok(response) => response,
            Err(error) => {
                *self.network_error.write().await = Some(format!("{error:#}"));
                return Err(error);
            }
        };
        self.apply_device_response(response).await;
        Ok(())
    }

    async fn apply_device_response(&self, response: bridge_common::ControlResponse) {
        if self.config.read().await.host_disconnected { return; }
        *self.devices.write().await = response.devices;
        *self.server_id.write().await = response.server_id;
        *self.network_error.write().await = None;
        self.devices_checked_at_ms.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            Ordering::Relaxed,
        );
    }

    async fn set_endpoint(&self, host: String, port: u16) -> Result<()> {
        if self.sessions.lock().await.values().any(|s| s.status.port.is_some() || s.status.state == "connecting") {
            bail!("请先释放本机全部 USB，再切换局域网或异地地址");
        }
        let mut candidate = self.config.read().await.clone();
        if candidate.client_id.is_none() { bail!("请先授权这台电脑，再切换连接地址"); }
        candidate.host = host;
        candidate.port = port;
        candidate.validate()?;
        let tls = network::paired_tls(&self.config_path)?;
        // Prove the new route reaches the same trusted host before saving it.
        // Keep the original per-computer identity and TLS server name.
        let response = network::control(&candidate, tls.clone(), &ControlRequest::ListDevices).await?;
        let expected = self.server_id.read().await.clone();
        if !expected.is_empty() && expected != response.server_id {
            bail!("新地址指向另一台主机，原连接设置已保留");
        }
        config::save(&self.config_path, &candidate)?;
        *self.config.write().await = candidate.clone();
        self.proxy.set_endpoint(Some(Endpoint { config: candidate, tls })).await;
        self.apply_device_response(response).await;
        Ok(())
    }

    async fn pair(
        &self,
        host: String,
        port: u16,
        server_name: String,
        name: String,
        ca_pem: String,
        code: String,
    ) -> Result<()> {
        if self
            .sessions
            .lock()
            .await
            .values()
            .any(|s| s.status.port.is_some() || s.status.state == "connecting")
        {
            bail!("请先释放本机全部 USB，再修改配对");
        }
        if code.trim().is_empty() || code.len() > 128 {
            bail!("配对码为空或过长");
        }
        let mut candidate = self.config.read().await.clone();
        candidate.host = host;
        candidate.port = port;
        candidate.server_name = server_name;
        candidate.display_name = name;
        candidate.validate()?;
        let tls = network::make_tls(ca_pem.as_bytes(), None)?;
        let response = network::control(
            &candidate,
            tls,
            &ControlRequest::Pair {
                code: code.trim().to_owned(),
                name: candidate.display_name.clone(),
            },
        )
        .await?;
        let pairing = response.pairing.context("Linux 没有返回客户端配对证书")?;
        if network::certificates(pairing.ca_cert_pem.as_bytes())?
            != network::certificates(ca_pem.as_bytes())?
        {
            bail!("返回的 CA 与选定的信任文件不同，配对已拒绝");
        }
        let tls = network::make_tls(
            pairing.ca_cert_pem.as_bytes(),
            Some((
                pairing.client_cert_pem.as_bytes(),
                pairing.client_key_pem.as_bytes(),
            )),
        )?;
        config::save_pairing(&self.config_path, &pairing)?;
        candidate.credential_generation = Some(pairing.client_id.clone());
        candidate.client_id = Some(pairing.client_id);
        candidate.host_disconnected = false;
        // Switching servers clears opt-in busids: a port on another host may
        // contain a different device. The stable local port remains unchanged.
        candidate.auto_connect.clear();
        candidate.auto_connect_identities.clear();
        config::save(&self.config_path, &candidate)?;
        *self.config.write().await = candidate.clone();
        *self.server_id.write().await = response.server_id;
        self.devices.write().await.clear();
        *self.network_error.write().await = None;
        self.sessions.lock().await.clear();
        self.proxy
            .set_endpoint(Some(Endpoint {
                config: candidate,
                tls,
            }))
            .await;
        tracing::info!("客户端配对完成");
        // Pairing is committed on both hosts. A best-effort list refresh must
        // not turn that success into a reported pairing failure. Bound this
        // follow-up separately so 90s management + 15s refresh fits 120s IPC.
        let refresh = tokio::time::timeout(Duration::from_secs(15), self.refresh()).await;
        let error = match refresh {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(format!("配对已完成，但设备列表暂时无法刷新：{error:#}")),
            Err(_) => Some("配对已完成，设备列表刷新超时；请稍后刷新设备状态".into()),
        };
        if let Some(error) = error {
            *self.network_error.write().await = Some(error);
        }
        Ok(())
    }

    async fn connect(&self, busid: &str) -> Result<()> {
        if self.config.read().await.host_disconnected { bail!("已主动断开 Linux，请先连接 Linux"); }
        config::validate_busid(busid)?;
        self.refresh().await?;
        let device = self
            .devices
            .read()
            .await
            .iter()
            .find(|d| d.busid == busid)
            .cloned()
            .context("设备已经离线")?;
        if !device.present {
            bail!("设备已经离线");
        }
        if let Some(reason) = &device.protected_reason {
            bail!("设备受保护：{reason}");
        }
        if !device.shared {
            bail!("请先在 Linux 启用此设备共享");
        }
        if let Some(owner) = &device.owner {
            bail!("设备已被 {owner} 占用，请先在原电脑释放");
        }
        let imported = tokio::task::spawn_blocking(driver::imported).await??;
        let local_service = self.proxy.port.to_string();
        if imported
            .iter()
            .any(|d| d.host == "127.0.0.1" && d.service == local_service && d.busid == busid)
        {
            bail!("设备已经在本机连接");
        }
        self.sessions.lock().await.remove(busid);
        self.proxy.revoke(busid).await;
        let (id, cancellation) = self.proxy.authorize(busid).await?;
        self.sessions.lock().await.insert(
            busid.to_owned(),
            Session {
                id,
                laser: is_laser(&device),
                status: SessionStatus {
                    busid: busid.to_owned(),
                    port: None,
                    state: "connecting".into(),
                    error: None,
                },
                cancellation: cancellation.clone(),
            },
        );
        let busid_owned = busid.to_owned();
        let port = self.proxy.port;
        let result = tokio::task::spawn_blocking(move || driver::attach(&busid_owned, port))
            .await
            .context("驱动连接任务异常")?;
        match result {
            Ok(driver_port) => {
                let mut sessions = self.sessions.lock().await;
                let session = sessions
                    .get_mut(busid)
                    .context("USB 会话在连接时已经结束")?;
                // If the stream closed during IOCTL, do not label it connected.
                if session.status.state == "failed" {
                    drop(sessions);
                    let _ = tokio::task::spawn_blocking(move || driver::detach(driver_port)).await;
                    bail!("USB 在连接期间断开，请查看网络状态");
                }
                session.status.port = Some(driver_port);
                session.status.state = "connected".into();
                tracing::info!(%busid, driver_port, vendor = device.vendor, product = device.product, "USB 已在本机连接");
                Ok(())
            }
            Err(error) => {
                let _ = cancellation.send(true);
                self.proxy.revoke(busid).await;
                if let Some(session) = self.sessions.lock().await.get_mut(busid) {
                    session.status.state = "failed".into();
                    session.status.error = Some(format!("{error:#}"));
                }
                Err(error)
            }
        }
    }

    async fn disconnect_host(&self) -> Result<()> {
        // Persist intent before touching any session. All callers hold mutations.
        let mut candidate = self.config.read().await.clone();
        candidate.host_disconnected = true;
        candidate.auto_connect.clear();
        candidate.auto_connect_identities.clear();
        config::save(&self.config_path, &candidate)?;
        *self.config.write().await = candidate;
        self.devices_checked_at_ms.store(0, Ordering::Relaxed);
        let busids: Vec<_> = self.sessions.lock().await.keys().cloned().collect();
        let mut failures = Vec::new();
        for busid in busids {
            if let Err(error) = self.release(&busid).await {
                failures.push(format!("{busid}: {error:#}"));
            }
        }
        *self.network_error.write().await = Some("已主动断开 Linux；配对已保留".into());
        if !failures.is_empty() { bail!("{}", failures.join("\n")); }
        Ok(())
    }

    async fn release(&self, busid: &str) -> Result<()> {
        config::validate_busid(busid)?;
        let mut candidate = self.config.read().await.clone();
        candidate.auto_connect.retain(|id| id != busid);
        candidate.auto_connect_identities.remove(busid);
        config::save(&self.config_path, &candidate)?;
        *self.config.write().await = candidate;
        let old_session = self.devices.read().await.iter().find(|d| d.busid == busid).and_then(|d| d.session_id.clone());
        let port = self
            .sessions
            .lock()
            .await
            .get(busid)
            .and_then(|s| s.status.port);
        let detach_error = if let Some(port) = port {
            tokio::task::spawn_blocking(move || driver::detach(port)).await
                .context("驱动断开任务失败").and_then(|r| r).err()
        } else { None };
        if let Some(session) = self.sessions.lock().await.remove(busid) {
            let _ = session.cancellation.send(true);
        }
        self.proxy.revoke(busid).await;
        if let Some(error) = detach_error { bail!("网络会话已关闭，但 Windows USB 驱动清理失败：{error:#}"); }
        tracing::info!(%busid, "USB 本机释放完成");
        // Local socket teardown is not proof that Linux has released its lease.
        // Confirm the old owner's lease disappeared before reporting success.
        let name = self.config.read().await.display_name.clone();
        tokio::time::timeout(Duration::from_secs(15), async {
            for _ in 0..15 {
                let config = self.config.read().await.clone();
                let tls = network::paired_tls(&self.config_path)?;
                let response = network::control(&config, tls, &ControlRequest::ListDevices).await
                    .context("本机已断开，Linux 释放状态待确认")?;
                let released = !response.devices.iter().any(|d| d.busid == busid &&
                    if let Some(id) = &old_session { d.session_id.as_ref() == Some(id) } else { d.owner.as_deref() == Some(&name) });
                self.apply_device_response(response).await;
                if released { return Ok(()); }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            bail!("本机已断开，但 Linux 尚未确认释放；请刷新状态后再让另一台电脑连接")
        }).await.context("本机已断开，等待 Linux 确认释放超时，请刷新状态")?
    }

    async fn proxy_ended(&self, event: ProxyEvent) {
        // Serialize the entire cleanup with connect/release/shutdown. Checking
        // the generation under sessions alone is insufficient: a reconnect
        // could otherwise install a new permit before this old event revokes
        // it or a reused driver port. No mutation awaits the event handler;
        // socket closure happens inside the proxy before the event is queued.
        let _guard = self.mutations.lock().await;
        let port = {
            let mut sessions = self.sessions.lock().await;
            let Some(session) = sessions.get_mut(&event.busid) else {
                return;
            };
            if session.id != event.session_id {
                return;
            }
            session.status.state = "failed".into();
            session.status.error = Some(event.error.unwrap_or_else(|| {
                if session.laser {
                    "USB 网络连接已断开。雕刻机的物理停止状态必须现场确认。".into()
                } else {
                    "USB 网络连接已断开，请检查网络或设备连接。".into()
                }
            }));
            let _ = session.cancellation.send(true);
            session.status.port.take()
        };
        self.proxy.revoke(&event.busid).await;
        if let Some(port) = port {
            let _ = tokio::task::spawn_blocking(move || driver::detach(port)).await;
        }
        tracing::warn!(busid = event.busid, "USB 会话断开，没有自动重连");
    }

    async fn set_auto_connect(&self, busid: &str, enabled: bool) -> Result<()> {
        config::validate_busid(busid)?;
        self.refresh().await?;
        let device = self
            .devices
            .read()
            .await
            .iter()
            .find(|d| d.busid == busid)
            .cloned()
            .context("设备已经离线")?;
        if enabled && is_laser(&device) {
            bail!("CH341 雕刻机始终手动连接");
        }
        if enabled && device.protected_reason.is_some() {
            bail!("受保护设备不能设置开机连接");
        }
        let mut candidate = self.config.read().await.clone();
        candidate.auto_connect.retain(|id| id != busid);
        candidate.auto_connect_identities.remove(busid);
        if enabled {
            candidate.auto_connect.push(busid.to_owned());
            candidate.auto_connect_identities.insert(
                busid.to_owned(),
                config::DeviceIdentity::from_device(&self.server_id.read().await, &device),
            );
        }
        config::save(&self.config_path, &candidate)?;
        *self.config.write().await = candidate;
        Ok(())
    }

    fn log_export(&self) -> Result<String> {
        let directory = config::directory(&self.config_path).join("logs");
        let mut paths = std::fs::read_dir(directory)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect::<Vec<_>>();
        paths.sort();
        paths.reverse();
        let mut output = String::new();
        for path in paths.into_iter().take(3) {
            let bytes = std::fs::read(&path)?;
            let start = bytes.len().saturating_sub(180_000);
            output.push_str(&format!(
                "\n=== {} ===\n",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            output.push_str(&String::from_utf8_lossy(&bytes[start..]));
        }
        Ok(output)
    }

    pub async fn shutdown(&self) {
        let _guard = self.mutations.lock().await;
        let sessions = std::mem::take(&mut *self.sessions.lock().await);
        for (busid, session) in sessions {
            let _ = session.cancellation.send(true);
            self.proxy.revoke(&busid).await;
            if let Some(port) = session.status.port {
                if let Ok(Err(error)) =
                    tokio::task::spawn_blocking(move || driver::detach(port)).await
                {
                    tracing::error!(%busid, %error, "关闭服务时释放失败");
                }
            }
        }
        self.proxy.shutdown().await;
    }
}

pub fn is_laser(device: &UsbDevice) -> bool {
    device.vendor == 0x1a86 && device.product == 0x5512
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invitation_import_validates_expiry_address_and_certificate() {
        let (ca, _, _) = control_fixture();
        let mut invite = ComputerInvitation { version: 1, host: "127.0.0.1".into(), port: 3241, server_name: "usb-bridge".into(), ca_pem: ca, code: "test-invitation".into(), expires_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 900 };
        assert!(ComputerInvitation::parse(&serde_json::to_string(&invite).unwrap()).is_ok());
        invite.expires_at = 0;
        assert!(ComputerInvitation::parse(&serde_json::to_string(&invite).unwrap()).is_err());
        invite.expires_at = u64::MAX;
        invite.port = 0;
        assert!(ComputerInvitation::parse(&serde_json::to_string(&invite).unwrap()).is_err());
        invite.port = 3241;
        invite.ca_pem = "not a certificate".into();
        assert!(ComputerInvitation::parse(&serde_json::to_string(&invite).unwrap()).is_err());
    }

    async fn test_backend(path: PathBuf, config: ClientConfig) -> Arc<Backend> {
        let (sender, _events) = mpsc::unbounded_channel();
        Arc::new(Backend {
            devices_checked_at_ms: AtomicU64::new(0),
            config_path: path,
            config: RwLock::new(config),
            devices: RwLock::new(Vec::new()),
            server_id: RwLock::new(String::new()),
            network_error: RwLock::new(None),
            driver_error: RwLock::new(None),
            sessions: Mutex::new(BTreeMap::new()),
            mutations: Mutex::new(()),
            proxy: Proxy::start(0, sender).await.unwrap(),
        })
    }

    #[tokio::test]
    async fn host_disconnect_persists_and_blocks_queued_actions_and_stale_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.json");
        let config = ClientConfig { host: "127.0.0.1".into(), client_id: Some("paired".into()), auto_connect: vec!["1-2".into()], ..Default::default() };
        let backend = test_backend(path.clone(), config).await;
        assert!(backend.handle(LocalRequest::DisconnectHost).await.ok);
        let saved = config::load(&path).unwrap();
        assert!(saved.host_disconnected);
        assert!(saved.auto_connect.is_empty());
        assert_eq!(saved.client_id.as_deref(), Some("paired"));
        let restarted = test_backend(path, saved).await;
        for request in [LocalRequest::Refresh, LocalRequest::Connect { busid: "1-2".into() }, LocalRequest::SetShared { busid: "1-2".into(), shared: true }] {
            let response = restarted.handle(request).await;
            assert!(!response.ok);
            assert!(response.error.unwrap().contains("断开 Linux"));
        }
        restarted.apply_device_response(bridge_common::ControlResponse { ok: true, server_id: "stale".into(), ..Default::default() }).await;
        assert!(restarted.server_id.read().await.is_empty());
        assert_eq!(restarted.devices_checked_at_ms.load(Ordering::Relaxed), 0);
    }

    fn control_fixture() -> (
        String,
        bridge_common::PairingBundle,
        tokio_rustls::TlsAcceptor,
    ) {
        use rcgen::{
            BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair,
            KeyUsagePurpose,
        };
        let ca_key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = params.self_signed(&ca_key).unwrap();
        let server_key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["usb-bridge".into()]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let server = params.signed_by(&server_key, &ca, &ca_key).unwrap();
        let client_key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["test-paired-identity".into()]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let client = params.signed_by(&client_key, &ca, &ca_key).unwrap();
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![server.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                server_key.serialize_der(),
            )),
        )
        .unwrap();
        let bundle = bridge_common::PairingBundle {
            client_id: "test-paired-identity".into(),
            ca_cert_pem: ca.pem(),
            client_cert_pem: client.pem(),
            client_key_pem: client_key.serialize_pem(),
        };
        (
            ca.pem(),
            bundle,
            tokio_rustls::TlsAcceptor::from(Arc::new(tls)),
        )
    }

    async fn control_socket(
        listener: &tokio::net::TcpListener,
        acceptor: &tokio_rustls::TlsAcceptor,
    ) -> tokio_rustls::server::TlsStream<tokio::net::TcpStream> {
        use tokio::io::AsyncReadExt;
        let (socket, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(socket).await.unwrap();
        let mut selector = [0; 8];
        tls.read_exact(&mut selector).await.unwrap();
        assert_eq!(&selector, b"control\n");
        tls
    }

    #[tokio::test]
    async fn committed_pairing_succeeds_when_followup_list_fails() {
        use tokio::io::AsyncWriteExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.json");
        let backend = test_backend(path.clone(), ClientConfig::default()).await;
        let (ca, bundle, acceptor) = control_fixture();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = tokio::spawn(async move {
            let mut tls = control_socket(&listener, &acceptor).await;
            let request: ControlRequest = network::read_frame(&mut tls).await.unwrap();
            assert!(matches!(request, ControlRequest::Pair { .. }));
            network::write_frame(
                &mut tls,
                &bridge_common::ControlResponse {
                    ok: true,
                    pairing: Some(bundle),
                    server_id: "test-server".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            tls.shutdown().await.unwrap();
            let mut tls = control_socket(&listener, &acceptor).await;
            let request: ControlRequest = network::read_frame(&mut tls).await.unwrap();
            assert!(matches!(request, ControlRequest::ListDevices));
            network::write_frame(
                &mut tls,
                &bridge_common::ControlResponse {
                    ok: false,
                    error: Some("synthetic refresh failure".into()),
                    server_id: "test-server".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            tls.shutdown().await.unwrap();
        });
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            backend.handle(LocalRequest::Pair {
                host: "127.0.0.1".into(),
                port,
                server_name: "usb-bridge".into(),
                name: "test user".into(),
                ca_pem: ca,
                code: "test-code".into(),
            }),
        )
        .await
        .unwrap();
        assert!(
            response.ok,
            "committed pairing was reported as failed: {:?}",
            response.error
        );
        let snapshot = response.snapshot.unwrap();
        assert!(snapshot.paired);
        assert_eq!(
            snapshot.config.client_id.as_deref(),
            Some("test-paired-identity")
        );
        assert!(snapshot
            .network_error
            .unwrap()
            .contains("synthetic refresh failure"));
        assert_eq!(
            config::load(&path).unwrap().client_id.as_deref(),
            Some("test-paired-identity")
        );
        assert!(network::paired_tls(&path).is_ok());
        peer.await.unwrap();
        backend.shutdown().await;
    }

    #[tokio::test]
    async fn failed_refresh_marks_cached_devices_as_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let backend = test_backend(
            directory.path().join("client.json"),
            ClientConfig::default(),
        )
        .await;
        *backend.server_id.write().await = "cached-server".into();
        assert!(backend.refresh().await.is_err());
        let snapshot = backend.snapshot().await;
        assert!(snapshot.network_error.is_some());
        assert!(snapshot.devices_checked_at_ms.is_none());
        backend.shutdown().await;
    }

    #[tokio::test]
    async fn shared_mutation_uses_success_reply_without_second_network_request() {
        use tokio::io::AsyncWriteExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.json");
        let (_ca, bundle, acceptor) = control_fixture();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        config::save_pairing(&path, &bundle).unwrap();
        let candidate = ClientConfig {
            host: "127.0.0.1".into(),
            port,
            client_id: Some(bundle.client_id.clone()),
            credential_generation: Some(bundle.client_id),
            ..ClientConfig::default()
        };
        config::save(&path, &candidate).unwrap();
        let backend = test_backend(path, candidate).await;
        let device = UsbDevice {
            busid: "1-2".into(),
            name: "synthetic serial".into(),
            vendor: 0x1234,
            product: 0x5678,
            serial: None,
            path: "/mock/1-2".into(),
            speed: 3,
            shared: true,
            owner: None,
            protected_reason: None,
            present: true, device_type: None, session_id: None,
        };
        let peer = tokio::spawn(async move {
            let mut tls = control_socket(&listener, &acceptor).await;
            let request: ControlRequest = network::read_frame(&mut tls).await.unwrap();
            assert!(matches!(
                request,
                ControlRequest::SetShared { shared: true, .. }
            ));
            network::write_frame(
                &mut tls,
                &bridge_common::ControlResponse {
                    ok: true,
                    devices: vec![device],
                    server_id: "test-server".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            tls.shutdown().await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(150), listener.accept())
                    .await
                    .is_err(),
                "a second query followed the already-complete mutation response"
            );
        });
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            backend.handle(LocalRequest::SetShared {
                busid: "1-2".into(),
                shared: true,
            }),
        )
        .await
        .unwrap();
        assert!(response.ok);
        let snapshot = response.snapshot.unwrap();
        assert_eq!(snapshot.server_id, "test-server");
        assert_eq!(snapshot.devices.len(), 1);
        assert!(snapshot.devices[0].shared);
        assert!(snapshot.network_error.is_none());
        assert!(snapshot.devices_checked_at_ms.is_some());
        peer.await.unwrap();
        backend.shutdown().await;
    }
    #[tokio::test]
    async fn invitation_uses_active_credentials_and_handoff_disables_reconnect() {
        use tokio::io::AsyncWriteExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.json");
        let (_ca, bundle, acceptor) = control_fixture();
        config::save_pairing(&path, &bundle).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let candidate = ClientConfig { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), client_id: Some(bundle.client_id.clone()), credential_generation: Some(bundle.client_id), auto_connect: vec!["1-2".into()], ..ClientConfig::default() };
        config::save(&path, &candidate).unwrap();
        let backend = test_backend(path.clone(), candidate).await;
        let peer = tokio::spawn(async move {
            for step in 0..2 {
                let mut tls = control_socket(&listener, &acceptor).await;
                let request: ControlRequest = network::read_frame(&mut tls).await.unwrap();
                let mut response = bridge_common::ControlResponse { ok: true, server_id: "test-server".into(), ..Default::default() };
                if step == 0 {
                    assert!(matches!(request, ControlRequest::CreateInvitation));
                    response.invitation = Some(bridge_common::EnrollmentInvitation { code: "test-invitation".into(), expires_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 900 });
                } else { assert!(matches!(request, ControlRequest::ListDevices)); }
                network::write_frame(&mut tls, &response).await.unwrap();
                tls.shutdown().await.unwrap();
            }
        });
        let response = backend.handle(LocalRequest::CreateInvitation).await;
        assert!(response.ok, "{:?}", response.error);
        let invite = ComputerInvitation::parse(&response.invitation.unwrap()).unwrap();
        assert_eq!(invite.host, "127.0.0.1");
        assert!(!invite.ca_pem.contains("PRIVATE KEY"));
        let response = backend.handle(LocalRequest::ReleaseAll).await;
        assert!(response.ok, "{:?}", response.error);
        assert!(response.snapshot.unwrap().sessions.is_empty());
        assert!(config::load(&path).unwrap().auto_connect.is_empty());
        assert!(config::load(&path).unwrap().auto_connect_identities.is_empty());
        peer.await.unwrap();
        backend.shutdown().await;
    }

    #[tokio::test]
    async fn switching_routes_keeps_identity_and_rejects_another_server() {
        use tokio::io::AsyncWriteExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.json");
        let (_, bundle, acceptor) = control_fixture();
        config::save_pairing(&path, &bundle).unwrap();
        let original = ClientConfig { host: "192.0.2.1".into(), client_id: Some(bundle.client_id.clone()), credential_generation: Some(bundle.client_id.clone()), ..ClientConfig::default() };
        config::save(&path, &original).unwrap();
        let backend = test_backend(path.clone(), original).await;
        *backend.server_id.write().await = "my-linux".into();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = tokio::spawn(async move {
            for server_id in ["another-linux", "my-linux"] {
                let mut tls = control_socket(&listener, &acceptor).await;
                let request: ControlRequest = network::read_frame(&mut tls).await.unwrap();
                assert!(matches!(request, ControlRequest::ListDevices));
                network::write_frame(&mut tls, &bridge_common::ControlResponse { ok: true, server_id: server_id.into(), ..Default::default() }).await.unwrap();
                tls.shutdown().await.unwrap();
            }
        });
        let rejected = backend.handle(LocalRequest::SetEndpoint { host: "127.0.0.1".into(), port }).await;
        assert!(!rejected.ok);
        assert_eq!(config::load(&path).unwrap().host, "192.0.2.1");
        let accepted = backend.handle(LocalRequest::SetEndpoint { host: "127.0.0.1".into(), port }).await;
        assert!(accepted.ok, "{:?}", accepted.error);
        let saved = config::load(&path).unwrap();
        assert_eq!(saved.host, "127.0.0.1");
        assert_eq!(saved.port, port);
        assert_eq!(saved.client_id, Some(bundle.client_id.clone()));
        assert_eq!(saved.credential_generation, Some(bundle.client_id));
        peer.await.unwrap();
        backend.shutdown().await;
    }

    #[tokio::test]
    async fn old_disconnect_waits_for_mutation_and_cannot_revoke_new_session() {
        // Real proxy permits and the real handler, with no driver installed or
        // device attached. The mutation guard models an in-progress reconnect.
        let directory = tempfile::tempdir().unwrap();
        let (sender, _events) = mpsc::unbounded_channel();
        let proxy = Proxy::start(0, sender).await.unwrap();
        let backend = Arc::new(Backend {
            devices_checked_at_ms: AtomicU64::new(0),
            config_path: directory.path().join("client.json"),
            config: RwLock::new(ClientConfig::default()),
            devices: RwLock::new(Vec::new()),
            server_id: RwLock::new(String::new()),
            network_error: RwLock::new(None),
            driver_error: RwLock::new(None),
            sessions: Mutex::new(BTreeMap::new()),
            mutations: Mutex::new(()),
            proxy,
        });
        let busid = "1-2";
        let make_session = |id, cancellation| Session {
            id,
            cancellation,
            laser: false,
            status: SessionStatus {
                busid: busid.into(),
                port: None,
                state: "connecting".into(),
                error: None,
            },
        };
        let (old_id, old_cancel) = backend.proxy.authorize(busid).await.unwrap();
        backend
            .sessions
            .lock()
            .await
            .insert(busid.into(), make_session(old_id, old_cancel));
        let mutation = backend.mutations.lock().await;
        let handler_backend = backend.clone();
        let mut handler = tokio::spawn(async move {
            handler_backend
                .proxy_ended(ProxyEvent {
                    busid: busid.into(),
                    session_id: old_id,
                    error: None,
                })
                .await;
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut handler)
                .await
                .is_err(),
            "disconnect cleanup bypassed the mutation lock"
        );
        backend.proxy.revoke(busid).await;
        let (new_id, new_cancel) = backend.proxy.authorize(busid).await.unwrap();
        let new_receiver = new_cancel.subscribe();
        backend
            .sessions
            .lock()
            .await
            .insert(busid.into(), make_session(new_id, new_cancel));
        drop(mutation);
        tokio::time::timeout(Duration::from_secs(2), handler)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(backend.sessions.lock().await[busid].id, new_id);
        assert_eq!(
            backend.sessions.lock().await[busid].status.state,
            "connecting"
        );
        assert!(!*new_receiver.borrow());
        assert!(
            backend.proxy.authorize(busid).await.is_err(),
            "old cleanup removed the new permit"
        );
        backend.shutdown().await;
    }
}
