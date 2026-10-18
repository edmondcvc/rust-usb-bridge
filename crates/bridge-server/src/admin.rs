//! Root-only local management. No administrative operation on the TLS client API.
use crate::Server;
use anyhow::{bail, Context, Result};
use bridge_common::ForceReleaseRequest;
use std::{sync::Arc, time::Duration};

impl Server {
    pub async fn force_release(self: &Arc<Self>, request: ForceReleaseRequest) -> Result<()> {
        if !bridge_common::valid_busid(&request.busid) || request.session_id.is_empty() {
            bail!("无效的设备或会话，请刷新后重试");
        }
        let busid = request.busid.clone();
        let token = request.session_id.clone();
        self.blocking_state(move |server, state| {
            if state.config.server_id != request.server_id { bail!("Linux 主机身份不匹配"); }
            let device = server.backend.discover()?.into_iter().find(|d| d.public.busid == request.busid)
                .context("设备已拔出，请刷新后重试")?;
            let d = &device.public;
            if d.path != request.path || d.vendor != request.vendor || d.product != request.product || d.serial != request.serial {
                bail!("设备身份发生变化，请刷新后重试");
            }
            if !state.config.shared.get(&request.busid).is_some_and(|r| crate::sysfs::Backend::identity_matches(&device, r))
                || device.driver.as_deref() != Some("usbip-host") { bail!("此设备不由本软件管理"); }
            let lease = state.leases.get(&request.busid).context("设备占用已变化，请刷新后重试")?;
            if lease.token != request.session_id { bail!("使用者或会话已变化，请刷新后重试"); }
            lease.cancel.send(true).context("设备会话正在结束，请刷新后重试")?;
            tracing::warn!(busid = %request.busid, owner = %lease.name, "administrator requested targeted USB release");
            Ok(())
        }).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let state = self.state.lock().await;
                if !state.leases.get(&busid).is_some_and(|l| l.token == token) {
                    // A new lease also proves the selected generation ended.
                    if state.leases.contains_key(&busid) { return; }
                    drop(state);
                    let id = busid.clone();
                    let done = self.blocking_state(move |server, _| {
                        let present = server.backend.discover()?.iter().any(|d| d.public.busid == id);
                        Ok(!present || server.backend.status(&id)? == 1)
                    }).await.unwrap_or(false);
                    if done { return; }
                } else { drop(state); }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }).await.context("断开请求已发送，但 Linux 释放尚未确认，请刷新状态")?;
        Ok(())
    }
}

#[cfg(unix)]
pub struct ListenerTask(tokio::task::JoinHandle<()>);
#[cfg(unix)]
impl Drop for ListenerTask { fn drop(&mut self) { self.0.abort(); } }

#[cfg(unix)]
pub async fn start(server: Arc<Server>) -> Result<ListenerTask> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use tokio::net::UnixListener;
    let path = server.state_dir.join("admin.sock");
    // state_dir is root-owned 0700 in production; server.lock excludes another instance.
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if !meta.file_type().is_socket() { bail!("admin.sock is not a socket"); }
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(ListenerTask(tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { break; };
            if !stream.peer_cred().is_ok_and(|c| c.uid() == 0) { continue; }
            let server = server.clone();
            // Serial management prevents an unbounded number of root-request tasks.
            let _ = tokio::time::timeout(Duration::from_secs(20), async {
                let request: ForceReleaseRequest = bridge_common::read_json(&mut stream).await?;
                let reply = match server.force_release(request).await {
                    Ok(()) => serde_json::json!({"ok":true}),
                    Err(e) => serde_json::json!({"ok":false,"error":format!("{e:#}")}),
                };
                bridge_common::write_json(&mut stream, &reply).await
            }).await;
        }
    })))
}

#[cfg(unix)]
pub async fn command(state_dir: std::path::PathBuf) -> Result<()> {
    use std::io::Read;
    crate::config::require_root(false)?;
    let mut text = String::new();
    std::io::stdin().take(32_001).read_to_string(&mut text)?;
    if text.len() > 32_000 { bail!("request too large"); }
    let request: ForceReleaseRequest = serde_json::from_str(&text)?;
    let reply: serde_json::Value = tokio::time::timeout(Duration::from_secs(20), async {
        let mut stream = tokio::net::UnixStream::connect(state_dir.join("admin.sock")).await
            .context("Linux 服务未运行或版本不支持，请升级 Linux 服务端")?;
        bridge_common::write_json(&mut stream, &request).await?;
        Ok::<_, anyhow::Error>(bridge_common::read_json(&mut stream).await?)
    }).await.context("管理员操作超时，释放状态待确认")??;
    println!("{}", reply);
    if reply["ok"] != true { bail!("{}", reply["error"].as_str().unwrap_or("释放未确认")); }
    Ok(())
}
