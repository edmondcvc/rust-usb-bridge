pub mod backend;
pub mod config;
pub mod driver;
#[cfg(windows)]
pub mod gui;
pub mod ipc;
pub mod network;
pub mod proxy;
pub mod presentation;
#[cfg(windows)]
pub mod security;
#[cfg(windows)]
pub mod service;

pub const SERVICE_NAME: &str = "UsbBridgeRust";
pub const PIPE_NAME: &str = r"\\.\pipe\RustUsbBridge.Control.v1";

#[cfg(windows)]
pub mod enrollment;

#[cfg(windows)]
mod window_lifecycle;
