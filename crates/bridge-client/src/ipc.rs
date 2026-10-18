use crate::{
    backend::{Backend, LocalRequest, LocalResponse},
    network,
};
use anyhow::{Context, Result};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

/// Production always authenticates the SCM service process. Diagnostics require
/// a separate, explicitly supplied process ID and never use the production pipe.
#[derive(Clone, Copy, Debug, Default)]
pub enum Target {
    #[default]
    Service,
    Diagnostic(u32),
}

#[cfg(windows)]
fn diagnostic_name(pid: u32) -> String {
    format!(r"\\.\pipe\RustUsbBridge.Diagnostic.{pid}")
}

#[cfg(windows)]
pub async fn request(request: &LocalRequest) -> Result<LocalResponse> {
    request_to(Target::Service, request).await
}

#[cfg(windows)]
fn service_pid() -> Result<u32> {
    use windows_service::{
        service::{ServiceAccess, ServiceState},
        service_manager::{ServiceManager, ServiceManagerAccess},
    };
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("无法核验 Windows 服务管理器身份")?;
    let service = manager
        .open_service(crate::SERVICE_NAME, ServiceAccess::QUERY_STATUS)
        .context("USB 正式后台服务未安装；请运行安装程序")?;
    let status = service.query_status()?;
    if status.current_state != ServiceState::Running {
        anyhow::bail!("USB 正式后台服务未运行；请启动 UsbBridgeRust 服务");
    }
    status
        .process_id
        .filter(|pid| *pid != 0)
        .context("Windows 服务管理器没有返回后台进程身份")
}

#[cfg(windows)]
fn verify_pipe_pid(
    pipe: &tokio::net::windows::named_pipe::NamedPipeClient,
    expected: u32,
) -> Result<()> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetNamedPipeServerProcessId(pipe: *mut c_void, process: *mut u32) -> i32;
    }
    let mut actual = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut actual) } == 0 {
        return Err(std::io::Error::last_os_error()).context("无法核验 USB 控制管道进程身份");
    }
    if expected == 0 || actual != expected {
        anyhow::bail!(
            "USB 控制管道身份不匹配：期望进程 {expected}，实际进程 {actual}；未发送控制请求"
        );
    }
    Ok(())
}

#[cfg(windows)]
pub async fn request_to(target: Target, request: &LocalRequest) -> Result<LocalResponse> {
    let (name, expected_pid) = match target {
        Target::Service => (crate::PIPE_NAME.to_owned(), service_pid()?),
        Target::Diagnostic(pid) if pid != 0 => (diagnostic_name(pid), pid),
        Target::Diagnostic(_) => anyhow::bail!("诊断后台进程 ID 必须为正数"),
    };
    exchange_verified(
        &name,
        expected_pid,
        matches!(target, Target::Service),
        request,
    )
    .await
}

#[cfg(windows)]
async fn exchange_verified(
    name: &str,
    expected_pid: u32,
    production: bool,
    request: &LocalRequest,
) -> Result<LocalResponse> {
    tokio::time::timeout(Duration::from_secs(120), async {
        let start = std::time::Instant::now();
        let mut pipe = loop {
            match crate::security::open_pipe_client(name) {
                Ok(pipe) => break pipe,
                Err(error)
                    if error.raw_os_error() == Some(231)
                        && start.elapsed() < Duration::from_secs(5) =>
                {
                    tokio::time::sleep(Duration::from_millis(100)).await
                }
                Err(error) => {
                    return Err(error).context("USB 后台服务未运行，请使用安装程序安装并启动服务")
                }
            }
        };
        verify_pipe_pid(&pipe, expected_pid)?;
        // Re-read SCM after opening to reject a stopped/replaced service during
        // connection. No pairing code, device mutation or other payload has
        // reached the pipe before both identity checks pass.
        if production && service_pid()? != expected_pid {
            anyhow::bail!("USB 后台服务在连接期间更换了进程；请重试");
        }
        network::write_frame(&mut pipe, request).await?;
        network::read_frame(&mut pipe).await
    })
    .await
    .context("客户端后台操作超时；操作结果尚未确认，请刷新设备状态后再操作")?
}

#[cfg(windows)]
pub async fn serve(backend: Arc<Backend>, stop: watch::Receiver<bool>) -> Result<()> {
    serve_named(backend, stop, crate::PIPE_NAME.to_owned(), false).await
}

#[cfg(windows)]
pub async fn serve_diagnostic(backend: Arc<Backend>, stop: watch::Receiver<bool>) -> Result<()> {
    serve_named(backend, stop, diagnostic_name(std::process::id()), true).await
}

#[cfg(windows)]
async fn serve_named(
    backend: Arc<Backend>,
    mut stop: watch::Receiver<bool>,
    name: String,
    diagnostic: bool,
) -> Result<()> {
    let mut pipe = crate::security::create_named_pipe(&name, true, diagnostic)?;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            result = pipe.connect() => result.context("控制管道连接失败")?,
        }
        // Keep a listening instance alive before moving the connected instance
        // to a task. This prevents pipe-name hijacking between requests.
        let next = crate::security::create_named_pipe(&name, false, diagnostic)?;
        let mut connection = std::mem::replace(&mut pipe, next);
        let backend = backend.clone();
        tokio::spawn(async move {
            let result: Result<()> = async {
                let request: LocalRequest = tokio::time::timeout(
                    Duration::from_secs(10),
                    network::read_frame(&mut connection),
                )
                .await
                .context("控制请求读取超时")??;
                let response = backend.handle(request).await;
                tokio::time::timeout(
                    Duration::from_secs(10),
                    network::write_frame(&mut connection, &response),
                )
                .await
                .context("控制响应写入超时")??;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                tracing::debug!(%error, "控制管道请求结束");
            }
        });
    }
    Ok(())
}

#[cfg(not(windows))]
pub async fn request(_: &LocalRequest) -> Result<LocalResponse> {
    anyhow::bail!("本地桌面服务 IPC 只在 Windows 上提供；Linux 上请使用 --list/--pair --direct");
}
#[cfg(not(windows))]
pub async fn request_to(_: Target, request: &LocalRequest) -> Result<LocalResponse> {
    self::request(request).await
}
#[cfg(not(windows))]
pub async fn serve(_: Arc<Backend>, _: watch::Receiver<bool>) -> Result<()> {
    anyhow::bail!("Windows 后台服务只能在 Windows 上运行");
}
#[cfg(not(windows))]
pub async fn serve_diagnostic(backend: Arc<Backend>, stop: watch::Receiver<bool>) -> Result<()> {
    serve(backend, stop).await
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn test_name(label: &str) -> String {
        format!(
            r"\\.\pipe\RustUsbBridge.Test.{label}.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[tokio::test]
    async fn verified_pid_accepts_normal_framed_communication() {
        let name = test_name("exchange");
        let mut server = crate::security::create_named_pipe(&name, true, false).unwrap();
        let task = tokio::spawn(async move {
            server.connect().await.unwrap();
            let request: LocalRequest = network::read_frame(&mut server).await.unwrap();
            assert!(matches!(request, LocalRequest::Snapshot));
            let response = LocalResponse {
                ok: true,
                error: None,
                snapshot: None,
                log: Some("普通用户通信正常".into()),
                invitation: None,
            };
            network::write_frame(&mut server, &response).await.unwrap();
            // Keep the instance until the client consumes the reply.
            let mut closed = [0];
            let _ = server.read(&mut closed).await;
        });
        let response = exchange_verified(&name, std::process::id(), false, &LocalRequest::Snapshot)
            .await
            .unwrap();
        assert!(response.ok);
        assert_eq!(response.log.as_deref(), Some("普通用户通信正常"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn wrong_server_pid_receives_no_control_payload() {
        let name = test_name("identity");
        let mut server = crate::security::create_named_pipe(&name, true, false).unwrap();
        let reader = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut first = [0u8; 1];
            match tokio::time::timeout(Duration::from_secs(2), server.read(&mut first))
                .await
                .unwrap()
            {
                Ok(n) => assert_eq!(
                    n, 0,
                    "a secret or control payload reached a mismatched server"
                ),
                Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe),
            }
        });
        let error = exchange_verified(
            &name,
            std::process::id().wrapping_add(1),
            false,
            &LocalRequest::Pair {
                host: "192.0.2.10".into(),
                port: 3241,
                server_name: "usb-bridge".into(),
                name: "test".into(),
                ca_pem: "private trust".into(),
                code: "must never reach pipe".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("身份不匹配"));
        reader.await.unwrap();
    }

    #[test]
    fn diagnostic_names_never_use_production_pipe() {
        assert_ne!(diagnostic_name(12345), crate::PIPE_NAME);
        assert_ne!(diagnostic_name(12345), diagnostic_name(54321));
    }
}
