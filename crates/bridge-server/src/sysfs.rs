//! Linux sysfs backend. Mock mode is deliberately isolated from real /sys.
use crate::config::SharedRecord;
use anyhow::{bail, Context, Result};
use bridge_common::{valid_busid, UsbDevice, UsbIpDescriptor};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Backend {
    pub root: PathBuf,
    pub proc_root: PathBuf,
    pub mock: bool,
}
#[derive(Debug, Clone)]
pub struct Device {
    pub public: UsbDevice,
    pub descriptor: UsbIpDescriptor,
    pub driver: Option<String>,
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}
fn decimal(path: impl AsRef<Path>) -> u32 {
    read(path).and_then(|v| v.parse().ok()).unwrap_or(0)
}
fn hexadecimal(path: impl AsRef<Path>) -> u16 {
    read(path)
        .and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0)
}
fn canonical(path: impl AsRef<Path>) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}
fn entries(path: impl AsRef<Path>) -> Vec<PathBuf> {
    fs::read_dir(path)
        .map(|es| es.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

impl Backend {
    pub fn new(root: PathBuf, proc_root: PathBuf, mock: bool) -> Result<Self> {
        let resolved = fs::canonicalize(&root).context("sysfs root does not exist")?;
        let real_sys = fs::canonicalize("/sys").unwrap_or_else(|_| PathBuf::from("/sys"));
        if mock && resolved.starts_with(&real_sys) {
            bail!("mock mode refuses the real /sys and its descendants");
        }
        if !mock && resolved != real_sys {
            bail!("alternate sysfs roots require --mock");
        }
        if mock && proc_root == Path::new("/proc") {
            bail!("mock mode requires an isolated --proc-root");
        }
        if !mock {
            let resolved_proc = fs::canonicalize(&proc_root).context("proc root does not exist")?;
            let real_proc = fs::canonicalize("/proc")?;
            if resolved_proc != real_proc {
                bail!("alternate proc roots require --mock");
            }
        }
        Ok(Self {
            root: resolved,
            proc_root,
            mock,
        })
    }
    fn usb_devices(&self) -> PathBuf {
        self.root.join("bus/usb/devices")
    }
    fn drivers(&self) -> PathBuf {
        self.root.join("bus/usb/drivers")
    }
    fn device_path(&self, busid: &str) -> Result<PathBuf> {
        if !valid_busid(busid) {
            bail!("invalid physical USB bus ID");
        }
        let path = self.usb_devices().join(busid);
        let actual = fs::canonicalize(&path).context("USB device is offline")?;
        if !actual.starts_with(&self.root) {
            bail!("USB device path escapes sysfs root");
        }
        Ok(path)
    }
    fn write(&self, path: impl AsRef<Path>, value: &str) -> Result<()> {
        let path = path.as_ref();
        if !path.starts_with(&self.root) {
            bail!("write escapes selected sysfs root");
        }
        let actual = fs::canonicalize(path)
            .with_context(|| format!("sysfs attribute {} is missing", path.display()))?;
        if !actual.starts_with(&self.root) {
            bail!("sysfs attribute symlink escapes selected root");
        }
        // Do not create attributes: a missing kernel interface must fail closed.
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(self.mock)
            .open(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        file.write_all(value.as_bytes())
            .with_context(|| format!("sysfs rejected {}", path.display()))
    }
    pub fn discover(&self) -> Result<Vec<Device>> {
        let protected = self.protected_paths()?;
        let mut result = Vec::new();
        for path in entries(self.usb_devices()) {
            let busid = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if !valid_busid(&busid) || !path.join("idVendor").exists() {
                continue;
            }
            let Some(actual) = canonical(&path) else {
                continue;
            };
            if !actual.starts_with(&self.root) {
                continue;
            }
            let class = hexadecimal(path.join("bDeviceClass")) as u8;
            let mut interfaces = Vec::new();
            for interface in entries(self.usb_devices()) {
                let name = interface.file_name().unwrap_or_default().to_string_lossy();
                if name.starts_with(&format!("{busid}:")) {
                    interfaces.push((
                        name.to_string(),
                        [
                            hexadecimal(interface.join("bInterfaceClass")) as u8,
                            hexadecimal(interface.join("bInterfaceSubClass")) as u8,
                            hexadecimal(interface.join("bInterfaceProtocol")) as u8,
                            0,
                        ],
                    ));
                }
            }
            interfaces.sort_by(|a, b| a.0.cmp(&b.0));
            let interfaces: Vec<_> = interfaces.into_iter().map(|i| i.1).collect();
            let protected_reason = if class == 9 || interfaces.iter().any(|i| i[0] == 9) {
                Some("USB 集线器本身受保护；可以共享其下的普通设备".into())
            } else if class == 3 || interfaces.iter().any(|i| i[0] == 3) {
                Some("维护输入设备受保护".into())
            } else if actual.to_string_lossy().contains("vhci_hcd") {
                Some("禁止将已通过 USB/IP 导入的设备重新导出".into())
            } else {
                protected
                    .iter()
                    .find(|(p, _)| p.starts_with(&actual))
                    .map(|(_, reason)| reason.clone())
            };
            let vendor = hexadecimal(path.join("idVendor"));
            let product = hexadecimal(path.join("idProduct"));
            let serial = read(path.join("serial")).filter(|s| !s.is_empty());
            let speed = match read(path.join("speed"))
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0) as u32
            {
                1 => 1,
                12 => 2,
                480 => 3,
                5000 => 5,
                x if x >= 10000 => 6,
                _ => 0,
            };
            let manufacturer = read(path.join("manufacturer")).unwrap_or_default();
            let product_name = read(path.join("product"))
                .unwrap_or_else(|| format!("USB {vendor:04x}:{product:04x}"));
            let name = format!("{manufacturer} {product_name}").trim().to_string();
            let driver = if self.mock {
                read(path.join("mock-driver")).filter(|s| !s.is_empty())
            } else {
                canonical(path.join("driver"))
                    .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            };
            let public = UsbDevice {
                busid: busid.clone(),
                name,
                vendor,
                product,
                serial,
                path: actual.to_string_lossy().to_string(),
                speed,
                shared: false,
                owner: None,
                protected_reason,
                present: true,
                device_type: Some(bridge_common::usb_device_type(vendor, product, class, &interfaces).into()),
                session_id: None,
            };
            let descriptor = UsbIpDescriptor {
                path: actual.to_string_lossy().to_string(),
                busid,
                busnum: decimal(path.join("busnum")),
                devnum: decimal(path.join("devnum")),
                speed,
                vendor,
                product,
                bcd_device: hexadecimal(path.join("bcdDevice")),
                device_class: class,
                device_subclass: hexadecimal(path.join("bDeviceSubClass")) as u8,
                device_protocol: hexadecimal(path.join("bDeviceProtocol")) as u8,
                configuration: decimal(path.join("bConfigurationValue")) as u8,
                configurations: decimal(path.join("bNumConfigurations")) as u8,
                interfaces,
            };
            result.push(Device {
                public,
                descriptor,
                driver,
            });
        }
        result.sort_by(|a, b| a.public.busid.cmp(&b.public.busid));
        Ok(result)
    }

    fn protected_paths(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut result = Vec::new();
        let mounts = fs::read_to_string(self.proc_root.join("self/mountinfo"))
            .context("cannot read mount protection metadata; USB sharing is disabled")?;
        {
            for line in mounts.lines() {
                let parts: Vec<_> = line.split_whitespace().collect();
                if parts.len() < 6 {
                    continue;
                }
                let mount = parts[4]
                    .replace("\\040", " ")
                    .replace("\\011", "\t")
                    .replace("\\134", "\\");
                let reason = if mount == "/" || mount == "/boot" || mount.starts_with("/boot/") {
                    format!("Linux 系统或启动存储受保护：{mount}")
                } else {
                    format!("存储正在挂载：{mount}；请先安全卸载")
                };
                let mut visited = BTreeSet::new();
                self.block_paths(
                    &self.root.join("dev/block").join(parts[2]),
                    &reason,
                    &mut visited,
                    &mut result,
                );
                // Btrfs and some layered mounts use an anonymous major/minor.
                // Resolve a real /dev mount source as a second independent path.
                if let Some(separator) = parts.iter().position(|p| *p == "-") {
                    if let Some(source) = parts.get(separator + 2) {
                        let source = source.split('[').next().unwrap_or(source);
                        if source.starts_with("/dev/") {
                            if let Some(device) =
                                canonical(source).and_then(|p| p.file_name().map(|n| n.to_owned()))
                            {
                                self.block_paths(
                                    &self.root.join("class/block").join(device),
                                    &reason,
                                    &mut visited,
                                    &mut result,
                                );
                            }
                        }
                    }
                }
            }
        }
        if !self.mock {
            fs::read_dir(self.root.join("class/net"))
                .context("cannot read network protection metadata; USB sharing is disabled")?;
        }
        for net in entries(self.root.join("class/net")) {
            if net.file_name().is_some_and(|n| n == "lo") {
                continue;
            }
            let flags = read(net.join("flags"))
                .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);
            if flags & 1 != 0 || read(net.join("operstate")).as_deref() == Some("up") {
                if let Some(path) = canonical(net.join("device")) {
                    result.push((path, "正在使用的 Linux 网络适配器受保护".into()));
                }
            }
        }
        // Active userspace libusb access (including an active VirtualHere session).
        for process in entries(&self.proc_root) {
            let pid = process
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if !pid.bytes().all(|b| b.is_ascii_digit()) || pid.is_empty() {
                continue;
            }
            for fd in entries(process.join("fd")) {
                let Ok(target) = fs::read_link(fd) else {
                    continue;
                };
                let value = target.to_string_lossy();
                if !value.starts_with("/dev/bus/usb/") {
                    continue;
                }
                let bits: Vec<_> = value.rsplit('/').take(2).collect();
                if bits.len() != 2 {
                    continue;
                }
                let (Ok(devnum), Ok(busnum)) = (bits[0].parse::<u32>(), bits[1].parse::<u32>())
                else {
                    continue;
                };
                for device in entries(self.usb_devices()) {
                    if decimal(device.join("busnum")) == busnum
                        && decimal(device.join("devnum")) == devnum
                    {
                        if let Some(path) = canonical(device) {
                            result.push((
                                path,
                                format!("设备正在被 Linux 用户进程占用：PID {pid}；请先释放原连接"),
                            ));
                        }
                    }
                }
            }
        }
        Ok(result)
    }
    fn block_paths(
        &self,
        path: &Path,
        reason: &str,
        visited: &mut BTreeSet<PathBuf>,
        result: &mut Vec<(PathBuf, String)>,
    ) {
        let Some(path) = canonical(path) else {
            return;
        };
        if !path.starts_with(&self.root) || !visited.insert(path.clone()) {
            return;
        }
        result.push((path.clone(), reason.into()));
        for slave in entries(path.join("slaves")) {
            self.block_paths(&slave, reason, visited, result);
        }
        // A partition's parent block device can be an LVM/MD backing device.
        if path.join("partition").exists() {
            if let Some(parent) = path.parent() {
                for slave in entries(parent.join("slaves")) {
                    self.block_paths(&slave, reason, visited, result);
                }
            }
        }
    }

    pub fn identity_matches(device: &Device, record: &SharedRecord) -> bool {
        device.public.vendor == record.vendor
            && device.public.product == record.product
            && device.public.serial == record.serial
            && device.public.path == record.path
    }
    pub fn record(device: &Device) -> SharedRecord {
        SharedRecord {
            vendor: device.public.vendor,
            product: device.public.product,
            serial: device.public.serial.clone(),
            path: device.public.path.clone(),
            original_driver: device.driver.clone(),
        }
    }
    pub fn bind(&self, device: &Device) -> Result<()> {
        if let Some(reason) = &device.public.protected_reason {
            bail!("{reason}");
        }
        // Recheck immediately before changing drivers so a mount or userspace
        // holder appearing after the control list cannot be silently displaced.
        let current = self
            .discover()?
            .into_iter()
            .find(|d| d.public.busid == device.public.busid)
            .context("USB device is offline")?;
        if !Self::identity_matches(&current, &Self::record(device)) {
            bail!("USB identity changed before sharing");
        }
        if let Some(reason) = current.public.protected_reason {
            bail!("{reason}");
        }
        if current.driver != device.driver {
            bail!("USB driver changed before sharing; refresh the device list");
        }
        if device.driver.as_deref() == Some("usbip-host") {
            bail!("device already belongs to another USB/IP exporter");
        }
        let drivers = self.drivers();
        if !drivers.join("usbip-host/match_busid").exists() {
            bail!("usbip-host kernel module is not loaded");
        }
        let busid = &device.public.busid;
        if let Some(driver) = &device.driver {
            self.write(drivers.join(driver).join("unbind"), busid)?;
        }
        let attempt = (|| -> Result<()> {
            self.write(
                drivers.join("usbip-host/match_busid"),
                &format!("add {busid}"),
            )?;
            self.write(drivers.join("usbip-host/bind"), busid)?;
            if self.mock {
                let path = self.device_path(busid)?;
                self.write(path.join("usbip_status"), "1")?;
                self.write(path.join("mock-driver"), "usbip-host")?;
            }
            Ok(())
        })();
        if let Err(error) = attempt {
            let _ = self.write(
                drivers.join("usbip-host/match_busid"),
                &format!("del {busid}"),
            );
            if let Some(driver) = &device.driver {
                let _ = self.write(drivers.join(driver).join("bind"), busid);
            }
            return Err(error);
        }
        Ok(())
    }
    pub fn restore(&self, busid: &str, record: &SharedRecord) -> Result<()> {
        let Some(device) = self
            .discover()?
            .into_iter()
            .find(|d| d.public.busid == busid)
        else {
            return Ok(());
        };
        if !Self::identity_matches(&device, record) {
            bail!("device identity changed; refusing to restore {busid}");
        }
        if self.mock {
            self.write(
                self.device_path(busid)?.join("mock-driver"),
                record.original_driver.as_deref().unwrap_or(""),
            )?;
            return Ok(());
        }
        if device.driver.as_deref() != Some("usbip-host") {
            // Recover a crash after the original driver was unbound but before
            // usbip-host was bound. Do not disturb a newly attached driver.
            if device.driver.is_none() {
                if let Some(driver) = &record.original_driver {
                    if driver.is_empty()
                        || driver
                            .bytes()
                            .any(|b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
                    {
                        bail!("invalid recorded driver name");
                    }
                    self.write(self.drivers().join(driver).join("bind"), busid)?;
                }
            }
            return Ok(());
        }
        self.disconnect(busid)?;
        self.write(self.drivers().join("usbip-host/unbind"), busid)?;
        self.write(
            self.drivers().join("usbip-host/match_busid"),
            &format!("del {busid}"),
        )?;
        // This kernel interface reattaches the generic USB driver and probes all
        // original interface drivers, matching upstream usbip unbind behavior.
        self.write(self.drivers().join("usbip-host/rebind"), busid)?;
        Ok(())
    }
    pub fn status(&self, busid: &str) -> Result<u32> {
        Ok(decimal(self.device_path(busid)?.join("usbip_status")))
    }
    pub fn disconnect(&self, busid: &str) -> Result<()> {
        if self.status(busid)? == 2 {
            self.write(self.device_path(busid)?.join("usbip_sockfd"), "-1")?;
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    pub fn handoff(&self, busid: &str, socket: &std::net::TcpStream) -> Result<()> {
        use std::os::fd::AsRawFd;
        if self.mock {
            bail!("mock backend never hands a socket to a kernel");
        }
        if self.status(busid)? != 1 {
            bail!("kernel USB/IP device is not available");
        }
        self.write(
            self.device_path(busid)?.join("usbip_sockfd"),
            &format!("{}\n", socket.as_raw_fd()),
        )
    }
    #[cfg(not(target_os = "linux"))]
    pub fn handoff(&self, _busid: &str, _socket: &std::net::TcpStream) -> Result<()> {
        bail!("kernel handoff is Linux-only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Backend) {
        let dir = tempfile::tempdir().unwrap();
        let sys = dir.path().join("sys");
        let proc = dir.path().join("proc");
        fs::create_dir_all(sys.join("bus/usb/devices/1-2")).unwrap();
        fs::create_dir_all(proc.join("self")).unwrap();
        fs::write(proc.join("self/mountinfo"), "").unwrap();
        for (name, value) in [
            ("idVendor", "1a86"),
            ("idProduct", "5512"),
            ("bDeviceClass", "00"),
            ("busnum", "1"),
            ("devnum", "2"),
            ("speed", "480"),
        ] {
            fs::write(sys.join("bus/usb/devices/1-2").join(name), value).unwrap();
        }
        let backend = Backend::new(sys, proc, true).unwrap();
        (dir, backend)
    }
    #[cfg(unix)]
    #[test]
    fn input_interfaces_and_hubs_are_protected() {
        let (_dir, backend) = fixture();
        let interface = backend.usb_devices().join("1-2:1.0");
        fs::create_dir(&interface).unwrap();
        fs::write(interface.join("bInterfaceClass"), "03").unwrap();
        assert!(backend.discover().unwrap()[0]
            .public
            .protected_reason
            .is_some());
        fs::write(interface.join("bInterfaceClass"), "ff").unwrap();
        assert!(backend.discover().unwrap()[0]
            .public
            .protected_reason
            .is_none());
        fs::write(backend.usb_devices().join("1-2/bDeviceClass"), "09").unwrap();
        assert!(backend.discover().unwrap()[0]
            .public
            .protected_reason
            .is_some());
    }
    #[test]
    fn identity_rejects_replaced_device_and_unsafe_root() {
        let (_dir, backend) = fixture();
        let first = backend.discover().unwrap().remove(0);
        let record = Backend::record(&first);
        fs::write(backend.usb_devices().join("1-2/idProduct"), "5513").unwrap();
        assert!(!Backend::identity_matches(
            &backend.discover().unwrap()[0],
            &record
        ));
        assert!(Backend::new(backend.root.clone(), backend.proc_root.clone(), false).is_err());
    }
    #[test]
    fn missing_mount_metadata_fails_closed() {
        let (_dir, backend) = fixture();
        fs::remove_file(backend.proc_root.join("self/mountinfo")).unwrap();
        assert!(backend.discover().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn mock_attribute_symlink_cannot_write_outside_the_sandbox() {
        use std::os::unix::fs::symlink;
        let (dir, backend) = fixture();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, "unchanged").unwrap();
        let attribute = backend.usb_devices().join("1-2/usbip_sockfd");
        symlink(&outside, &attribute).unwrap();
        assert!(backend.write(attribute, "-1").is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "unchanged");
    }
    #[cfg(unix)]
    #[test]
    fn mounted_root_protection_follows_mapper_slaves_to_usb_ancestor() {
        use std::os::unix::fs::symlink;
        let (_dir, backend) = fixture();
        let usb = backend.usb_devices().join("1-2");
        let partition = usb.join("host0/target0/block/sda/sda1");
        fs::create_dir_all(&partition).unwrap();
        fs::write(partition.join("partition"), "1").unwrap();
        let mapper = backend.root.join("devices/virtual/block/dm-0");
        fs::create_dir_all(mapper.join("slaves")).unwrap();
        symlink(&partition, mapper.join("slaves/sda1")).unwrap();
        fs::create_dir_all(backend.root.join("dev/block")).unwrap();
        symlink(&mapper, backend.root.join("dev/block/253:0")).unwrap();
        fs::write(
            backend.proc_root.join("self/mountinfo"),
            "20 1 253:0 / / rw - ext4 /dev/mapper/root rw\n",
        )
        .unwrap();
        let devices = backend.discover().unwrap();
        assert!(devices[0]
            .public
            .protected_reason
            .as_ref()
            .unwrap()
            .contains("系统"));
        assert!(backend.bind(&devices[0]).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn active_usb_network_and_userspace_fd_are_protected() {
        use std::os::unix::fs::symlink;
        let (_dir, backend) = fixture();
        let net = backend.root.join("class/net/eth-usb");
        fs::create_dir_all(&net).unwrap();
        let interface = backend.usb_devices().join("1-2:1.0");
        fs::create_dir_all(&interface).unwrap();
        // Match a real sysfs interface beneath its physical-device ancestry.
        let child = backend.usb_devices().join("1-2/1-2:1.0");
        fs::create_dir_all(&child).unwrap();
        symlink(&child, net.join("device")).unwrap();
        fs::write(net.join("flags"), "0x1").unwrap();
        assert!(backend.discover().unwrap()[0]
            .public
            .protected_reason
            .as_ref()
            .unwrap()
            .contains("网络"));
        fs::write(net.join("flags"), "0x0").unwrap();
        let fds = backend.proc_root.join("123/fd");
        fs::create_dir_all(&fds).unwrap();
        symlink("/dev/bus/usb/001/002", fds.join("4")).unwrap();
        assert!(backend.discover().unwrap()[0]
            .public
            .protected_reason
            .as_ref()
            .unwrap()
            .contains("PID 123"));
    }
}
