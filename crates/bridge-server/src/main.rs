use anyhow::Result;
use bridge_server::{config, sysfs::Backend, Server};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "usb-bridge-server",
    version,
    about = "Rust USB bridge Linux server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    ForceRelease {
        #[arg(long, default_value = "/var/lib/rust-usb-bridge")]
        state_dir: PathBuf,
    },
    Init {
        #[arg(long, default_value = "/var/lib/rust-usb-bridge")]
        state_dir: PathBuf,
        #[arg(long)]
        server_name: Vec<String>,
        #[arg(long)]
        mock: bool,
    },
    Run {
        #[arg(long, default_value = "/var/lib/rust-usb-bridge")]
        state_dir: PathBuf,
        #[arg(long)]
        listen: Option<String>,
        #[arg(long, default_value = "/sys")]
        sysfs_root: PathBuf,
        #[arg(long, default_value = "/proc")]
        proc_root: PathBuf,
        #[arg(long)]
        mock: bool,
    },
    Restore {
        #[arg(long, default_value = "/var/lib/rust-usb-bridge")]
        state_dir: PathBuf,
        #[arg(long, default_value = "/sys")]
        sysfs_root: PathBuf,
        #[arg(long, default_value = "/proc")]
        proc_root: PathBuf,
        #[arg(long)]
        mock: bool,
    },
    PairCode {
        #[arg(long, default_value = "/var/lib/rust-usb-bridge")]
        state_dir: PathBuf,
        #[arg(long)]
        mock: bool,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bridge_server=info".into()),
        )
        .init();
    match Cli::parse().command {
        Command::ForceRelease { state_dir } => {
            #[cfg(unix)]
            bridge_server::admin::command(state_dir).await?;
            #[cfg(not(unix))]
            { let _ = state_dir; anyhow::bail!("Linux only"); }
        }
        Command::Init {
            state_dir,
            server_name,
            mock,
        } => {
            config::require_root(mock)?;
            let config = config::init(&state_dir, server_name)?;
            println!("Initialized server {} in {}. Public trust certificate: ca.pem. Run pair-code to display the current pairing code.",config.server_id,state_dir.display());
        }
        Command::Run {
            state_dir,
            listen,
            sysfs_root,
            proc_root,
            mock,
        } => {
            config::require_root(mock)?;
            let server = Server::open(state_dir, Backend::new(sysfs_root, proc_root, mock)?)?;
            let result = tokio::select! {
                result = server.clone().serve(listen) => result,
                _ = shutdown_signal() => { tracing::info!("server shutting down; dropping network sessions"); Ok(()) }
            };
            server.shutdown().await;
            // The kernel holds a reference to its accepted TCP socket. Closing
            // Rust's loopback peer produces EOF and triggers per-device URB
            // cleanup even after a forced process exit. Shared selections stay
            // until explicit SetShared(false) or restore for rollback.
            result?;
        }
        Command::Restore {
            state_dir,
            sysfs_root,
            proc_root,
            mock,
        } => {
            config::require_root(mock)?;
            Server::open(state_dir, Backend::new(sysfs_root, proc_root, mock)?)?
                .restore_all()
                .await?;
            println!("Restored recorded USB bindings; other exporters were not modified.");
        }
        Command::PairCode { state_dir, mock } => {
            config::require_root(mock)?;
            println!("{}", config::load(&state_dir)?.pair_code);
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM registration");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
