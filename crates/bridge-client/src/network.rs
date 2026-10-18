use crate::config::{self, ClientConfig};
use anyhow::{bail, Context, Result};
use bridge_common::{ControlRequest, ControlResponse};
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, ServerName},
    ClientConfig as TlsConfig, RootCertStore,
};
use std::{
    io::BufReader,
    path::Path,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{client::TlsStream, TlsConnector};

pub const MAX_FRAME: usize = 1024 * 1024;
// The target's system USB flash drive has measured 22.479s fsync stalls.
// A durable configuration save contains multiple file/directory syncs.
pub const MANAGEMENT_TIMEOUT: Duration = Duration::from_secs(90);

const STAGES: [&str; 10] = [
    "开始请求",
    "建立 TCP 连接",
    "TLS 握手",
    "写入协议选择",
    "写入请求长度",
    "写入请求数据",
    "flush 请求",
    "读取响应长度",
    "读取响应数据",
    "验证响应",
];

struct ControlProgress {
    operation: &'static str,
    stage: AtomicU8,
    started: Instant,
}

impl ControlProgress {
    fn new(request: &ControlRequest) -> Self {
        let operation = match request {
            ControlRequest::Pair { .. } => "pair",
            ControlRequest::CreateInvitation => "create_invitation",
            ControlRequest::ListDevices => "list_devices",
            ControlRequest::Status => "status",
            ControlRequest::SetShared { shared: true, .. } => "enable_shared",
            ControlRequest::SetShared { shared: false, .. } => "disable_shared",
        };
        Self {
            operation,
            stage: AtomicU8::new(0),
            started: Instant::now(),
        }
    }
    fn advance(&self, stage: u8) {
        self.stage.store(stage, Ordering::Relaxed);
        tracing::debug!(
            operation = self.operation,
            stage = STAGES[stage as usize],
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "management stage"
        );
    }
    fn name(&self) -> &'static str {
        STAGES[self.stage.load(Ordering::Relaxed) as usize]
    }
}

async fn stage_timeout<T>(
    duration: Duration,
    progress: &ControlProgress,
    operation: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    match tokio::time::timeout(duration, operation).await {
        Ok(result) => {
            result.with_context(|| format!("Linux 管理请求失败（阶段：{}）", progress.name()))
        }
        Err(_) => {
            tracing::warn!(
                operation = progress.operation,
                stage = progress.name(),
                elapsed_ms = progress.started.elapsed().as_millis() as u64,
                "management deadline expired"
            );
            bail!(
                "Linux 管理请求超时（阶段：{}，耗时 {} 毫秒）；操作结果尚未确认，请刷新设备状态后再操作",
                progress.name(),
                progress.started.elapsed().as_millis()
            );
        }
    }
}

pub fn certificates(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    let certs =
        rustls_pemfile::certs(&mut BufReader::new(pem)).collect::<std::io::Result<Vec<_>>>()?;
    if certs.is_empty() {
        bail!("CA 或客户端证书没有 PEM 证书内容");
    }
    Ok(certs)
}

pub fn make_tls(ca: &[u8], identity: Option<(&[u8], &[u8])>) -> Result<Arc<TlsConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    for cert in certificates(ca)? {
        roots.add(cert).context("CA 证书无效")?;
    }
    let builder = TlsConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots);
    let config = if let Some((cert, key)) = identity {
        let key: PrivateKeyDer<'static> =
            rustls_pemfile::private_key(&mut BufReader::new(key))?.context("客户端私钥无效")?;
        builder
            .with_client_auth_cert(certificates(cert)?, key)
            .context("客户端证书与私钥不匹配")?
    } else {
        builder.with_no_client_auth()
    };
    Ok(Arc::new(config))
}

pub fn paired_tls(path: &Path) -> Result<Arc<TlsConfig>> {
    let folder = config::active_credentials_dir(path)?;
    let ca = std::fs::read(folder.join("ca.pem"))
        .context("尚未配对：请先选择 Linux 导出的 CA 文件并输入配对码")?;
    let cert = std::fs::read(folder.join("client.pem"))?;
    let key = std::fs::read(folder.join("client.key"))?;
    make_tls(&ca, Some((&cert, &key)))
}

pub async fn open_tls(
    config: &ClientConfig,
    tls: Arc<TlsConfig>,
    selector: &[u8],
) -> Result<TlsStream<TcpStream>> {
    open_tls_tracked(config, tls, selector, None).await
}

async fn open_tls_tracked(
    config: &ClientConfig,
    tls: Arc<TlsConfig>,
    selector: &[u8],
    progress: Option<&ControlProgress>,
) -> Result<TlsStream<TcpStream>> {
    let name = ServerName::try_from(config.server_name.clone()).context("证书名称无效")?;
    let mut stream = tokio::time::timeout(Duration::from_secs(10), async {
        if let Some(progress) = progress {
            progress.advance(1);
        }
        let socket = TcpStream::connect((config.host.as_str(), config.port)).await?;
        socket.set_nodelay(true)?;
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(15))
            .with_interval(Duration::from_secs(5));
        socket2::SockRef::from(&socket).set_tcp_keepalive(&keepalive)?;
        if let Some(progress) = progress {
            progress.advance(2);
        }
        TlsConnector::from(tls).connect(name, socket).await
    })
    .await
    .context("连接 Linux 超时")?
    .context("连接 Linux 失败，请检查网络及 CA 证书")?;
    if let Some(progress) = progress {
        progress.advance(3);
    }
    stream.write_all(selector).await?;
    Ok(stream)
}

pub async fn write_frame<S: AsyncWrite + Unpin, T: serde::Serialize>(
    stream: &mut S,
    value: &T,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        bail!("消息过大");
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

pub async fn read_frame<S: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
    stream: &mut S,
) -> Result<T> {
    let count = stream.read_u32().await? as usize;
    if count == 0 || count > MAX_FRAME {
        bail!("消息长度无效");
    }
    let mut data = vec![0u8; count];
    stream.read_exact(&mut data).await?;
    serde_json::from_slice(&data).context("收到无效的 JSON 消息")
}

pub async fn control(
    config: &ClientConfig,
    tls: Arc<TlsConfig>,
    request: &ControlRequest,
) -> Result<ControlResponse> {
    let progress = ControlProgress::new(request);
    stage_timeout(MANAGEMENT_TIMEOUT, &progress, async {
        let mut stream = open_tls_tracked(config, tls, b"control\n", Some(&progress)).await?;
        let response = exchange_control(&mut stream, request, &progress).await?;
        if !response.ok {
            bail!(
                "{}",
                response.error.as_deref().unwrap_or("Linux 拒绝了请求")
            );
        }
        tracing::debug!(
            operation = progress.operation,
            elapsed_ms = progress.started.elapsed().as_millis() as u64,
            "management completed"
        );
        Ok(response)
    })
    .await
}

async fn exchange_control<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    request: &ControlRequest,
    progress: &ControlProgress,
) -> Result<ControlResponse> {
    let bytes = serde_json::to_vec(request)?;
    if bytes.len() > MAX_FRAME {
        bail!("消息过大");
    }
    progress.advance(4);
    stream.write_u32(bytes.len() as u32).await?;
    progress.advance(5);
    stream.write_all(&bytes).await?;
    progress.advance(6);
    stream.flush().await?;
    progress.advance(7);
    let count = stream.read_u32().await? as usize;
    if count == 0 || count > MAX_FRAME {
        bail!("消息长度无效");
    }
    progress.advance(8);
    let mut data = vec![0; count];
    stream.read_exact(&mut data).await?;
    progress.advance(9);
    serde_json::from_slice(&data).context("收到无效的 JSON 消息")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct TraceBuffer(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for TraceBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceBuffer {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn management_deadline_reports_exact_read_stage_without_secrets() {
        use tracing::instrument::WithSubscriber;
        for partial_body in [false, true] {
            let (mut client, mut server) = tokio::io::duplex(512);
            let peer = tokio::spawn(async move {
                let _: ControlRequest = read_frame(&mut server).await.unwrap();
                if partial_body {
                    server.write_u32(128).await.unwrap();
                    server.write_all(b"{").await.unwrap();
                    server.flush().await.unwrap();
                }
                std::future::pending::<()>().await;
            });
            let request = ControlRequest::Pair {
                code: "PAIR_CODE_MUST_NOT_APPEAR".into(),
                name: "PRIVATE_NAME_MUST_NOT_APPEAR".into(),
            };
            let progress = ControlProgress::new(&request);
            let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(TraceBuffer(bytes.clone()))
                .finish();
            let error = stage_timeout(
                Duration::from_millis(40),
                &progress,
                exchange_control(&mut client, &request, &progress),
            )
            .with_subscriber(subscriber)
            .await
            .unwrap_err();
            let expected = if partial_body {
                "读取响应数据"
            } else {
                "读取响应长度"
            };
            assert!(error.to_string().contains(expected));
            let logged = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
            assert!(logged.contains(expected));
            assert!(!logged.contains("PAIR_CODE_MUST_NOT_APPEAR"));
            assert!(!logged.contains("PRIVATE_NAME_MUST_NOT_APPEAR"));
            peer.abort();
        }
    }

    #[tokio::test]
    async fn tracked_control_exchange_preserves_framing_and_reply_bounds() {
        for oversized in [false, true] {
            let (mut client, mut server) = tokio::io::duplex(2048);
            let peer = tokio::spawn(async move {
                let request: ControlRequest = read_frame(&mut server).await.unwrap();
                assert!(matches!(request, ControlRequest::Status));
                if oversized {
                    server.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
                } else {
                    write_frame(
                        &mut server,
                        &ControlResponse {
                            ok: true,
                            server_id: "test-server".into(),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                }
            });
            let request = ControlRequest::Status;
            let progress = ControlProgress::new(&request);
            let response = exchange_control(&mut client, &request, &progress).await;
            if oversized {
                assert!(response.unwrap_err().to_string().contains("消息长度无效"));
            } else {
                assert_eq!(response.unwrap().server_id, "test-server");
                assert_eq!(progress.name(), "验证响应");
            }
            peer.await.unwrap();
        }
    }
    #[tokio::test]
    async fn framed_json_roundtrip_and_bound() {
        let (mut left, mut right) = tokio::io::duplex(128);
        let writer = tokio::spawn(async move {
            write_frame(&mut left, &serde_json::json!({"op":"status"}))
                .await
                .unwrap();
        });
        let reply: serde_json::Value = read_frame(&mut right).await.unwrap();
        writer.await.unwrap();
        assert_eq!(reply["op"], "status");
        let (mut left, mut right) = tokio::io::duplex(16);
        left.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
        assert!(read_frame::<_, serde_json::Value>(&mut right)
            .await
            .is_err());
    }
    #[test]
    fn invalid_ca_fails_closed() {
        assert!(make_tls(b"not a CA", None).is_err());
    }
}
