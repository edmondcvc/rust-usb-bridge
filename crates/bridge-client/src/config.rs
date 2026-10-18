use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub server_id: String,
    pub path: String,
    pub vendor: u16,
    pub product: u16,
    pub serial: Option<String>,
}

impl DeviceIdentity {
    pub fn from_device(server_id: &str, device: &bridge_common::UsbDevice) -> Self {
        Self {
            server_id: server_id.into(),
            path: device.path.clone(),
            vendor: device.vendor,
            product: device.product,
            serial: device.serial.clone(),
        }
    }
    pub fn matches(&self, server_id: &str, device: &bridge_common::UsbDevice) -> bool {
        self == &Self::from_device(server_id, device)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientConfig {
    pub host_disconnected: bool,
    pub host: String,
    pub port: u16,
    pub server_name: String,
    pub display_name: String,
    pub proxy_port: u16,
    pub client_id: Option<String>,
    pub credential_generation: Option<String>,
    pub auto_connect: Vec<String>,
    pub auto_connect_identities: BTreeMap<String, DeviceIdentity>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            host_disconnected: false,
            host: String::new(),
            port: 3241,
            server_name: "usb-bridge".into(),
            display_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows 电脑".into()),
            proxy_port: 0,
            client_id: None,
            credential_generation: None,
            auto_connect: Vec::new(),
            auto_connect_identities: BTreeMap::new(),
        }
    }
}

impl ClientConfig {
    pub fn validate(&self) -> Result<()> {
        if (self.host.is_empty() && (self.client_id.is_some() || !self.auto_connect.is_empty()))
            || self.host.len() > 255
            || self.host.chars().any(char::is_whitespace)
        {
            bail!("Linux 地址无效");
        }
        if self.port == 0 || self.server_name.is_empty() || self.display_name.trim().is_empty() {
            bail!("端口、证书名称和电脑名称不能为空");
        }
        if self.display_name.len() > 120 {
            bail!("电脑名称过长");
        }
        if let Some(generation) = &self.credential_generation {
            validate_generation(generation)?;
        }
        for busid in &self.auto_connect {
            validate_busid(busid)?;
        }
        for busid in self.auto_connect_identities.keys() {
            validate_busid(busid)?;
        }
        Ok(())
    }
}

pub fn validate_busid(busid: &str) -> Result<()> {
    if !bridge_common::valid_busid(busid) {
        bail!("USB 端口标识无效");
    }
    Ok(())
}

pub fn default_path() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"C:\ProgramData\RustUsbBridge\client.json")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("client.json")
    }
}

pub fn load(path: &Path) -> Result<ClientConfig> {
    let config = if path.exists() {
        serde_json::from_slice(&std::fs::read(path).context("读取客户端配置失败")?)
            .context("客户端配置格式无效")?
    } else {
        ClientConfig::default()
    };
    ClientConfig::validate(&config)?;
    Ok(config)
}

pub fn directory(path: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_owned()
}

pub fn credentials_dir(path: &Path) -> PathBuf {
    directory(path).join("credentials")
}

fn validate_generation(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        bail!("客户端证书代号无效");
    }
    Ok(())
}

pub fn active_credentials_dir(path: &Path) -> Result<PathBuf> {
    let config = load(path)?;
    let root = credentials_dir(path);
    Ok(match config.credential_generation {
        Some(value) => root.join(value),
        None => root,
    })
}

pub fn save(path: &Path, config: &ClientConfig) -> Result<()> {
    config.validate()?;
    let parent = directory(path);
    std::fs::create_dir_all(&parent)?;
    let pending = path.with_extension("json.new");
    let data = serde_json::to_vec_pretty(config)?;
    write_atomic(&pending, path, &data)
}

pub fn write_atomic(pending: &Path, destination: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(pending)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    #[cfg(windows)]
    crate::security::replace_file(pending, destination)?;
    #[cfg(not(windows))]
    std::fs::rename(pending, destination)?;
    Ok(())
}

pub fn save_pairing(path: &Path, pair: &bridge_common::PairingBundle) -> Result<()> {
    validate_generation(&pair.client_id)?;
    let root = credentials_dir(path);
    std::fs::create_dir_all(&root)?;
    #[cfg(windows)]
    crate::security::restrict_directory(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    }
    // Each certificate generation has its own private directory. The config's
    // atomic replacement is the commit point; a write failure cannot corrupt
    // a previously committed client's certificate/key pair.
    let directory = root.join(&pair.client_id);
    std::fs::create_dir(&directory).context("创建新配对证书目录失败")?;
    #[cfg(windows)]
    crate::security::restrict_directory(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    for (filename, value) in [
        ("ca.pem", &pair.ca_cert_pem),
        ("client.pem", &pair.client_cert_pem),
        ("client.key", &pair.client_key_pem),
    ] {
        let target = directory.join(filename);
        write_atomic(
            &directory.join(format!("{filename}.new")),
            &target,
            value.as_bytes(),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_round_trip_preserves_proxy_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("client.json");
        let mut value = ClientConfig::default();
        value.proxy_port = 41234;
        save(&path, &value).unwrap();
        assert_eq!(load(&path).unwrap().proxy_port, 41234);
        value.proxy_port = 41235;
        save(&path, &value).unwrap();
        assert_eq!(load(&path).unwrap().proxy_port, 41235);
    }
    #[test]
    fn credential_generation_commit_preserves_existing_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("client.json");
        let mut config = ClientConfig::default();
        save(&path, &config).unwrap();
        let pair = bridge_common::PairingBundle {
            client_id: "test-client-one".into(),
            ca_cert_pem: "test-ca".into(),
            client_cert_pem: "test-certificate".into(),
            client_key_pem: "test-private-key".into(),
        };
        save_pairing(&path, &pair).unwrap();
        assert_eq!(load(&path).unwrap().credential_generation, None);
        let private = credentials_dir(&path)
            .join("test-client-one")
            .join("client.key");
        assert_eq!(
            std::fs::read_to_string(&private).unwrap(),
            "test-private-key"
        );
        config.credential_generation = Some(pair.client_id.clone());
        config.client_id = Some(pair.client_id.clone());
        config.host = "127.0.0.1".into();
        save(&path, &config).unwrap();
        assert_eq!(
            active_credentials_dir(&path).unwrap(),
            private.parent().unwrap()
        );
        assert!(save_pairing(&path, &pair).is_err());
        assert_eq!(
            std::fs::read_to_string(&private).unwrap(),
            "test-private-key"
        );
        let mut invalid = pair;
        invalid.client_id = "../escape".into();
        assert!(save_pairing(&path, &invalid).is_err());
    }
    #[test]
    fn fresh_install_has_no_sample_host_but_paired_config_requires_one() {
        let mut config = ClientConfig::default();
        assert!(config.host.is_empty());
        assert!(config.validate().is_ok());
        config.client_id = Some("paired".into());
        assert!(config.validate().is_err());
        config.host = "192.0.2.10".into();
        assert!(config.validate().is_ok());
    }
    #[test]
    fn untrusted_busids_rejected() {
        for bad in ["", "../../x", "1-1\0", "$(echo x)", "1-1\n", "1-1:1.0"] {
            assert!(validate_busid(bad).is_err());
        }
        for good in ["1-1", "2-1.2.3", "12-4"] {
            validate_busid(good).unwrap();
        }
    }
    #[test]
    fn automatic_connect_identity_rejects_replaced_devices_or_hosts() {
        let device = bridge_common::UsbDevice {
            busid: "1-2".into(),
            name: "storage".into(),
            vendor: 0x1234,
            product: 0x4321,
            serial: Some("real123".into()),
            path: "/sys/devices/usb1/1-2".into(),
            speed: 3,
            shared: true,
            owner: None,
            protected_reason: None,
            present: true, device_type: None, session_id: None,
        };
        let identity = DeviceIdentity::from_device("host-one", &device);
        assert!(identity.matches("host-one", &device));
        assert!(!identity.matches("host-two", &device));
        let mut replaced = device.clone();
        replaced.serial = Some("real456".into());
        assert!(!identity.matches("host-one", &replaced));
        replaced = device.clone();
        replaced.product = 0x4322;
        assert!(!identity.matches("host-one", &replaced));
        replaced = device.clone();
        replaced.path = "/sys/devices/usb2/2-2".into();
        assert!(!identity.matches("host-one", &replaced));
    }
}
