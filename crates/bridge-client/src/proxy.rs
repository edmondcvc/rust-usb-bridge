use crate::{
    config::{self, ClientConfig},
    network,
};
use anyhow::{bail, Context, Result};
use rustls::ClientConfig as TlsConfig;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch, Mutex, RwLock},
};

#[derive(Clone)]
struct Permit {
    id: u64,
    claimed: Arc<AtomicBool>,
    cancelled: watch::Receiver<bool>,
}

#[derive(Clone)]
pub struct Endpoint {
    pub config: ClientConfig,
    pub tls: Arc<TlsConfig>,
}

#[derive(Debug)]
pub struct ProxyEvent {
    pub busid: String,
    pub session_id: u64,
    pub error: Option<String>,
}

pub struct Proxy {
    pub port: u16,
    endpoint: Arc<RwLock<Option<Endpoint>>>,
    permits: Arc<Mutex<HashMap<String, Permit>>>,
    listener_shutdown: watch::Sender<bool>,
    next_session: AtomicU64,
}

impl Proxy {
    pub async fn start(port: u16, events: mpsc::UnboundedSender<ProxyEvent>) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .context("本地 USB 代理端口被占用，请关闭冲突程序后重启服务")?;
        let actual = listener.local_addr()?.port();
        let endpoint: Arc<RwLock<Option<Endpoint>>> = Arc::new(RwLock::new(None));
        let permits = Arc::new(Mutex::new(HashMap::new()));
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let endpoint_task = endpoint.clone();
        let permits_task = permits.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.accept() => match result {
                        Ok((socket, peer)) => {
                            if !peer.ip().is_loopback() { continue; }
                            let endpoint = endpoint_task.clone(); let permits = permits_task.clone(); let events = events.clone();
                            tokio::spawn(async move {
                                if let Err(error) = forward(socket, endpoint, permits, events).await {
                                    tracing::warn!(error = %error, "本地 USB 代理连接失败");
                                }
                            });
                        }
                        Err(error) => { tracing::error!(%error, "本地 USB 代理监听失败"); break; }
                    },
                    _ = stop_rx.changed() => break,
                }
            }
        });
        Ok(Self {
            port: actual,
            endpoint,
            permits,
            listener_shutdown: stop_tx,
            next_session: AtomicU64::new(1),
        })
    }

    pub async fn set_endpoint(&self, endpoint: Option<Endpoint>) {
        *self.endpoint.write().await = endpoint;
    }

    pub async fn authorize(&self, busid: &str) -> Result<(u64, watch::Sender<bool>)> {
        config::validate_busid(busid)?;
        let mut permits = self.permits.lock().await;
        if permits.contains_key(busid) {
            bail!("设备已有本机连接会话");
        }
        let (cancelled, receiver) = watch::channel(false);
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        permits.insert(
            busid.to_owned(),
            Permit {
                id,
                claimed: Arc::new(AtomicBool::new(false)),
                cancelled: receiver,
            },
        );
        Ok((id, cancelled))
    }

    pub async fn revoke(&self, busid: &str) {
        self.permits.lock().await.remove(busid);
    }

    pub async fn shutdown(&self) {
        let _ = self.listener_shutdown.send(true);
        self.permits.lock().await.clear();
    }
}

async fn forward(
    mut socket: TcpStream,
    endpoint: Arc<RwLock<Option<Endpoint>>>,
    permits: Arc<Mutex<HashMap<String, Permit>>>,
    events: mpsc::UnboundedSender<ProxyEvent>,
) -> Result<()> {
    socket.set_nodelay(true)?;
    // A single import handshake precedes opaque USB/IP packets. Never buffer or
    // reinterpret URBs: control/bulk/interrupt/isochronous data retain wire order.
    let mut handshake = [0u8; 40];
    tokio::time::timeout(Duration::from_secs(10), socket.read_exact(&mut handshake))
        .await
        .context("本地驱动握手超时")??;
    let busid = parse_import(&handshake)?;
    let mut permit = permits
        .lock()
        .await
        .get(&busid)
        .cloned()
        .context("未授权的本地 USB 请求")?;
    if *permit.cancelled.borrow() || permit.claimed.swap(true, Ordering::SeqCst) {
        bail!("USB 会话已结束或已被使用");
    }
    let target = endpoint.read().await.clone().context("客户端尚未配对")?;
    let result: Result<()> = async {
        let mut upstream = network::open_tls(&target.config, target.tls, b"usbip\n").await?;
        upstream.write_all(&handshake).await?;
        upstream.flush().await?;
        let (mut local_read, mut local_write) = tokio::io::split(socket);
        let (mut remote_read, mut remote_write) = tokio::io::split(upstream);
        // Finish on either EOF, rather than waiting for the opposite endpoint
        // to also close while the Windows driver still has outstanding URBs.
        tokio::select! {
            result = tokio::io::copy(&mut local_read, &mut remote_write) => {
                let upload = result.context("本地 USB 连接中断")?;
                tracing::info!(%busid, upload, "本地 USB 会话结束");
            }
            result = tokio::io::copy(&mut remote_read, &mut local_write) => {
                let download = result.context("USB 网络连接中断")?;
                tracing::info!(%busid, download, "网络 USB 会话结束");
            }
            _ = permit.cancelled.changed() => { tracing::info!(%busid, "本机释放 USB 会话"); }
        }
        Ok(())
    }
    .await;
    // Dropping both streams closes this device only. No request replay occurs.
    let error = result.as_ref().err().map(|e| format!("{e:#}"));
    let _ = events.send(ProxyEvent {
        busid: busid.clone(),
        session_id: permit.id,
        error,
    });
    result
}

pub fn parse_import(handshake: &[u8; 40]) -> Result<String> {
    if handshake[..8] != [0x01, 0x11, 0x80, 0x03, 0, 0, 0, 0] {
        bail!("本地代理只接受 USB/IP 1.1.1 导入请求");
    }
    let bytes = &handshake[8..];
    let end = bytes
        .iter()
        .position(|&b| b == 0)
        .context("USB/IP busid 未结束")?;
    if bytes[end..].iter().any(|&b| b != 0) {
        bail!("USB/IP busid 尾部数据无效");
    }
    let busid = std::str::from_utf8(&bytes[..end])?.to_owned();
    config::validate_busid(&busid)?;
    Ok(busid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allows_only_import_with_valid_device() {
        let mut request = [0u8; 40];
        request[..8].copy_from_slice(&[1, 0x11, 0x80, 3, 0, 0, 0, 0]);
        request[8..11].copy_from_slice(b"1-2");
        assert_eq!(parse_import(&request).unwrap(), "1-2");
        request[3] = 5;
        assert!(parse_import(&request).is_err());
        request[3] = 3;
        request[20] = 1;
        assert!(parse_import(&request).is_err());
    }
    #[tokio::test]
    async fn loopback_listener_preserves_fixed_port_and_per_device_permits() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let first = Proxy::start(0, tx.clone()).await.unwrap();
        assert!(first.port > 0);
        assert!(Proxy::start(first.port, tx).await.is_err());
        let (_, a) = first.authorize("1-2").await.unwrap();
        let (_, b) = first.authorize("1-3").await.unwrap();
        assert!(first.authorize("1-2").await.is_err());
        a.send(true).unwrap();
        first.revoke("1-2").await;
        assert!(!*b.subscribe().borrow());
        first.authorize("1-2").await.unwrap();
        first.shutdown().await;
    }
    #[tokio::test]
    async fn mtls_proxy_forwards_opaque_packets_and_isolates_disconnects() {
        use rcgen::{
            BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair,
            KeyUsagePurpose,
        };
        use rustls::{
            pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
            RootCertStore,
        };
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let server_key = KeyPair::generate().unwrap();
        let mut server_params = CertificateParams::new(vec!["usb-bridge".into()]).unwrap();
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let server_cert = server_params.signed_by(&server_key, &ca, &ca_key).unwrap();
        let client_key = KeyPair::generate().unwrap();
        let mut client_params = CertificateParams::new(vec!["test-client".into()]).unwrap();
        client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let client_cert = client_params.signed_by(&client_key, &ca, &ca_key).unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(ca.der().clone()).unwrap();
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .unwrap();
        let server_tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server_cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
        )
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_tls));
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let server_port = listener.local_addr().unwrap().port();
        let server_task = tokio::spawn(async move {
            let mut tasks = Vec::new();
            for _ in 0..2 {
                let (socket, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tasks.push(tokio::spawn(async move {
                    let mut stream = acceptor.accept(socket).await.unwrap();
                    let mut selector = [0u8; 6];
                    stream.read_exact(&mut selector).await.unwrap();
                    assert_eq!(&selector, b"usbip\n");
                    let mut import = [0u8; 40];
                    stream.read_exact(&mut import).await.unwrap();
                    stream.write_all(&import).await.unwrap();
                    let mut buffer = [0u8; 2048];
                    loop {
                        let count = match stream.read(&mut buffer).await {
                            Ok(count) => count,
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::UnexpectedEof
                                        | std::io::ErrorKind::ConnectionReset
                                ) =>
                            {
                                break
                            }
                            Err(error) => panic!("unexpected mock TLS failure: {error}"),
                        };
                        if count == 0 {
                            break;
                        }
                        stream.write_all(&buffer[..count]).await.unwrap();
                    }
                }));
            }
            for task in tasks {
                task.await.unwrap();
            }
        });
        let tls = network::make_tls(
            ca.pem().as_bytes(),
            Some((
                client_cert.pem().as_bytes(),
                client_key.serialize_pem().as_bytes(),
            )),
        )
        .unwrap();
        let mut config = ClientConfig::default();
        config.host = "127.0.0.1".into();
        config.port = server_port;
        let (events, mut received) = mpsc::unbounded_channel();
        let proxy = Proxy::start(0, events).await.unwrap();
        proxy.set_endpoint(Some(Endpoint { config, tls })).await;
        let (first_id, first_cancel) = proxy.authorize("1-2").await.unwrap();
        let (_second_id, second_cancel) = proxy.authorize("1-3").await.unwrap();
        let mut first = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, proxy.port))
            .await
            .unwrap();
        let mut second = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, proxy.port))
            .await
            .unwrap();
        for (stream, busid) in [(&mut first, b"1-2"), (&mut second, b"1-3")] {
            let mut import = [0u8; 40];
            import[..8].copy_from_slice(&[1, 0x11, 0x80, 3, 0, 0, 0, 0]);
            import[8..11].copy_from_slice(busid);
            stream.write_all(&import).await.unwrap();
            let mut reply = [0u8; 40];
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reply, import);
        }
        // Includes arbitrary bytes, zeros and packet-like metadata. The proxy
        // does not parse endpoint types or rewrite isochronous payloads.
        let payload = (0u8..=255).cycle().take(1536).collect::<Vec<_>>();
        first.write_all(&payload).await.unwrap();
        let mut echoed = vec![0u8; payload.len()];
        first.read_exact(&mut echoed).await.unwrap();
        assert_eq!(echoed, payload);
        first_cancel.send(true).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.busid, "1-2");
        assert_eq!(event.session_id, first_id);
        second.write_all(&payload).await.unwrap();
        let mut echoed = vec![0u8; payload.len()];
        tokio::time::timeout(Duration::from_secs(5), second.read_exact(&mut echoed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(echoed, payload);
        second_cancel.send(true).unwrap();
        let _ = received.recv().await;
        drop(first);
        drop(second);
        proxy.shutdown().await;
        tokio::time::timeout(Duration::from_secs(5), server_task)
            .await
            .unwrap()
            .unwrap();
    }
}
