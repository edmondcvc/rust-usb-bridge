use crate::{backend::Backend, ipc, SERVICE_NAME};
use anyhow::{Context, Result};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};
use tokio::sync::watch;
use windows_service::{
    define_windows_service,
    service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};

static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
define_windows_service!(ffi_service_main, service_main);

pub fn dispatch(path: PathBuf) -> Result<()> {
    CONFIG_PATH
        .set(path)
        .map_err(|_| anyhow::anyhow!("服务配置已初始化"))?;
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("连接 Windows 服务管理器失败")?;
    Ok(())
}

fn service_main(_: Vec<OsString>) {
    if let Err(error) = run() {
        tracing::error!(error = %error, "USB 后台服务退出");
    }
}

fn status(
    state: ServiceState,
    accepts: ServiceControlAccept,
    wait_hint: Duration,
    exit: ServiceExitCode,
) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accepts,
        exit_code: exit,
        checkpoint: if state == ServiceState::StartPending || state == ServiceState::StopPending {
            1
        } else {
            0
        },
        wait_hint,
        process_id: None,
    }
}

fn run() -> Result<()> {
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let handler = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handler.set_service_status(status(
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        Duration::from_secs(30),
        ServiceExitCode::Win32(0),
    ))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;
    let result: Result<()> = runtime.block_on(async {
        let config = CONFIG_PATH.get().context("缺少服务配置路径")?.clone();
        let backend = Backend::start(config).await?;
        // Create the restricted pipe before SCM reports Running.
        let stop_for_ipc = stop_rx.clone();
        let ipc_backend = backend.clone();
        let mut ipc_task = tokio::spawn(async move { ipc::serve(ipc_backend, stop_for_ipc).await });
        tokio::task::yield_now().await;
        handler.set_service_status(status(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN, Duration::ZERO, ServiceExitCode::Win32(0)))?;
        tracing::info!("USB 后台服务已启动");
        let result = tokio::select! {
            _ = stop_rx.changed() => Ok(()),
            result = &mut ipc_task => match result { Ok(result) => result, Err(error) => Err(error.into()) },
        };
        handler.set_service_status(status(ServiceState::StopPending, ServiceControlAccept::empty(), Duration::from_secs(180), ServiceExitCode::Win32(0)))?;
        backend.shutdown().await;
        ipc_task.abort();
        result
    });
    let code = if result.is_ok() {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::ServiceSpecific(1)
    };
    handler.set_service_status(status(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        Duration::ZERO,
        code,
    ))?;
    result
}

pub fn install(path: &Path) -> Result<()> {
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;
    let executable = std::env::current_exe()?;
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: "Rust USB 网络转发后台".into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: executable,
        launch_arguments: vec![
            OsString::from("--service"),
            OsString::from("--config"),
            path.as_os_str().to_owned(),
        ],
        dependencies: Vec::new(),
        account_name: Some(OsString::from(r"NT AUTHORITY\LocalService")),
        account_password: None,
    };
    let service = manager.create_service(
        &info,
        ServiceAccess::CHANGE_CONFIG | ServiceAccess::QUERY_STATUS,
    )?;
    service.set_description(
        "将 Linux USB 设备通过已配对的 TLS 连接转发到官方签名 USB/IP 驱动。关闭窗口不会中断连接。",
    )?;
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE,
    )?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop()?;
        let start = std::time::Instant::now();
        while service.query_status()?.current_state != ServiceState::Stopped {
            if start.elapsed() > Duration::from_secs(180) {
                anyhow::bail!("后台服务停止超时，卸载没有删除服务");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    service.delete()?;
    Ok(())
}
