pub mod config;
pub mod admin;
pub mod sysfs;

use anyhow::{bail, Context, Result};
use bridge_common::{
    ControlRequest, ControlResponse, UsbDevice, UsbIpHeader, OP_REP_DEVLIST, OP_REP_IMPORT,
    OP_REQ_DEVLIST, OP_REQ_IMPORT, USBIP_VERSION,
};
use config::Config;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use sysfs::{Backend, Device};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{watch, Mutex, Semaphore},
};
use tokio_rustls::{server::TlsStream, TlsAcceptor};
use tracing::Instrument;

static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct Lease {
    client_id: String,
    name: String,
    token: String,
    cancel: watch::Sender<bool>,
}
struct State {
    config: Config,
    leases: BTreeMap<String, Lease>,
    binding_errors: BTreeMap<String, String>,
    shutting_down: bool,
}
pub struct Server {
    state_dir: PathBuf,
    pub backend: Backend,
    state: Arc<Mutex<State>>,
    _state_lock: std::fs::File,
}
type SecureStream = TlsStream<TcpStream>;

fn keepalive(socket: &TcpStream) -> Result<()> {
    let socket = socket2::SockRef::from(socket);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(20))
        .with_interval(Duration::from_secs(5));
    #[cfg(target_os = "linux")]
    let keepalive = keepalive.with_retries(3);
    socket.set_tcp_keepalive(&keepalive)?;
    #[cfg(target_os = "linux")]
    socket.set_tcp_user_timeout(Some(Duration::from_secs(30)))?;
    Ok(())
}

impl Server {
    pub fn open(state_dir: PathBuf, backend: Backend) -> Result<Arc<Self>> {
        use fs2::FileExt;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(state_dir.join("server.lock"))?;
        lock.try_lock_exclusive()
            .context("another server or restore process is using this state directory")?;
        let config = config::load(&state_dir)?;
        Ok(Arc::new(Self {
            state_dir,
            backend,
            state: Arc::new(Mutex::new(State {
                config,
                leases: BTreeMap::new(),
                binding_errors: BTreeMap::new(),
                shutting_down: false,
            })),
            _state_lock: lock,
        }))
    }
    fn annotate(&self, state: &State, device: &mut Device) {
        let managed = state
            .config
            .shared
            .get(&device.public.busid)
            .is_some_and(|r| Backend::identity_matches(device, r));
        device.public.shared = managed && device.driver.as_deref() == Some("usbip-host");
        device.public.owner = state
            .leases
            .get(&device.public.busid)
            .map(|l| l.name.clone());
        device.public.session_id = state.leases.get(&device.public.busid).map(|l| l.token.clone());
        if device.driver.as_deref() == Some("usbip-host") && !managed {
            device.public.protected_reason = Some("设备由另一套 USB/IP 服务持有".into());
        }
    }
    fn list(&self, state: &State) -> Result<Vec<UsbDevice>> {
        let mut devices = self.backend.discover()?;
        for device in &mut devices {
            self.annotate(state, device);
        }
        let mut public: Vec<_> = devices.into_iter().map(|d| d.public).collect();
        for (busid, record) in &state.config.shared {
            if !public.iter().any(|d| &d.busid == busid) {
                public.push(UsbDevice {
                    busid: busid.clone(),
                    name: format!("USB {:04x}:{:04x}", record.vendor, record.product),
                    vendor: record.vendor,
                    product: record.product,
                    serial: record.serial.clone(),
                    path: record.path.clone(),
                    speed: 0,
                    shared: false,
                    owner: None,
                    protected_reason: None,
                    present: false, device_type: None, session_id: None,
                });
            }
        }
        Ok(public)
    }
    async fn blocking_state<R, F>(self: &Arc<Self>, operation: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Server, &mut State) -> Result<R> + Send + 'static,
    {
        let started = Instant::now();
        tracing::debug!("blocking operation state lock waiting");
        let mut state = self.state.clone().lock_owned().await;
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "blocking operation state lock acquired"
        );
        // The worker owns both the transaction guard and Server (including its
        // process lock file). Client cancellation cannot interrupt a committed
        // intent between persistence, driver changes, and recovery.
        let server = self.clone();
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || span.in_scope(|| operation(&server, &mut state)))
            .await
            .context("blocking USB management worker failed")?
    }
    async fn authenticated_client(&self, tls: &SecureStream) -> Option<(String, String)> {
        let certificate = tls.get_ref().1.peer_certificates()?.first()?;
        let fingerprint = hex::encode(Sha256::digest(certificate.as_ref()));
        let state = self.state.lock().await;
        let identity = state
            .config
            .clients
            .iter()
            .find(|(_, c)| c.certificate_sha256 == fingerprint)
            .map(|(id, c)| (id.clone(), c.name.clone()));
        drop(state);
        identity
    }
    async fn control(
        self: &Arc<Self>,
        request: ControlRequest,
        identity: Option<(String, String)>,
    ) -> ControlResponse {
        let started = Instant::now();
        // Only a fixed operation name is logged. Pair codes, names and all
        // request/response credential payloads stay out of diagnostic output.
        let operation_name = match &request {
            ControlRequest::Pair { .. } => "pair",
            ControlRequest::CreateInvitation => "create_invitation",
            ControlRequest::ListDevices => "list_devices",
            ControlRequest::Status => "status",
            ControlRequest::SetShared { shared: true, .. } => "share",
            ControlRequest::SetShared { shared: false, .. } => "unshare",
        };
        let span = tracing::debug_span!("management", operation = operation_name);
        let result = self
            .blocking_state(move |server, state| {
                Ok(server.control_locked(request, identity, state, started, operation_name))
            })
            .instrument(span)
            .await;
        match result {
            Ok(response) => response,
            Err(error) => ControlResponse {
                error: Some(format!("后台 USB 管理任务失败：{error:#}")),
                server_id: self.state.lock().await.config.server_id.clone(),
                ..Default::default()
            },
        }
    }
    fn control_locked(
        &self,
        request: ControlRequest,
        identity: Option<(String, String)>,
        state: &mut State,
        started: Instant,
        operation_name: &'static str,
    ) -> ControlResponse {
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "management operation started"
        );
        let mut response = ControlResponse {
            server_id: state.config.server_id.clone(),
            ..Default::default()
        };
        let operation = (|| -> Result<()> {
            if state.shutting_down {
                bail!("服务正在停止，请稍后再试");
            }
            if let ControlRequest::Pair { code, name } = request {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
                let invitation_hash = hex::encode(Sha256::digest(code.as_bytes()));
                let invited = state.config.invitations.get(&invitation_hash)
                    .is_some_and(|invite| invite.expires_at > now && state.config.clients.contains_key(&invite.issued_by));
                if !invited && state
                    .config
                    .pair_code
                    .as_bytes()
                    .ct_eq(code.as_bytes())
                    .unwrap_u8()
                    != 1
                {
                    bail!("授权码无效、已使用或已过期；请在已授权电脑上重新生成邀请");
                }
                tracing::debug!("pair certificate issuance started");
                let (bundle, record) = config::issue_client(&self.state_dir, &name)?;
                tracing::debug!(
                    elapsed_ms = started.elapsed().as_millis(),
                    "pair certificate issuance completed"
                );
                let previous = state.config.clone();
                state
                    .config
                    .clients
                    .insert(bundle.client_id.clone(), record);
                if invited {
                    state.config.invitations.remove(&invitation_hash);
                } else {
                    state.config.pair_code = uuid::Uuid::new_v4().to_string();
                }
                if let Err(error) = config::save(&self.state_dir, &state.config) {
                    state.config = previous;
                    return Err(error);
                }
                response.pairing = Some(bundle);
                return Ok(());
            }
            let Some((id, _name)) = identity else {
                bail!("请先配对；此操作需要已批准的客户端证书");
            };
            match request {
                ControlRequest::CreateInvitation => {
                    if !state.config.clients.contains_key(&id) { bail!("此电脑的授权已失效"); }
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
                    let code = uuid::Uuid::new_v4().to_string();
                    let expires_at = now + 15 * 60;
                    let mut candidate = state.config.clone();
                    candidate.invitations.retain(|_, invite| invite.expires_at > now);
                    if candidate.invitations.len() >= 32 { bail!("有效邀请过多，请稍后再试"); }
                    candidate.invitations.insert(hex::encode(Sha256::digest(code.as_bytes())), config::InvitationRecord { expires_at, issued_by: id });
                    config::save(&self.state_dir, &candidate)?;
                    state.config = candidate;
                    response.invitation = Some(bridge_common::EnrollmentInvitation { code, expires_at });
                }
                ControlRequest::ListDevices | ControlRequest::Status => {
                    response.devices = self.list(&state)?;
                }
                ControlRequest::SetShared { busid, shared } => {
                    if !bridge_common::valid_busid(&busid) {
                        bail!("无效的 USB 设备编号");
                    }
                    if state.leases.contains_key(&busid) {
                        bail!("设备仍被 Windows 占用；请先断开连接");
                    }
                    if shared {
                        let device = self
                            .backend
                            .discover()?
                            .into_iter()
                            .find(|d| d.public.busid == busid)
                            .context("设备已离线")?;
                        let previous_record = state.config.shared.get(&busid).cloned();
                        if let Some(record) = &previous_record {
                            if !Backend::identity_matches(&device, record) {
                                bail!("此端口的设备身份已改变；请先关闭旧的共享记录");
                            }
                            if device.driver.as_deref() == Some("usbip-host") {
                                response.devices = self.list(&state)?;
                                return Ok(());
                            }
                            if device.driver.is_some() && device.driver != record.original_driver {
                                bail!("设备已绑定其他驱动；请先恢复原共享记录后再操作");
                            }
                        }
                        if let Some(reason) = &device.public.protected_reason {
                            bail!("{reason}");
                        }
                        if device.driver.as_deref() == Some("usbip-host") {
                            bail!("设备已由其他 USB/IP 服务持有");
                        }
                        // A failed earlier bind may leave the device unbound. Keep
                        // its saved original driver instead of recording that gap.
                        let record = previous_record.unwrap_or_else(|| Backend::record(&device));
                        // Write ownership intent first so a crash between unbind/bind
                        // can be recovered by the explicit restore command.
                        let mut candidate = state.config.clone();
                        candidate.shared.insert(busid.clone(), record.clone());
                        config::save(&self.state_dir, &candidate)
                            .context("保存 USB 恢复记录失败；尚未开始更改设备驱动")?;
                        state.config = candidate;
                        if let Err(error) = self.backend.bind(&device) {
                            if let Err(restore_error) = self.backend.restore(&busid, &record) {
                                // The intent was saved before bind. Keep its identity
                                // and original driver so restore can retry later.
                                return Err(error.context(format!(
                                    "USB 共享绑定失败；原驱动恢复失败，恢复记录已保留：{restore_error:#}"
                                )));
                            }
                            state.config.shared.remove(&busid);
                            if let Err(save_error) = config::save(&self.state_dir, &state.config) {
                                return Err(error.context(format!(
                                    "USB 共享绑定失败；原驱动已恢复，但保存共享记录清理失败：{save_error:#}"
                                )));
                            }
                            return Err(error.context("USB 共享绑定失败；原驱动已恢复"));
                        }
                        tracing::info!(%busid, "USB sharing enabled");
                    } else if let Some(record) = state.config.shared.get(&busid).cloned() {
                        let present = self
                            .backend
                            .discover()?
                            .into_iter()
                            .find(|d| d.public.busid == busid);
                        // If a replacement occupies the old port, forget the old
                        // record without touching the replacement's driver.
                        if present
                            .as_ref()
                            .is_some_and(|d| Backend::identity_matches(d, &record))
                        {
                            self.backend.restore(&busid, &record)?;
                        }
                        state.config.shared.remove(&busid);
                        config::save(&self.state_dir, &state.config)?;
                        tracing::info!(%busid, "USB sharing disabled");
                    }
                    response.devices = self.list(&state)?;
                }
                ControlRequest::Pair { .. } => unreachable!(),
            }
            Ok(())
        })();
        match operation {
            Ok(()) => response.ok = true,
            Err(e) => response.error = Some(format!("{e:#}")),
        }
        tracing::debug!(
            operation = operation_name,
            ok = response.ok,
            elapsed_ms = started.elapsed().as_millis(),
            "management operation completed"
        );
        response
    }
    async fn selector(tls: &mut SecureStream) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        while bytes.len() < 8 {
            let byte = tls.read_u8().await?;
            bytes.push(byte);
            if byte == b'\n' {
                return Ok(bytes);
            }
        }
        bail!("unknown protocol selector")
    }
    async fn connection(self: Arc<Self>, mut tls: SecureStream) -> Result<()> {
        let started = Instant::now();
        tracing::debug!("protocol selector waiting");
        let selector =
            tokio::time::timeout(Duration::from_secs(10), Self::selector(&mut tls)).await??;
        let protocol = if selector == bridge_common::CONTROL_SELECTOR {
            "control"
        } else if selector == bridge_common::USBIP_SELECTOR {
            "usbip"
        } else {
            "unknown"
        };
        tracing::debug!(
            protocol,
            elapsed_ms = started.elapsed().as_millis(),
            "protocol selector received"
        );
        tracing::debug!("client authorization started");
        let identity = self.authenticated_client(&tls).await;
        tracing::debug!(
            paired = identity.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "client authorization completed"
        );
        if selector == bridge_common::CONTROL_SELECTOR {
            tracing::debug!("management request frame waiting");
            let request = tokio::time::timeout(
                Duration::from_secs(10),
                bridge_common::read_json::<_, ControlRequest>(&mut tls),
            )
            .await??;
            let response = self.control(request, identity).await;
            if !response.ok
                && response
                    .error
                    .as_ref()
                    .is_some_and(|e| e.starts_with("配对码"))
            {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            tracing::debug!(
                elapsed_ms = started.elapsed().as_millis(),
                "management response send started"
            );
            bridge_common::write_json(&mut tls, &response).await?;
            tracing::debug!(
                elapsed_ms = started.elapsed().as_millis(),
                "management response sent"
            );
            tracing::debug!("TLS shutdown started");
            tls.shutdown().await?;
            tracing::debug!(
                elapsed_ms = started.elapsed().as_millis(),
                "TLS shutdown completed"
            );
        } else if selector == bridge_common::USBIP_SELECTOR {
            let identity = identity.context("USB/IP requires a paired client certificate")?;
            self.usbip(tls, identity).await?;
        } else {
            bail!("unknown protocol selector");
        }
        Ok(())
    }
    async fn usbip(
        self: &Arc<Self>,
        mut tls: SecureStream,
        identity: (String, String),
    ) -> Result<()> {
        let mut bytes = [0; 8];
        tokio::time::timeout(Duration::from_secs(10), tls.read_exact(&mut bytes)).await??;
        let header = UsbIpHeader::decode(bytes);
        if header.version != USBIP_VERSION || header.status != 0 {
            bail!("unsupported or invalid USB/IP negotiation header");
        }
        if header.code == OP_REQ_DEVLIST {
            let devices: Vec<_> = self
                .blocking_state(|server, state| {
                    let mut devices = server.backend.discover()?;
                    for device in &mut devices {
                        server.annotate(state, device);
                    }
                    Ok(devices
                        .into_iter()
                        .filter(|d| d.public.shared && d.public.protected_reason.is_none())
                        .collect())
                })
                .await?;
            // A slow DEVLIST reader must not hold the global lease/config lock.
            tls.write_all(
                &UsbIpHeader {
                    version: USBIP_VERSION,
                    code: OP_REP_DEVLIST,
                    status: 0,
                }
                .encode(),
            )
            .await?;
            tls.write_u32(devices.len() as u32).await?;
            for device in devices {
                tls.write_all(&device.descriptor.encode()?).await?;
                for interface in device.descriptor.interfaces {
                    tls.write_all(&interface).await?;
                }
            }
            tls.flush().await?;
            return Ok(());
        }
        if header.code != OP_REQ_IMPORT {
            bail!("unsupported USB/IP negotiation operation");
        }
        let mut busid_bytes = [0; 32];
        tokio::time::timeout(Duration::from_secs(10), tls.read_exact(&mut busid_bytes)).await??;
        let end = busid_bytes
            .iter()
            .position(|b| *b == 0)
            .context("busid is not NUL terminated")?;
        if busid_bytes[end..].iter().any(|b| *b != 0) {
            bail!("nonzero USB/IP busid padding");
        }
        let busid = std::str::from_utf8(&busid_bytes[..end])?.to_string();
        let reservation = self.reserve(&busid, identity).await;
        let (device, token, mut cancelled) = match reservation {
            Ok(value) => value,
            Err(error) => {
                tls.write_all(
                    &UsbIpHeader {
                        version: USBIP_VERSION,
                        code: OP_REP_IMPORT,
                        status: 1,
                    }
                    .encode(),
                )
                .await?;
                tls.flush().await?;
                tracing::warn!(%busid, %error, "USB import denied");
                return Ok(());
            }
        };
        let transfer = async {
            let descriptor = device.descriptor.encode()?;
            let (mut proxy, kernel) = match self.kernel_socket(&busid).await {
                Ok(sockets) => sockets,
                Err(error) => {
                    tls.write_all(
                        &UsbIpHeader {
                            version: USBIP_VERSION,
                            code: OP_REP_IMPORT,
                            status: 1,
                        }
                        .encode(),
                    )
                    .await?;
                    tls.flush().await?;
                    return Err(error);
                }
            };
            tls.write_all(
                &UsbIpHeader {
                    version: USBIP_VERSION,
                    code: OP_REP_IMPORT,
                    status: 0,
                }
                .encode(),
            )
            .await?;
            tls.write_all(&descriptor).await?;
            tls.flush().await?;
            tracing::info!(%busid, "USB import established");
            // Raw URB bytes are forwarded unchanged, including isochronous
            // descriptors. Reconnection never caches or replays a request.
            tokio::select! {
                value = tokio::io::copy_bidirectional(&mut tls, &mut proxy) => { value?; }
                _ = cancelled.changed() => {}
            }
            let _ = proxy.shutdown().await;
            drop(kernel);
            Ok::<(), anyhow::Error>(())
        }
        .await;
        self.release(&busid, &token).await;
        transfer
    }
    async fn reserve(
        self: &Arc<Self>,
        busid: &str,
        identity: (String, String),
    ) -> Result<(Device, String, watch::Receiver<bool>)> {
        if !bridge_common::valid_busid(busid) {
            bail!("invalid USB busid");
        }
        let busid = busid.to_owned();
        self.blocking_state(move |server, state| {
            if state.shutting_down {
                bail!("server is shutting down");
            }
            if state.leases.contains_key(&busid) {
                bail!("device is owned by another session");
            }
            let mut device = server
                .backend
                .discover()?
                .into_iter()
                .find(|d| d.public.busid == busid)
                .context("USB device offline")?;
            server.annotate(state, &mut device);
            if !device.public.shared {
                bail!("device is not shared by this service");
            }
            if let Some(reason) = &device.public.protected_reason {
                bail!("{reason}");
            }
            if server.backend.status(&busid)? != 1 {
                bail!("kernel device unavailable or busy");
            }
            device.descriptor.encode()?;
            let token = uuid::Uuid::new_v4().to_string();
            let (cancel, receiver) = watch::channel(false);
            state.leases.insert(
                busid,
                Lease {
                    client_id: identity.0,
                    name: identity.1,
                    token: token.clone(),
                    cancel,
                },
            );
            Ok((device, token, receiver))
        })
        .await
    }
    async fn release(self: &Arc<Self>, busid: &str, token: &str) {
        let busid = busid.to_owned();
        let token = token.to_owned();
        if let Err(error) = self.blocking_state(move |server, state| {
        if state
            .leases
            .get(&busid)
            .is_some_and(|lease| lease.token == token)
        {
            if let Err(error) = server.backend.disconnect(&busid) {
                tracing::debug!(%busid, %error, "device already unavailable during cleanup");
            }
            state.leases.remove(&busid);
            tracing::info!(%busid, "USB session released");
        }
        Ok(())
        }).await {
            tracing::warn!(%error, "USB session cleanup worker failed");
        }
    }
    async fn kernel_socket(
        self: &Arc<Self>,
        busid: &str,
    ) -> Result<(TcpStream, Option<std::net::TcpStream>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let peer = TcpStream::connect(listener.local_addr()?).await?;
        peer.set_nodelay(true)?;
        let (kernel, _) = listener.accept().await?;
        kernel.set_nodelay(true)?;
        if self.backend.mock {
            // Explicit mock transport ONLY: deterministic byte echo for network
            // isolation tests. It does not pretend to execute USB URBs.
            tokio::spawn(async move {
                let (mut reader, mut writer) = kernel.into_split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
            return Ok((peer, None));
        }
        let kernel = kernel.into_std()?;
        kernel.set_nonblocking(false)?;
        let busid = busid.to_owned();
        let server = self.clone();
        let span = tracing::Span::current();
        let kernel = tokio::task::spawn_blocking(move || {
            span.in_scope(|| {
                server.backend.handoff(&busid, &kernel)?;
                Ok::<_, anyhow::Error>(kernel)
            })
        })
        .await
        .context("kernel socket handoff worker failed")??;
        Ok((peer, Some(kernel)))
    }
    pub async fn restore_all(self: &Arc<Self>) -> Result<()> {
        self.blocking_state(|server, state| {
            for lease in state.leases.values() {
                let _ = lease.cancel.send(true);
            }
            state.leases.clear();
            let records = state.config.shared.clone();
            let mut failures = Vec::new();
            for (busid, record) in records {
                match server.backend.restore(&busid, &record) {
                    Ok(()) => {
                        state.config.shared.remove(&busid);
                    }
                    Err(error) => {
                        failures.push(format!("{busid}: {error:#}"));
                    }
                }
            }
            config::save(&server.state_dir, &state.config)?;
            if !failures.is_empty() {
                bail!(
                    "some owned devices could not be restored: {}",
                    failures.join("; ")
                );
            }
            Ok(())
        })
        .await
    }
    pub async fn shutdown(self: &Arc<Self>) {
        if let Err(error) = self
            .blocking_state(|server, state| {
                state.shutting_down = true;
                for (busid, lease) in &state.leases {
                    let _ = lease.cancel.send(true);
                    if let Err(error) = server.backend.disconnect(busid) {
                        tracing::debug!(%busid,%error,"device unavailable during shutdown");
                    }
                }
                Ok(())
            })
            .await
        {
            tracing::warn!(%error, "USB shutdown worker failed");
        }
        let drain = async {
            loop {
                let empty = {
                    let state = self.state.lock().await;
                    state.leases.is_empty()
                };
                if empty {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(5), drain)
            .await
            .is_err()
        {
            tracing::warn!("session shutdown timed out; process exit will close loopback peers");
        }
    }
    fn reconcile_shared(&self, state: &mut State, devices: &[Device]) {
        if state.shutting_down {
            return;
        }
        for device in devices {
            let busid = &device.public.busid;
            let Some(record) = state.config.shared.get(busid) else {
                continue;
            };
            if state.leases.contains_key(busid)
                || !Backend::identity_matches(device, record)
                || device.public.protected_reason.is_some()
            {
                continue;
            }
            if device.driver.as_deref() == Some("usbip-host") {
                state.binding_errors.remove(busid);
                continue;
            }
            // A selected device returning at the same physical port can be
            // rebound after a Linux restart. A new/foreign driver is untouched.
            if device.driver != record.original_driver {
                continue;
            }
            match self.backend.bind(device) {
                Ok(()) => {
                    state.binding_errors.remove(busid);
                    tracing::info!(%busid,"restored selected USB sharing");
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    if state.binding_errors.get(busid) != Some(&error) {
                        tracing::warn!(%busid,%error,"selected device could not be shared");
                    }
                    state.binding_errors.insert(busid.clone(), error);
                }
            }
        }
    }
    pub async fn serve(self: Arc<Self>, listen: Option<String>) -> Result<()> {
        #[cfg(unix)]
        let _admin = admin::start(self.clone()).await?;
        // A previous process crash may leave a kernel connection resetting.
        // Only devices recorded by this service are considered.
        self.blocking_state(|server, state| {
            let devices = server.backend.discover()?;
            for device in &devices {
                if state
                    .config
                    .shared
                    .get(&device.public.busid)
                    .is_some_and(|r| Backend::identity_matches(&device, r))
                    && device.driver.as_deref() == Some("usbip-host")
                {
                    server.backend.disconnect(&device.public.busid)?;
                }
            }
            server.reconcile_shared(state, &devices);
            Ok(())
        })
        .await?;
        let listen = match listen {
            Some(listen) => listen,
            None => {
                let state = self.state.lock().await;
                state.config.listen.clone()
            }
        };
        let state_dir = self.state_dir.clone();
        let tls_config = tokio::task::spawn_blocking(move || config::tls_config(&state_dir))
            .await
            .context("TLS configuration worker failed")??;
        let acceptor = TlsAcceptor::from(Arc::new(tls_config));
        let listener = TcpListener::bind(&listen).await?;
        let slots = Arc::new(Semaphore::new(128));
        let monitor = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if let Err(error) = monitor
                    .blocking_state(|server, state| {
                        let devices = server.backend.discover().unwrap_or_default();
                        for (busid, lease) in &state.leases {
                            let valid = devices
                                .iter()
                                .find(|d| &d.public.busid == busid)
                                .is_some_and(|d| {
                                    state
                                        .config
                                        .shared
                                        .get(busid)
                                        .is_some_and(|r| Backend::identity_matches(d, r))
                                        && d.public.protected_reason.is_none()
                                });
                            if !valid || !state.config.clients.contains_key(&lease.client_id) {
                                let _ = lease.cancel.send(true);
                            }
                        }
                        server.reconcile_shared(state, &devices);
                        Ok(())
                    })
                    .await
                {
                    tracing::warn!(%error, "USB device monitor worker failed");
                }
            }
        });
        tracing::info!(%listen, mock = self.backend.mock, "USB bridge TLS listener ready");
        loop {
            let (stream, peer) = listener.accept().await?;
            let connection_id = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(connection_id, %peer, available_slots = slots.available_permits(), "TCP connection accepted");
            stream.set_nodelay(true)?;
            keepalive(&stream)?;
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                tracing::debug!(connection_id, %peer, "TCP connection rejected because all slots are occupied");
                drop(stream);
                continue;
            };
            let acceptor = acceptor.clone();
            let server = self.clone();
            let span = tracing::debug_span!("connection", connection_id, %peer);
            tokio::spawn(async move {
                let _permit = permit;
                let started = Instant::now();
                let result = async {
                    tracing::debug!("TLS handshake started");
                    let tls =
                        tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream))
                            .await??;
                    tracing::debug!(elapsed_ms = started.elapsed().as_millis(), "TLS handshake completed");
                    server.connection(tls).await
                }
                .await;
                if let Err(error) = result {
                    tracing::debug!(%error, elapsed_ms = started.elapsed().as_millis(), "connection closed with error");
                } else {
                    tracing::debug!(elapsed_ms = started.elapsed().as_millis(), "connection completed");
                }
            }.instrument(span));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn fixture() -> (tempfile::TempDir, Arc<Server>) {
        let dir = tempfile::tempdir().unwrap();
        let sys = dir.path().join("sys");
        let proc = dir.path().join("proc");
        fs::create_dir_all(sys.join("bus/usb/devices/1-2")).unwrap();
        fs::create_dir_all(proc.join("self")).unwrap();
        fs::write(proc.join("self/mountinfo"), "").unwrap();
        for (name, value) in [
            ("idVendor", "1a86"),
            ("idProduct", "5512"),
            ("bDeviceClass", "00"),
            ("busnum", "1"),
            ("devnum", "2"),
            ("usbip_status", "1"),
            ("mock-driver", "usbip-host"),
        ] {
            fs::write(sys.join("bus/usb/devices/1-2").join(name), value).unwrap();
        }
        let state = dir.path().join("state");
        config::init(&state, vec![]).unwrap();
        let backend = Backend::new(sys, proc, true).unwrap();
        let device = backend.discover().unwrap().remove(0);
        let mut config = config::load(&state).unwrap();
        config.shared.insert("1-2".into(), Backend::record(&device));
        config::save(&state, &config).unwrap();
        let server = Server::open(state, backend).unwrap();
        (dir, server)
    }
    #[tokio::test]
    async fn leases_are_exclusive_and_cleanup_allows_new_session() {
        let (_dir, server) = fixture();
        let (_, token, _) = server
            .reserve("1-2", ("client-a".into(), "A".into()))
            .await
            .unwrap();
        assert!(server
            .reserve("1-2", ("client-b".into(), "B".into()))
            .await
            .is_err());
        server.release("1-2", "stale-token").await;
        assert!(server
            .reserve("1-2", ("client-b".into(), "B".into()))
            .await
            .is_err());
        server.release("1-2", &token).await;
        assert!(server
            .reserve("1-2", ("client-b".into(), "B".into()))
            .await
            .is_ok());
    }
    #[tokio::test]
    async fn targeted_admin_release_rejects_races_and_leaves_other_device_running() {
        let (_dir, server) = fixture();
        let source = server.backend.root.join("bus/usb/devices/1-2");
        let other = server.backend.root.join("bus/usb/devices/1-3");
        fs::create_dir_all(&other).unwrap();
        for entry in fs::read_dir(&source).unwrap() {
            let entry = entry.unwrap(); fs::copy(entry.path(), other.join(entry.file_name())).unwrap();
        }
        let second = server.backend.discover().unwrap().into_iter().find(|d| d.public.busid == "1-3").unwrap();
        server.state.lock().await.config.shared.insert("1-3".into(), Backend::record(&second));
        let (_, token, mut cancelled) = server.reserve("1-2", ("a".into(), "电脑 A".into())).await.unwrap();
        let (_, other_token, other_cancelled) = server.reserve("1-3", ("b".into(), "电脑 B".into())).await.unwrap();
        let state = server.state.lock().await;
        let d = server.list(&state).unwrap().into_iter().find(|d| d.busid == "1-2").unwrap();
        let request = bridge_common::ForceReleaseRequest { server_id: state.config.server_id.clone(), busid: d.busid, session_id: token.clone(), path: d.path, vendor: d.vendor, product: d.product, serial: d.serial };
        drop(state);
        let mut wrong = request.clone(); wrong.session_id = "old session".into();
        assert!(server.force_release(wrong).await.is_err());
        let mut wrong = request.clone(); wrong.server_id = "other server".into();
        assert!(server.force_release(wrong).await.is_err());
        let mut wrong = request.clone(); wrong.product ^= 1;
        assert!(server.force_release(wrong).await.is_err());
        assert!(!*cancelled.borrow()); assert!(!*other_cancelled.borrow());
        let cleanup = server.clone();
        let task = tokio::spawn(async move { cancelled.changed().await.unwrap(); cleanup.release("1-2", &token).await; });
        server.force_release(request.clone()).await.unwrap(); task.await.unwrap();
        assert!(!*other_cancelled.borrow());
        assert_eq!(server.state.lock().await.leases.get("1-3").unwrap().token, other_token);
        assert!(server.force_release(request).await.is_err());
        server.release("1-3", &other_token).await;
    }

    #[tokio::test]
    async fn forced_release_timeout_never_reports_success() {
        let (_dir, server) = fixture();
        let (device, token, mut cancelled) = server.reserve("1-2", ("a".into(), "A".into())).await.unwrap();
        let d = device.public;
        let request = bridge_common::ForceReleaseRequest { server_id: server.state.lock().await.config.server_id.clone(), busid:d.busid,session_id:token.clone(),path:d.path,vendor:d.vendor,product:d.product,serial:d.serial };
        let result = server.force_release(request).await;
        assert!(result.unwrap_err().to_string().contains("尚未确认"));
        assert!(*cancelled.borrow_and_update());
        assert!(server.state.lock().await.leases.contains_key("1-2"));
        server.release("1-2", &token).await;
    }

    #[test]
    fn duplicate_state_process_is_rejected() {
        let (_dir, server) = fixture();
        assert!(Server::open(server.state_dir.clone(), server.backend.clone()).is_err());
    }
    #[tokio::test]
    async fn shutdown_cancels_leases_and_preserves_selected_shares() {
        let (_dir, server) = fixture();
        let (_, token, mut cancelled) = server
            .reserve("1-2", ("A".into(), "A".into()))
            .await
            .unwrap();
        let session_server = server.clone();
        let session = tokio::spawn(async move {
            cancelled.changed().await.unwrap();
            session_server.release("1-2", &token).await;
        });
        server.shutdown().await;
        session.await.unwrap();
        assert!(server.state.lock().await.leases.is_empty());
        assert!(config::load(&server.state_dir)
            .unwrap()
            .shared
            .contains_key("1-2"));
        assert!(server
            .reserve("1-2", ("B".into(), "B".into()))
            .await
            .is_err());
    }
    #[tokio::test]
    async fn selected_device_rebind_requires_the_same_identity_and_original_driver() {
        let (_dir, server) = fixture();
        let path = server.backend.root.join("bus/usb/devices/1-2");
        fs::write(path.join("mock-driver"), "usb").unwrap();
        for (driver, attributes) in [
            ("usb", vec!["bind", "unbind"]),
            ("usbip-host", vec!["bind", "match_busid"]),
        ] {
            let directory = server.backend.root.join("bus/usb/drivers").join(driver);
            fs::create_dir_all(&directory).unwrap();
            for attribute in attributes {
                fs::write(directory.join(attribute), "").unwrap();
            }
        }
        let devices = server.backend.discover().unwrap();
        let mut state = server.state.lock().await;
        state
            .config
            .shared
            .insert("1-2".into(), Backend::record(&devices[0]));
        server.reconcile_shared(&mut state, &devices);
        assert_eq!(
            server.backend.discover().unwrap()[0].driver.as_deref(),
            Some("usbip-host")
        );
        fs::write(path.join("mock-driver"), "foreign").unwrap();
        server.reconcile_shared(&mut state, &server.backend.discover().unwrap());
        assert_eq!(
            fs::read_to_string(path.join("mock-driver")).unwrap(),
            "foreign"
        );
        fs::write(path.join("mock-driver"), "usb").unwrap();
        fs::write(path.join("idProduct"), "5513").unwrap();
        server.reconcile_shared(&mut state, &server.backend.discover().unwrap());
        assert_eq!(fs::read_to_string(path.join("mock-driver")).unwrap(), "usb");
    }
    #[tokio::test]
    async fn failed_intent_save_preserves_previous_state_and_cannot_trigger_monitor_bind() {
        let (_dir, server) = fixture();
        let path = server.backend.root.join("bus/usb/devices/1-2");
        fs::write(path.join("mock-driver"), "usb").unwrap();
        for (driver, attributes) in [
            ("usb", vec!["bind", "unbind"]),
            ("usbip-host", vec!["bind", "match_busid"]),
        ] {
            let directory = server.backend.root.join("bus/usb/drivers").join(driver);
            fs::create_dir_all(&directory).unwrap();
            for attribute in attributes {
                fs::write(directory.join(attribute), "").unwrap();
            }
        }
        let original = Backend::record(&server.backend.discover().unwrap()[0]);
        let config_path = server.state_dir.join("config.json");
        let backup = server.state_dir.join("saved-config.json");
        for previous in [None, Some(original.clone())] {
            {
                let mut state = server.state.lock().await;
                state.config.shared.clear();
                if let Some(record) = &previous {
                    state.config.shared.insert("1-2".into(), record.clone());
                }
                config::save(&server.state_dir, &state.config).unwrap();
            }
            // The second case models an earlier incomplete recovery. A new
            // intent must not overwrite the saved usb driver with this None.
            fs::write(
                path.join("mock-driver"),
                if previous.is_some() { "" } else { "usb" },
            )
            .unwrap();
            let committed = fs::read(&config_path).unwrap();
            fs::rename(&config_path, &backup).unwrap();
            fs::create_dir(&config_path).unwrap();
            let response = server
                .control(
                    ControlRequest::SetShared {
                        busid: "1-2".into(),
                        shared: true,
                    },
                    Some(("client-a".into(), "A".into())),
                )
                .await;
            assert!(!response.ok);
            let error = response.error.unwrap();
            assert!(
                error.contains("保存 USB 恢复记录失败；尚未开始更改设备驱动"),
                "{error}"
            );
            let devices = server.backend.discover().unwrap();
            {
                let mut state = server.state.lock().await;
                assert_eq!(
                    serde_json::to_value(state.config.shared.get("1-2")).unwrap(),
                    serde_json::to_value(&previous).unwrap(),
                );
                server.reconcile_shared(&mut state, &devices);
            }
            assert_eq!(
                fs::read_to_string(path.join("mock-driver")).unwrap(),
                if previous.is_some() { "" } else { "usb" },
            );
            for (driver, attribute) in [
                ("usb", "unbind"),
                ("usbip-host", "bind"),
                ("usbip-host", "match_busid"),
            ] {
                assert!(fs::read(
                    server
                        .backend
                        .root
                        .join("bus/usb/drivers")
                        .join(driver)
                        .join(attribute)
                )
                .unwrap()
                .is_empty());
            }
            assert_eq!(fs::read(&backup).unwrap(), committed);
            fs::remove_dir(&config_path).unwrap();
            fs::rename(&backup, &config_path).unwrap();
            assert_eq!(fs::read(&config_path).unwrap(), committed);
            assert_eq!(
                serde_json::to_value(config::load(&server.state_dir).unwrap().shared.get("1-2"))
                    .unwrap(),
                serde_json::to_value(previous).unwrap(),
            );
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn failed_bind_and_restore_preserve_recovery_record_until_restore_succeeds() {
        use std::os::unix::fs::symlink;

        let (dir, server) = fixture();
        let path = server.backend.root.join("bus/usb/devices/1-2");
        fs::write(path.join("serial"), "recovery-fixture-001").unwrap();
        let driver_file = dir.path().join("original-driver");
        fs::write(&driver_file, "usb").unwrap();
        fs::remove_file(path.join("mock-driver")).unwrap();
        // Reading this sandbox file discovers the original driver, while the
        // mock backend refuses to write through a symlink outside its sysfs root.
        symlink(&driver_file, path.join("mock-driver")).unwrap();
        for (driver, attributes) in [
            ("usb", vec!["bind", "unbind"]),
            ("usbip-host", vec!["match_busid"]),
        ] {
            let directory = server.backend.root.join("bus/usb/drivers").join(driver);
            fs::create_dir_all(&directory).unwrap();
            for attribute in attributes {
                fs::write(directory.join(attribute), "").unwrap();
            }
        }
        // Missing usbip-host/bind forces a bind failure after original unbind;
        // the outside-root mock-driver symlink independently forces restore to fail.
        {
            let mut state = server.state.lock().await;
            state.config.shared.clear();
            config::save(&server.state_dir, &state.config).unwrap();
        }
        let response = server
            .control(
                ControlRequest::SetShared {
                    busid: "1-2".into(),
                    shared: true,
                },
                Some(("client-a".into(), "A".into())),
            )
            .await;
        assert!(!response.ok);
        let error = response.error.unwrap();
        assert!(error.contains("USB 共享绑定失败"), "{error}");
        assert!(error.contains("原驱动恢复失败，恢复记录已保留"), "{error}");
        assert!(error.contains("usbip-host/bind"), "{error}");
        assert!(error.contains("symlink escapes selected root"), "{error}");
        assert_eq!(fs::read_to_string(&driver_file).unwrap(), "usb");
        assert_eq!(
            fs::read_to_string(server.backend.root.join("bus/usb/drivers/usb/unbind")).unwrap(),
            "1-2"
        );
        let persisted = config::load(&server.state_dir).unwrap();
        let record = persisted.shared.get("1-2").unwrap();
        assert_eq!(record.vendor, 0x1a86);
        assert_eq!(record.product, 0x5512);
        assert_eq!(record.serial.as_deref(), Some("recovery-fixture-001"));
        assert_eq!(
            record.path,
            fs::canonicalize(&path).unwrap().to_string_lossy()
        );
        assert_eq!(record.original_driver.as_deref(), Some("usb"));
        let in_memory = server.state.lock().await.config.shared["1-2"].clone();
        assert_eq!(
            serde_json::to_value(record).unwrap(),
            serde_json::to_value(in_memory).unwrap()
        );

        // A retry while the failed bind left driver=None must keep the original
        // usb record, even when both bind and restoration fail again.
        fs::write(&driver_file, "").unwrap();
        let retry = server
            .control(
                ControlRequest::SetShared {
                    busid: "1-2".into(),
                    shared: true,
                },
                Some(("client-a".into(), "A".into())),
            )
            .await;
        assert!(!retry.ok);
        assert!(retry.error.unwrap().contains("恢复记录已保留"));
        let after_retry = config::load(&server.state_dir).unwrap();
        assert_eq!(
            serde_json::to_value(&after_retry.shared["1-2"]).unwrap(),
            serde_json::to_value(record).unwrap(),
        );
        assert_eq!(
            server.state.lock().await.config.shared["1-2"]
                .original_driver
                .as_deref(),
            Some("usb")
        );

        // Repair only the fake attribute, then the normal rollback API can use
        // the saved original driver and clear its record after successful restore.
        fs::remove_file(path.join("mock-driver")).unwrap();
        fs::write(path.join("mock-driver"), "").unwrap();
        server.restore_all().await.unwrap();
        assert_eq!(fs::read_to_string(path.join("mock-driver")).unwrap(), "usb");
        assert!(server.state.lock().await.config.shared.is_empty());
        assert!(config::load(&server.state_dir).unwrap().shared.is_empty());

        // A bind failure whose restore succeeds must still remove its record.
        let response = server
            .control(
                ControlRequest::SetShared {
                    busid: "1-2".into(),
                    shared: true,
                },
                Some(("client-a".into(), "A".into())),
            )
            .await;
        assert!(!response.ok);
        let error = response.error.unwrap();
        assert!(error.contains("原驱动已恢复"), "{error}");
        assert!(!error.contains("恢复记录已保留"), "{error}");
        assert_eq!(fs::read_to_string(path.join("mock-driver")).unwrap(), "usb");
        assert!(server.state.lock().await.config.shared.is_empty());
        assert!(config::load(&server.state_dir).unwrap().shared.is_empty());
    }
    #[tokio::test]
    async fn unpaired_control_cannot_read_or_share_and_pair_code_rotates() {
        let (_dir, server) = fixture();
        assert!(!server.control(ControlRequest::ListDevices, None).await.ok);
        assert!(
            !server
                .control(
                    ControlRequest::SetShared {
                        busid: "1-2".into(),
                        shared: false
                    },
                    None
                )
                .await
                .ok
        );
        let code = server.state.lock().await.config.pair_code.clone();
        let response = server
            .control(
                ControlRequest::Pair {
                    code: code.clone(),
                    name: "A".into(),
                },
                None,
            )
            .await;
        assert!(response.ok && response.pairing.is_some());
        assert!(
            !server
                .control(
                    ControlRequest::Pair {
                        code,
                        name: "B".into()
                    },
                    None
                )
                .await
                .ok
        );
    }
    #[tokio::test]
    async fn invitations_require_enrollment_expire_and_are_consumed_once() {
        let (_dir, server) = fixture();
        assert!(!server.control(ControlRequest::CreateInvitation, None).await.ok);
        assert!(!server.control(ControlRequest::CreateInvitation, Some(("unknown".into(), "unknown".into()))).await.ok);
        let code = server.state.lock().await.config.pair_code.clone();
        let owner = server.control(ControlRequest::Pair { code, name: "My PC A".into() }, None).await.pairing.unwrap();
        let identity = Some((owner.client_id, "My PC A".into()));
        let invitation = server.control(ControlRequest::CreateInvitation, identity.clone()).await.invitation.unwrap();
        let key = hex::encode(Sha256::digest(invitation.code.as_bytes()));
        assert!(config::load(&server.state_dir).unwrap().invitations.contains_key(&key));
        assert!(!fs::read_to_string(server.state_dir.join("config.json")).unwrap().contains(&invitation.code));
        let next = server.control(ControlRequest::Pair { code: invitation.code.clone(), name: "My PC B".into() }, None).await;
        assert!(next.ok && next.pairing.is_some());
        assert!(!server.control(ControlRequest::Pair { code: invitation.code, name: "Replay".into() }, None).await.ok);
        let expired = server.control(ControlRequest::CreateInvitation, identity).await.invitation.unwrap();
        let key = hex::encode(Sha256::digest(expired.code.as_bytes()));
        server.state.lock().await.config.invitations.get_mut(&key).unwrap().expires_at = 0;
        assert!(!server.control(ControlRequest::Pair { code: expired.code, name: "Late".into() }, None).await.ok);
    }
    async fn test_tls(
        address: &str,
        state_dir: &std::path::Path,
        bundle: Option<&bridge_common::PairingBundle>,
    ) -> tokio_rustls::client::TlsStream<TcpStream> {
        let ca = fs::read(state_dir.join("ca.pem")).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        for certificate in rustls_pemfile::certs(&mut std::io::Cursor::new(ca)) {
            roots.add(certificate.unwrap()).unwrap();
        }
        let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots);
        let config = if let Some(bundle) = bundle {
            let certs =
                rustls_pemfile::certs(&mut std::io::Cursor::new(bundle.client_cert_pem.as_bytes()))
                    .collect::<std::io::Result<Vec<_>>>()
                    .unwrap();
            let key = rustls_pemfile::private_key(&mut std::io::Cursor::new(
                bundle.client_key_pem.as_bytes(),
            ))
            .unwrap()
            .unwrap();
            builder.with_client_auth_cert(certs, key).unwrap()
        } else {
            builder.with_no_client_auth()
        };
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(
                rustls::pki_types::ServerName::try_from("usb-bridge").unwrap(),
                TcpStream::connect(address).await.unwrap(),
            )
            .await
            .unwrap()
    }
    async fn test_control(
        address: &str,
        server: &Server,
        bundle: Option<&bridge_common::PairingBundle>,
        request: ControlRequest,
    ) -> ControlResponse {
        let mut stream = test_tls(address, &server.state_dir, bundle).await;
        stream
            .write_all(bridge_common::CONTROL_SELECTOR)
            .await
            .unwrap();
        bridge_common::write_json(&mut stream, &request)
            .await
            .unwrap();
        bridge_common::read_json(&mut stream).await.unwrap()
    }
    #[tokio::test(flavor = "current_thread")]
    async fn established_tls_data_stream_continues_during_blocking_persistence() {
        let (_dir, server) = fixture();
        let code = server.state.lock().await.config.pair_code.clone();
        let bundle = server
            .control(
                ControlRequest::Pair {
                    code,
                    name: "blocking-persistence-test".into(),
                },
                None,
            )
            .await
            .pairing
            .unwrap();
        let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap().to_string();
        drop(socket);
        let running = {
            let server = server.clone();
            let address = address.clone();
            tokio::spawn(async move { server.serve(Some(address)).await })
        };
        for attempt in 0..200 {
            if TcpStream::connect(&address).await.is_ok() {
                break;
            }
            assert!(attempt < 199, "test listener did not start");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut data = test_tls(&address, &server.state_dir, Some(&bundle)).await;
        data.write_all(bridge_common::USBIP_SELECTOR).await.unwrap();
        data.write_all(&UsbIpHeader::request(OP_REQ_IMPORT).encode())
            .await
            .unwrap();
        let mut busid = [0; 32];
        busid[..3].copy_from_slice(b"1-2");
        data.write_all(&busid).await.unwrap();
        let mut header = [0; 8];
        data.read_exact(&mut header).await.unwrap();
        assert_eq!(UsbIpHeader::decode(header).status, 0);
        let mut descriptor = [0; 312];
        data.read_exact(&mut descriptor).await.unwrap();

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let pending_persistence = {
            let server = server.clone();
            tokio::spawn(async move {
                server
                    .blocking_state(move |_server, _state| {
                        entered_tx.send(()).unwrap();
                        // Deterministically stand in for a slow durable write while
                        // retaining the very same transaction guard used by control.
                        release_rx.recv_timeout(Duration::from_secs(5))?;
                        Ok(())
                    })
                    .await
            })
        };
        entered_rx.await.unwrap();
        assert!(
            server.state.try_lock().is_err(),
            "test must hold the management lock"
        );
        let expected: Vec<u8> = (0..65536).map(|i| (i % 251) as u8).collect();
        let started = Instant::now();
        let transfer = tokio::time::timeout(Duration::from_secs(1), async {
            data.write_all(&expected).await?;
            data.flush().await?;
            let mut echoed = vec![0; expected.len()];
            data.read_exact(&mut echoed).await?;
            Ok::<_, std::io::Error>(echoed)
        })
        .await;
        let elapsed = started.elapsed();
        release_tx.send(()).unwrap();
        pending_persistence.await.unwrap().unwrap();
        assert_eq!(transfer.unwrap().unwrap(), expected);
        assert!(
            elapsed < Duration::from_secs(1),
            "data was stalled for {elapsed:?}"
        );
        drop(data);
        server.shutdown().await;
        running.abort();
    }
    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_management_caller_keeps_process_lock_until_worker_finishes() {
        let (dir, server) = fixture();
        let state_dir = server.state_dir.clone();
        let backend = server.backend.clone();
        let finished_file = dir.path().join("completed-worker");
        let worker_file = finished_file.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let caller = {
            let server = server.clone();
            tokio::spawn(async move {
                server
                    .blocking_state(move |_server, _state| {
                        entered_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5))?;
                        fs::write(worker_file, b"transaction finished")?;
                        finished_tx.send(()).unwrap();
                        Ok(())
                    })
                    .await
            })
        };
        entered_rx.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        drop(server);
        assert!(Server::open(state_dir.clone(), backend.clone()).is_err());
        assert!(!finished_file.exists());
        release_tx.send(()).unwrap();
        finished_rx.await.unwrap();
        assert_eq!(fs::read(&finished_file).unwrap(), b"transaction finished");
        // Completion is signalled just before the worker drops its owned guard
        // and Server. Wait for those destructors, rather than assuming scheduling.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if Server::open(state_dir.clone(), backend.clone()).is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn tls_pairing_enrollment_and_usbip_import_are_enforced_over_real_tcp() {
        let (_dir, server) = fixture();
        let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap().to_string();
        drop(socket);
        let running = {
            let server = server.clone();
            let address = address.clone();
            tokio::spawn(async move { server.serve(Some(address)).await })
        };
        for attempt in 0..100 {
            if TcpStream::connect(&address).await.is_ok() {
                break;
            }
            assert!(attempt < 99, "test listener did not start");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !test_control(&address, &server, None, ControlRequest::ListDevices)
                .await
                .ok
        );
        let code = server.state.lock().await.config.pair_code.clone();
        let response = test_control(
            &address,
            &server,
            None,
            ControlRequest::Pair {
                code,
                name: "TLS client".into(),
            },
        )
        .await;
        let bundle = response.pairing.unwrap();
        assert!(!test_control(&address, &server, None, ControlRequest::CreateInvitation).await.ok);
        let invitation = test_control(&address, &server, Some(&bundle), ControlRequest::CreateInvitation).await.invitation.unwrap();
        let second = test_control(&address, &server, None, ControlRequest::Pair { code: invitation.code.clone(), name: "Invited PC".into() }).await;
        assert!(second.ok && second.pairing.is_some());
        assert!(!test_control(&address, &server, None, ControlRequest::Pair { code: invitation.code, name: "Replay".into() }).await.ok);
        assert!(
            test_control(
                &address,
                &server,
                Some(&bundle),
                ControlRequest::ListDevices
            )
            .await
            .ok
        );
        // A CA-signed certificate which was never enrolled cannot manage devices.
        let (unenrolled, _) = config::issue_client(&server.state_dir, "unenrolled").unwrap();
        assert!(
            !test_control(
                &address,
                &server,
                Some(&unenrolled),
                ControlRequest::ListDevices
            )
            .await
            .ok
        );
        let mut data = test_tls(&address, &server.state_dir, Some(&bundle)).await;
        data.write_all(bridge_common::USBIP_SELECTOR).await.unwrap();
        data.write_all(&UsbIpHeader::request(OP_REQ_IMPORT).encode())
            .await
            .unwrap();
        let mut busid = [0; 32];
        busid[..3].copy_from_slice(b"1-2");
        data.write_all(&busid).await.unwrap();
        let mut header = [0; 8];
        data.read_exact(&mut header).await.unwrap();
        assert_eq!(UsbIpHeader::decode(header).status, 0);
        let mut descriptor = [0; 312];
        data.read_exact(&mut descriptor).await.unwrap();
        assert_eq!(&descriptor[300..304], &[0x1a, 0x86, 0x55, 0x12]);
        let bytes: Vec<u8> = (0..4096).map(|n| (n % 251) as u8).collect();
        data.write_all(&bytes).await.unwrap();
        data.flush().await.unwrap();
        let mut echoed = vec![0; bytes.len()];
        data.read_exact(&mut echoed).await.unwrap();
        assert_eq!(bytes, echoed);
        let devices = test_control(
            &address,
            &server,
            Some(&bundle),
            ControlRequest::ListDevices,
        )
        .await
        .devices;
        assert_eq!(devices[0].owner.as_deref(), Some("TLS client"));
        drop(data);
        for _ in 0..100 {
            if server.state.lock().await.leases.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(server.state.lock().await.leases.is_empty());
        running.abort();
    }
}
