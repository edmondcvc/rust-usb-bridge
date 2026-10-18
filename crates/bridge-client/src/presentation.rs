//! User-facing state, independent of drawing and driver calls.
use crate::{
    backend::{LocalRequest, LocalResponse, SessionStatus, Snapshot},
    config::DeviceIdentity,
};
use anyhow::{bail, Context, Result};
use bridge_common::UsbDevice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Unknown,
    Offline,
    Connecting,
    Mine,
    Protected,
    Occupied,
    Failed,
    Unshared,
    Available,
}

impl DeviceState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "状态未知",
            Self::Offline => "此服务未发现设备",
            Self::Connecting => "正在连接",
            Self::Mine => "本机正在使用",
            Self::Protected => "暂不能连接",
            Self::Occupied => "其他电脑正在使用",
            Self::Failed => "连接未成功",
            Self::Unshared => "空闲，可以连接",
            Self::Available => "空闲，可以连接",
        }
    }
}

/// A user click is one workflow. The service retains its existing two-step API.
/// Capture physical identity so preparation cannot connect a replacement device.
pub struct PreparedConnection {
    busid: String,
    identity: DeviceIdentity,
}

fn usable_device<'a>(snapshot: &'a Snapshot, busid: &str) -> Result<&'a UsbDevice> {
    if snapshot.config.host_disconnected || !snapshot.paired || snapshot.server_id.is_empty() || snapshot.network_error.is_some() {
        bail!("无法确认 Linux 设备状态，请刷新后重试");
    }
    if snapshot.driver_error.is_some() {
        bail!("Windows USB 驱动未就绪，请修复驱动后再连接");
    }
    let device = snapshot
        .devices
        .iter()
        .find(|d| d.busid == busid)
        .context("设备已不在列表中，请刷新后重试")?;
    if !device.present {
        bail!("此服务未发现设备，请检查原连接状态");
    }
    if let Some(reason) = &device.protected_reason {
        bail!("设备暂不能连接：{reason}");
    }
    if let Some(owner) = &device.owner {
        bail!("设备正在由 {owner} 使用，请等待对方断开连接");
    }
    Ok(device)
}

pub fn connection_request(
    snapshot: &Snapshot,
    busid: &str,
) -> Result<(LocalRequest, Option<PreparedConnection>)> {
    let device = usable_device(snapshot, busid)?;
    if device.shared {
        Ok((
            LocalRequest::Connect {
                busid: busid.into(),
            },
            None,
        ))
    } else {
        Ok((
            LocalRequest::SetShared {
                busid: busid.into(),
                shared: true,
            },
            Some(PreparedConnection {
                busid: busid.into(),
                identity: DeviceIdentity::from_device(&snapshot.server_id, device),
            }),
        ))
    }
}

impl PreparedConnection {
    pub fn followup(self, reply: &LocalResponse) -> Result<LocalRequest> {
        if !reply.ok {
            bail!(
                "设备准备失败：{}",
                reply.error.as_deref().unwrap_or("请刷新后重试")
            );
        }
        // Use only this preparation response, never an older GUI snapshot.
        let snapshot = reply
            .snapshot
            .as_ref()
            .context("后台未确认设备准备结果，请刷新后重试")?;
        let device = usable_device(snapshot, &self.busid)?;
        if !self.identity.matches(&snapshot.server_id, device) {
            bail!("准备期间设备或 Linux 主机发生变化，请刷新后重新选择");
        }
        if !device.shared {
            bail!("后台尚未完成设备准备，请刷新后重试");
        }
        Ok(LocalRequest::Connect { busid: self.busid })
    }
}

pub fn device_state(
    device: &UsbDevice,
    session: Option<&SessionStatus>,
    live: bool,
    pending: bool,
) -> DeviceState {
    // A cached owner is never evidence of current availability after a failure.
    if !live {
        return DeviceState::Unknown;
    }
    if !device.present {
        return DeviceState::Offline;
    }
    if pending || session.is_some_and(|s| s.state == "connecting") {
        return DeviceState::Connecting;
    }
    if session.is_some_and(|s| s.port.is_some() && s.state == "connected") {
        return DeviceState::Mine;
    }
    if device.owner.is_some() {
        return DeviceState::Occupied;
    }
    if device.protected_reason.is_some() {
        return DeviceState::Protected;
    }
    if session.is_some_and(|s| s.error.is_some()) {
        return DeviceState::Failed;
    }
    if !device.shared {
        return DeviceState::Unshared;
    }
    DeviceState::Available
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device() -> UsbDevice {
        UsbDevice {
            busid: "1-2".into(),
            name: "USB".into(),
            vendor: 1,
            product: 2,
            serial: None,
            path: "/test".into(),
            speed: 3,
            shared: true,
            owner: None,
            protected_reason: None,
            present: true, device_type: None, session_id: None,
        }
    }
    fn snapshot(d: UsbDevice) -> Snapshot {
        Snapshot {
            config: crate::config::ClientConfig::default(),
            paired: true,
            server_id: "linux-a".into(),
            devices: vec![d],
            sessions: vec![],
            driver_error: None,
            network_error: None,
            proxy_port: 0,
            devices_checked_at_ms: Some(1),
        }
    }
    fn prepared_reply(mut s: Snapshot) -> LocalResponse {
        s.devices[0].shared = true;
        LocalResponse {
            ok: true,
            error: None,
            snapshot: Some(s),
            log: None,
            invitation: None,
        }
    }
    #[test]
    fn one_click_prepares_then_connects_the_same_device() {
        let mut d = device();
        d.shared = false;
        let s = snapshot(d);
        let (request, pending) = connection_request(&s, "1-2").unwrap();
        assert!(matches!(
            request,
            LocalRequest::SetShared { shared: true, .. }
        ));
        let next = pending.unwrap().followup(&prepared_reply(s)).unwrap();
        assert!(matches!(next, LocalRequest::Connect { busid } if busid == "1-2"));
    }
    #[test]
    fn ready_device_connects_without_preparation() {
        let (request, pending) = connection_request(&snapshot(device()), "1-2").unwrap();
        assert!(matches!(request, LocalRequest::Connect { .. }));
        assert!(pending.is_none());
    }
    #[test]
    fn failed_or_unconfirmed_preparation_never_connects() {
        for mode in 0..3 {
            let mut d = device();
            d.shared = false;
            let s = snapshot(d);
            let (_, pending) = connection_request(&s, "1-2").unwrap();
            let reply = match mode {
                0 => LocalResponse::error("bind failed"),
                1 => LocalResponse {
                    ok: true,
                    error: None,
                    snapshot: None,
                    log: None,
                    invitation: None,
                },
                _ => LocalResponse {
                    ok: true,
                    error: None,
                    snapshot: Some(s),
                    log: None,
                    invitation: None,
                },
            };
            assert!(pending.unwrap().followup(&reply).is_err());
        }
    }
    #[test]
    fn replaced_device_or_server_after_preparation_never_connects() {
        for mode in 0..3 {
            let mut d = device();
            d.shared = false;
            let s = snapshot(d);
            let (_, pending) = connection_request(&s, "1-2").unwrap();
            let mut reply = prepared_reply(s);
            let next = reply.snapshot.as_mut().unwrap();
            match mode {
                0 => next.server_id = "linux-b".into(),
                1 => next.devices[0].serial = Some("replacement".into()),
                _ => next.devices[0].path = "/new-port".into(),
            }
            assert!(pending.unwrap().followup(&reply).is_err());
        }
    }
    #[test]
    fn occupied_protected_offline_or_driver_failure_never_prepares() {
        for mode in 0..5 {
            let mut d = device();
            d.shared = false;
            let mut s = snapshot(d);
            match mode {
                0 => s.devices[0].owner = Some("peer".into()),
                1 => s.devices[0].protected_reason = Some("系统盘".into()),
                2 => s.devices[0].present = false,
                3 => s.driver_error = Some("driver".into()),
                _ => s.network_error = Some("offline".into()),
            }
            assert!(connection_request(&s, "1-2").is_err());
        }
    }
    #[test]
    fn new_owner_or_network_error_after_preparation_never_connects() {
        for mode in 0..2 {
            let mut d = device();
            d.shared = false;
            let s = snapshot(d);
            let (_, pending) = connection_request(&s, "1-2").unwrap();
            let mut reply = prepared_reply(s);
            if mode == 0 {
                reply.snapshot.as_mut().unwrap().devices[0].owner = Some("peer".into());
            } else {
                reply.snapshot.as_mut().unwrap().network_error = Some("offline".into());
            }
            assert!(pending.unwrap().followup(&reply).is_err());
        }
    }
    #[test]
    fn stale_free_and_stale_owned_devices_are_unknown() {
        let mut d = device();
        assert_eq!(device_state(&d, None, false, false), DeviceState::Unknown);
        d.owner = Some("工作电脑".into());
        let s = SessionStatus {
            busid: d.busid.clone(),
            port: Some(1),
            state: "connected".into(),
            error: None,
        };
        assert_eq!(
            device_state(&d, Some(&s), false, false),
            DeviceState::Unknown
        );
    }
    #[test]
    fn owner_takes_priority_over_old_attach_failure() {
        let mut d = device();
        d.owner = Some("另一台电脑".into());
        let s = SessionStatus {
            busid: d.busid.clone(),
            port: None,
            state: "failed".into(),
            error: Some("old".into()),
        };
        assert_eq!(
            device_state(&d, Some(&s), true, false),
            DeviceState::Occupied
        );
        d.owner = None;
        assert_eq!(device_state(&d, Some(&s), true, false), DeviceState::Failed);
    }
    #[test]
    fn availability_never_overrides_protection_or_missing_device() {
        let mut d = device();
        assert_eq!(device_state(&d, None, true, false), DeviceState::Available);
        d.shared = false;
        assert_eq!(device_state(&d, None, true, false), DeviceState::Unshared);
        d.protected_reason = Some("系统盘".into());
        assert_eq!(device_state(&d, None, true, false), DeviceState::Protected);
        d.present = false;
        assert_eq!(device_state(&d, None, true, false), DeviceState::Offline);
    }
    #[test]
    fn connection_requires_confirmed_driver_port() {
        let d = device();
        let mut s = SessionStatus {
            busid: d.busid.clone(),
            port: None,
            state: "connected".into(),
            error: None,
        };
        assert_eq!(
            device_state(&d, Some(&s), true, false),
            DeviceState::Available
        );
        s.port = Some(1);
        assert_eq!(device_state(&d, Some(&s), true, false), DeviceState::Mine);
        assert_eq!(
            device_state(&d, Some(&s), true, true),
            DeviceState::Connecting
        );
    }
}
