#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use anyhow::{bail, Context, Result};
use bridge_client::{
    backend::{Backend, LocalRequest},
    config::{self, ClientConfig},
    driver, ipc, network,
    proxy::{Endpoint, Proxy},
};
use bridge_common::ControlRequest;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "bridge-client",
    version,
    about = "Rust USB 网络客户端：Windows 中文界面、后台服务与 TLS USB/IP 转发"
)]
struct Args {
    #[arg(long, default_value_os_t = config::default_path())]
    config: PathBuf,
    #[arg(long)]
    service: bool,
    /// Isolated tutorial UI: never connects to a service or device.
    #[arg(long)]
    tutorial_demo: bool,
    #[arg(long)]
    background: bool,
    /// Explicit development IPC: connect only to the PID-qualified diagnostic backend.
    #[arg(long, conflicts_with_all = ["service", "background", "install_service", "uninstall_service", "direct", "diagnostics", "proxy"])]
    diagnostic_ipc: Option<std::num::NonZeroU32>,
    #[arg(long)]
    install_service: bool,
    #[arg(long)]
    uninstall_service: bool,
    #[arg(long)]
    diagnostics: bool,
    #[arg(long)]
    list: bool,
    #[arg(long)]
    status: bool,
    #[arg(long)]
    pair: bool,
    /// Read config and connect directly; otherwise use the restricted Windows service pipe.
    #[arg(long)]
    direct: bool,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    server_name: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    code: Option<String>,
    #[arg(long)]
    connect: Option<String>,
    #[arg(long)]
    release: Option<String>,
    #[arg(long)]
    share: Option<String>,
    #[arg(long)]
    unshare: Option<String>,
    #[arg(long)]
    export_logs: Option<PathBuf>,
    /// Test-only explicit proxy. Does not invoke the Windows driver.
    #[arg(long)]
    proxy: bool,
    #[arg(long, requires = "proxy")]
    allow_busid: Vec<String>,
    #[arg(long, requires = "proxy")]
    proxy_port: Option<u16>,
    /// Write structured command output to a file (useful for the GUI-subsystem release binary).
    #[arg(long)]
    output: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    if let Err(error) = run(&args) {
        let text = format!("{error:#}");
        eprintln!("{text}");
        if let Some(path) = &args.output {
            let _ = std::fs::write(
                path,
                serde_json::to_string_pretty(&serde_json::json!({"ok":false,"error":text}))
                    .unwrap(),
            );
        }
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<()> {
    #[cfg(windows)]
    if args.tutorial_demo { return bridge_client::gui::run(ipc::Target::Service, true); }
    if args.install_service {
        #[cfg(windows)]
        {
            return bridge_client::service::install(&args.config);
        }
        #[cfg(not(windows))]
        {
            bail!("服务安装只支持 Windows");
        }
    }
    if args.uninstall_service {
        #[cfg(windows)]
        {
            return bridge_client::service::uninstall();
        }
        #[cfg(not(windows))]
        {
            bail!("服务卸载只支持 Windows");
        }
    }
    if args.service {
        let _log = start_logging(&args.config)?;
        #[cfg(windows)]
        {
            return bridge_client::service::dispatch(args.config.clone());
        }
        #[cfg(not(windows))]
        {
            bail!("SCM 服务只支持 Windows");
        }
    }
    let commands = args.background
        || args.diagnostics
        || args.list
        || args.status
        || args.pair
        || args.proxy
        || args.connect.is_some()
        || args.release.is_some()
        || args.share.is_some()
        || args.unshare.is_some()
        || args.export_logs.is_some();
    if !commands {
        #[cfg(windows)]
        {
            return bridge_client::gui::run(ipc_target(args), false);
        }
        #[cfg(not(windows))]
        {
            bail!("Linux 上使用 --list/--pair --direct；中文桌面界面只在 Windows 上提供");
        }
    }
    if args.diagnostics {
        let probe = driver::probe();
        return output(
            args,
            &serde_json::json!({"pinned_driver":driver::PINNED_DRIVER_VERSION,"pinned_pnp_driver":driver::PINNED_PNP_DRIVER_VERSION,"plugin_size":std::mem::size_of::<driver::PluginRequest>(),"imported_size":std::mem::size_of::<driver::ImportedDevice>(),"driver_present":probe.is_ok(),"identity":probe.as_ref().ok().map(|value|&value.identity),"devices":probe.as_ref().ok().map(|value|&value.devices),"error":probe.err().map(|e|format!("{e:#}"))}),
        );
    }
    if !args.background && std::env::var_os("RUST_LOG").is_some() {
        // Explicit child-process diagnostics only; no global environment or
        // installed-service logging settings are changed by direct CLI runs.
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .context("RUST_LOG 格式无效")?,
            )
            .try_init()
            .map_err(|error| anyhow::anyhow!("初始化诊断日志失败：{error}"))?;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async_run(args))
}

fn ipc_target(args: &Args) -> ipc::Target {
    args.diagnostic_ipc
        .map(|pid| ipc::Target::Diagnostic(pid.get()))
        .unwrap_or(ipc::Target::Service)
}

fn start_logging(path: &std::path::Path) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let directory = config::directory(path).join("logs");
    std::fs::create_dir_all(&directory)?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("client")
        .filename_suffix("log")
        .max_log_files(7)
        .build(directory)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(writer)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("bridge_client=info")),
        )
        .try_init();
    Ok(guard)
}

fn candidate(args: &Args, mut config: ClientConfig) -> Result<ClientConfig> {
    if let Some(value) = &args.host {
        config.host = value.clone();
    }
    if let Some(value) = args.port {
        config.port = value;
    }
    if let Some(value) = &args.server_name {
        config.server_name = value.clone();
    }
    if let Some(value) = &args.name {
        config.display_name = value.clone();
    }
    config.validate()?;
    Ok(config)
}

async fn async_run(args: &Args) -> Result<()> {
    if args.background {
        let _guard = start_logging(&args.config)?;
        let backend = Backend::start(args.config.clone()).await?;
        let (stop, receiver) = tokio::sync::watch::channel(false);
        tracing::info!(
            pid = std::process::id(),
            "显式诊断后台启动，客户端必须指定 --diagnostic-ipc PID"
        );
        let mut task = tokio::spawn(ipc::serve_diagnostic(backend.clone(), receiver));
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, result = &mut task => { result??; } }
        let _ = stop.send(true);
        backend.shutdown().await;
        task.abort();
        return Ok(());
    }
    if args.proxy {
        let config = candidate(args, config::load(&args.config)?)?;
        if config.client_id.is_none() {
            bail!("测试代理需要已配对配置");
        }
        let tls = network::paired_tls(&args.config)?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let proxy = Proxy::start(args.proxy_port.unwrap_or(config.proxy_port), tx).await?;
        proxy.set_endpoint(Some(Endpoint { config, tls })).await;
        let mut permits = Vec::new();
        for busid in &args.allow_busid {
            permits.push(proxy.authorize(busid).await?);
        }
        output(
            args,
            &serde_json::json!({"ok":true,"listen":format!("127.0.0.1:{}",proxy.port),"authorized_busids":args.allow_busid}),
        )?;
        loop {
            tokio::select! { _ = tokio::signal::ctrl_c() => break, event = rx.recv() => if let Some(event) = event { eprintln!("proxy session {} ended: {:?}",event.busid,event.error); } else { break; } }
        }
        for (_, cancel) in permits {
            let _ = cancel.send(true);
        }
        proxy.shutdown().await;
        return Ok(());
    }
    if args.direct || !cfg!(windows) {
        return direct(args).await;
    }
    let request = if args.pair {
        let base = ipc::request_to(ipc_target(args), &LocalRequest::Snapshot)
            .await?
            .snapshot
            .context("后台没有返回配置")?
            .config;
        let config = candidate(args, base)?;
        LocalRequest::Pair {
            host: config.host,
            port: config.port,
            server_name: config.server_name,
            name: config.display_name,
            ca_pem: std::fs::read_to_string(args.ca.as_ref().context("需要 --ca CA.pem")?)?,
            code: args.code.as_ref().context("需要 --code 配对码")?.clone(),
        }
    } else if let Some(busid) = &args.connect {
        LocalRequest::Connect {
            busid: busid.clone(),
        }
    } else if let Some(busid) = &args.release {
        LocalRequest::Release {
            busid: busid.clone(),
        }
    } else if let Some(busid) = &args.share {
        LocalRequest::SetShared {
            busid: busid.clone(),
            shared: true,
        }
    } else if let Some(busid) = &args.unshare {
        LocalRequest::SetShared {
            busid: busid.clone(),
            shared: false,
        }
    } else if args.export_logs.is_some() {
        LocalRequest::Logs
    } else if args.list {
        LocalRequest::Refresh
    } else {
        LocalRequest::Snapshot
    };
    let mut response = ipc::request_to(ipc_target(args), &request).await?;
    if let Some(path) = &args.export_logs {
        if let Some(log) = response.log.take() {
            std::fs::write(path, log)?;
        }
    }
    output(args, &response)?;
    if !response.ok {
        bail!("{}", response.error.as_deref().unwrap_or("后台操作失败"));
    }
    Ok(())
}

async fn direct(args: &Args) -> Result<()> {
    let mut config = candidate(args, config::load(&args.config)?)?;
    if args.pair {
        let ca = std::fs::read(args.ca.as_ref().context("需要 --ca CA.pem")?)?;
        let tls = network::make_tls(&ca, None)?;
        let response = network::control(
            &config,
            tls,
            &ControlRequest::Pair {
                code: args.code.as_ref().context("需要 --code 配对码")?.clone(),
                name: config.display_name.clone(),
            },
        )
        .await?;
        let pairing = response.pairing.context("服务器没有返回配对证书")?;
        if network::certificates(pairing.ca_cert_pem.as_bytes())? != network::certificates(&ca)? {
            bail!("服务器返回的 CA 与信任文件不同");
        }
        network::make_tls(
            pairing.ca_cert_pem.as_bytes(),
            Some((
                pairing.client_cert_pem.as_bytes(),
                pairing.client_key_pem.as_bytes(),
            )),
        )?;
        config::save_pairing(&args.config, &pairing)?;
        config.credential_generation = Some(pairing.client_id.clone());
        config.client_id = Some(pairing.client_id);
        config.auto_connect.clear();
        config.auto_connect_identities.clear();
        config::save(&args.config, &config)?;
        return output(
            args,
            &serde_json::json!({"ok":true,"client_id":config.client_id,"server_id":response.server_id}),
        );
    }
    if args.connect.is_some() || args.release.is_some() {
        bail!("设备连接与释放必须使用 Windows 后台服务，不支持 --direct");
    }
    let request = if let Some(busid) = &args.share {
        ControlRequest::SetShared {
            busid: busid.clone(),
            shared: true,
        }
    } else if let Some(busid) = &args.unshare {
        ControlRequest::SetShared {
            busid: busid.clone(),
            shared: false,
        }
    } else if args.status {
        ControlRequest::Status
    } else {
        ControlRequest::ListDevices
    };
    let response = network::control(&config, network::paired_tls(&args.config)?, &request).await?;
    output(args, &response)
}

fn output<T: serde::Serialize>(args: &Args, data: &T) -> Result<()> {
    let data = serde_json::to_string_pretty(data)?;
    if let Some(path) = &args.output {
        std::fs::write(path, &data)?;
    }
    println!("{data}");
    Ok(())
}
