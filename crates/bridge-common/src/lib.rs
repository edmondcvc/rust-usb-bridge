//! Stable control wire format and USB/IP 1.1.1 negotiation primitives.
use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const CONTROL_SELECTOR: &[u8] = b"control\n";
pub const USBIP_SELECTOR: &[u8] = b"usbip\n";
pub const JSON_MAX_BYTES: usize = 1024 * 1024;
pub const USBIP_VERSION: u16 = 0x0111;
pub const OP_REQ_IMPORT: u16 = 0x8003;
pub const OP_REP_IMPORT: u16 = 0x0003;
pub const OP_REQ_DEVLIST: u16 = 0x8005;
pub const OP_REP_DEVLIST: u16 = 0x0005;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ControlRequest {
    ListDevices,
    SetShared { busid: String, shared: bool },
    Pair { code: String, name: String },
    Status,
    CreateInvitation,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ControlResponse {
    pub ok: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub devices: Vec<UsbDevice>,
    pub pairing: Option<PairingBundle>,
    pub server_id: String,
    #[serde(default)]
    pub invitation: Option<EnrollmentInvitation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentInvitation {
    pub code: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbDevice {
    #[serde(default)]
    pub device_type: Option<String>,
    /// Opaque observation identifier, never an authentication credential.
    #[serde(default)]
    pub session_id: Option<String>,
    pub busid: String,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub serial: Option<String>,
    pub path: String,
    pub speed: u32,
    pub shared: bool,
    pub owner: Option<String>,
    pub protected_reason: Option<String>,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForceReleaseRequest {
    pub server_id: String,
    pub busid: String,
    pub session_id: String,
    pub path: String,
    pub vendor: u16,
    pub product: u16,
    pub serial: Option<String>,
}

pub fn usb_device_type(vendor: u16, product: u16, class: u8, interfaces: &[[u8; 4]]) -> &'static str {
    if matches!(vendor, 0x1a86 | 0x4348) && product == 0x5512 { return "雕刻机接口"; }
    let mut kinds = std::collections::BTreeSet::new();
    for c in std::iter::once(class).chain(interfaces.iter().map(|i| i[0])) {
        let kind = match c { 1 => "音频设备", 2 | 10 => "通信设备", 3 => "输入设备", 7 => "打印设备", 8 => "存储设备", 9 => "USB 集线器", 14 => "摄像头", _ => continue };
        kinds.insert(kind);
    }
    if kinds.len() > 1 { return "复合 USB 设备"; }
    if interfaces.iter().any(|i| i[0] == 2 && i[1] == 2) && kinds.len() == 1 { return "串口设备"; }
    kinds.into_iter().next().unwrap_or("其他 USB 设备")
}

#[cfg(test)]
#[test]
fn class_information_and_old_device_payloads_are_compatible() {
    assert_eq!(usb_device_type(0x1a86, 0x5512, 255, &[]), "雕刻机接口");
    assert_eq!(usb_device_type(1, 2, 0, &[[14, 1, 0, 0], [14, 2, 0, 0]]), "摄像头");
    assert_eq!(usb_device_type(1, 2, 0, &[[14, 1, 0, 0], [1, 2, 0, 0]]), "复合 USB 设备");
    assert_eq!(usb_device_type(1, 2, 0, &[[2, 2, 0, 0], [10, 0, 0, 0]]), "串口设备");
    assert_eq!(usb_device_type(1, 2, 255, &[]), "其他 USB 设备");
    let device: UsbDevice = serde_json::from_value(serde_json::json!({"busid":"1-2","name":"old","vendor":1,"product":2,"serial":null,"path":"/usb","speed":3,"shared":true,"owner":null,"protected_reason":null,"present":true})).unwrap();
    assert!(device.device_type.is_none() && device.session_id.is_none());
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingBundle {
    pub client_id: String,
    pub ca_cert_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

pub async fn read_json<R: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
    reader: &mut R,
) -> io::Result<T> {
    let length = reader.read_u32().await? as usize;
    if length == 0 || length > JSON_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid control frame length",
        ));
    }
    let mut data = vec![0; length];
    reader.read_exact(&mut data).await?;
    serde_json::from_slice(&data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub async fn write_json<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> io::Result<()> {
    let data =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if data.is_empty() || data.len() > JSON_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "control frame too large",
        ));
    }
    writer.write_u32(data.len() as u32).await?;
    writer.write_all(&data).await?;
    writer.flush().await
}

/// Physical-device sysnames only; never interface names or paths.
pub fn valid_busid(value: &str) -> bool {
    if value.is_empty() || value.len() >= 32 {
        return false;
    }
    let Some((bus, port)) = value.split_once('-') else {
        return false;
    };
    !bus.is_empty()
        && bus.bytes().all(|b| b.is_ascii_digit())
        && port
            .split('.')
            .all(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbIpHeader {
    pub version: u16,
    pub code: u16,
    pub status: u32,
}
impl UsbIpHeader {
    pub fn request(code: u16) -> Self {
        Self {
            version: USBIP_VERSION,
            code,
            status: 0,
        }
    }
    pub fn encode(self) -> [u8; 8] {
        let mut bytes = [0; 8];
        bytes[..2].copy_from_slice(&self.version.to_be_bytes());
        bytes[2..4].copy_from_slice(&self.code.to_be_bytes());
        bytes[4..].copy_from_slice(&self.status.to_be_bytes());
        bytes
    }
    pub fn decode(bytes: [u8; 8]) -> Self {
        Self {
            version: u16::from_be_bytes(bytes[..2].try_into().unwrap()),
            code: u16::from_be_bytes(bytes[2..4].try_into().unwrap()),
            status: u32::from_be_bytes(bytes[4..].try_into().unwrap()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsbIpDescriptor {
    pub path: String,
    pub busid: String,
    pub busnum: u32,
    pub devnum: u32,
    pub speed: u32,
    pub vendor: u16,
    pub product: u16,
    pub bcd_device: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub configuration: u8,
    pub configurations: u8,
    pub interfaces: Vec<[u8; 4]>,
}
impl UsbIpDescriptor {
    pub fn encode(&self) -> io::Result<[u8; 312]> {
        if self.path.len() >= 256 || !valid_busid(&self.busid) || self.interfaces.len() > 255 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid USB/IP descriptor",
            ));
        }
        let mut result = [0; 312];
        result[..self.path.len()].copy_from_slice(self.path.as_bytes());
        result[256..256 + self.busid.len()].copy_from_slice(self.busid.as_bytes());
        for (offset, value) in [(288, self.busnum), (292, self.devnum), (296, self.speed)] {
            result[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        for (offset, value) in [
            (300, self.vendor),
            (302, self.product),
            (304, self.bcd_device),
        ] {
            result[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
        }
        result[306..].copy_from_slice(&[
            self.device_class,
            self.device_subclass,
            self.device_protocol,
            self.configuration,
            self.configurations,
            self.interfaces.len() as u8,
        ]);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_path_and_interface_busids() {
        for value in [
            "../1-2",
            "1-2:1.0",
            "1-",
            "-2",
            "1-2/driver",
            "1-2..3",
            "usb1",
        ] {
            assert!(!valid_busid(value));
        }
        for value in ["1-2", "1-2.10.3", "10-12"] {
            assert!(valid_busid(value));
        }
    }
    #[test]
    fn device_wire_layout_is_network_order() {
        let data = UsbIpDescriptor {
            path: "/sys/devices/usb1/1-2".into(),
            busid: "1-2".into(),
            busnum: 1,
            devnum: 7,
            speed: 3,
            vendor: 0x1a86,
            product: 0x5512,
            interfaces: vec![[0xff, 0, 0, 0]],
            ..Default::default()
        }
        .encode()
        .unwrap();
        assert_eq!(data.len(), 312);
        assert_eq!(&data[300..304], &[0x1a, 0x86, 0x55, 0x12]);
        assert_eq!(data[311], 1);
        assert_eq!(
            UsbIpHeader::request(OP_REQ_IMPORT).encode(),
            [1, 0x11, 0x80, 3, 0, 0, 0, 0]
        );
    }
    #[tokio::test]
    async fn rejects_huge_json_before_allocation() {
        let mut bytes: &[u8] = &[0xff, 0xff, 0xff, 0xff];
        assert_eq!(
            read_json::<_, ControlRequest>(&mut bytes)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
