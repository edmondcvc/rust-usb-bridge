//! Direct ABI for usbip-win2 v.0.9.8.1, include/usbip/vhci.h.
//! No serial override and no persistent attach request are sent.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const PINNED_DRIVER_VERSION: &str = "0.9.8.1";
// The release/ABI version is not the WDK StampInf DriverVer. This exact signed
// x64 release (SHA256 38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a)
// installs usbip2_ude.inf DriverVer=09/22/2026,12.1.30.403.
pub const PINNED_PNP_DRIVER_VERSION: &str = "12.1.30.403";
pub const PINNED_DRIVER_PROVIDER: &str = "USBIP-WIN2";
pub const PINNED_DRIVER_SERVICE: &str = "usbip2_ude";
pub const IOCTL_PLUGIN_ONCE: u32 = 0x22e018;
pub const IOCTL_PLUGOUT: u32 = 0x22e004;
pub const IOCTL_GET_IMPORTED: u32 = 0x22e008;
pub const IOCTL_STOP_ATTEMPTS: u32 = 0x22e014;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Location {
    pub port: i32,
    pub location_hash: u32,
    pub busid: [u8; 32],
    pub service: [u8; 32],
    pub host: [u8; 1025],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Properties {
    pub devid: u32,
    pub speed: i32,
    pub vendor: u16,
    pub product: u16,
    pub serial: [u8; 16],
    pub iserial: u8,
    pub wsk_events: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ImportedDevice {
    pub location: Location,
    pub properties: Properties,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginRequest {
    pub size: u32,
    pub location: Location,
    pub serial: [u8; 16],
    pub wsk_events: u8,
}

#[repr(C)]
pub struct StopRequest {
    pub size: u32,
    pub location: Location,
    pub count: i32,
}

#[repr(C)]
pub struct PlugoutRequest {
    pub size: u32,
    pub port: i32,
}

const _: () = assert!(std::mem::size_of::<Location>() == 1100);
const _: () = assert!(std::mem::size_of::<Properties>() == 32);
const _: () = assert!(std::mem::size_of::<ImportedDevice>() == 1132);
const _: () = assert!(std::mem::size_of::<PluginRequest>() == 1124);
const _: () = assert!(std::mem::offset_of!(PluginRequest, location) == 4);
const _: () = assert!(std::mem::offset_of!(PluginRequest, serial) == 1104);
const _: () = assert!(std::mem::offset_of!(PluginRequest, wsk_events) == 1120);
const _: () = assert!(std::mem::size_of::<StopRequest>() == 1108);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriverDevice {
    pub port: i32,
    pub busid: String,
    pub host: String,
    pub service: String,
    pub vendor: u16,
    pub product: u16,
    pub speed: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriverIdentity {
    pub instance_id: String,
    pub driver_version: String,
    pub driver_provider: String,
    pub driver_service: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriverProbe {
    pub identity: DriverIdentity,
    pub devices: Vec<DriverDevice>,
}

fn verify_pinned_identity(identity: &DriverIdentity) -> Result<()> {
    if identity.driver_version != PINNED_PNP_DRIVER_VERSION
        || identity.driver_provider != PINNED_DRIVER_PROVIDER
        || identity.driver_service != PINNED_DRIVER_SERVICE
    {
        bail!(
            "USB/IP 驱动实例 {} 的 PnP 版本为 {}，提供商为 {}，服务为 {}；发行包 {} 要求 PnP 版本 {}、提供商 {}、服务 {}。请运行本软件的固定版本安装程序",
            identity.instance_id,
            identity.driver_version,
            identity.driver_provider,
            identity.driver_service,
            PINNED_DRIVER_VERSION,
            PINNED_PNP_DRIVER_VERSION,
            PINNED_DRIVER_PROVIDER,
            PINNED_DRIVER_SERVICE
        );
    }
    Ok(())
}

pub fn plugin_request(busid: &str, proxy_port: u16) -> Result<PluginRequest> {
    crate::config::validate_busid(busid)?;
    if proxy_port == 0 {
        bail!("本地 USB 代理尚未启动");
    }
    // Zero the entire object, including padding required by the driver ABI.
    let mut request: PluginRequest = unsafe { std::mem::zeroed() };
    request.size = std::mem::size_of::<PluginRequest>() as u32;
    copy_string(&mut request.location.busid, busid)?;
    copy_string(&mut request.location.service, &proxy_port.to_string())?;
    copy_string(&mut request.location.host, "127.0.0.1")?;
    // Empty serial preserves the real USB identity; wsk_events false selects
    // the driver's zero-copy mode for webcam, storage and audio workloads.
    Ok(request)
}

fn copy_string(target: &mut [u8], value: &str) -> Result<()> {
    if value.len() >= target.len() || value.as_bytes().contains(&0) {
        bail!("驱动请求字段过长或含空字节");
    }
    target[..value.len()].copy_from_slice(value.as_bytes());
    Ok(())
}

pub fn parse_imported(bytes: &[u8]) -> Result<Vec<DriverDevice>> {
    const STRIDE: usize = std::mem::size_of::<ImportedDevice>();
    if bytes.len() < 4 || (bytes.len() - 4) % STRIDE != 0 {
        bail!("USB/IP 驱动返回的结构体尺寸与 0.9.8.1 不一致");
    }
    let mut devices = Vec::new();
    for data in bytes[4..].chunks_exact(STRIDE) {
        // DeviceIoControl may return data at an address not aligned to repr(C).
        let device = unsafe { std::ptr::read_unaligned(data.as_ptr().cast::<ImportedDevice>()) };
        if device.location.port < 1 {
            bail!("USB/IP 驱动返回了无效端口");
        }
        let string = |data: &[u8]| -> Result<String> {
            let length = data
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(data.len());
            Ok(std::str::from_utf8(&data[..length])?.to_string())
        };
        devices.push(DriverDevice {
            port: device.location.port,
            busid: string(&device.location.busid)?,
            host: string(&device.location.host)?,
            service: string(&device.location.service)?,
            vendor: device.properties.vendor,
            product: device.properties.product,
            speed: device.properties.speed,
        });
    }
    Ok(devices)
}

#[cfg(windows)]
mod native {
    use super::*;
    use anyhow::Context;
    use std::{ffi::c_void, ptr};
    type Handle = *mut c_void;

    #[repr(C)]
    struct Guid {
        a: u32,
        b: u16,
        c: u16,
        d: [u8; 8],
    }
    #[repr(C)]
    struct PropertyKey {
        guid: Guid,
        pid: u32,
    }
    const VHCI_GUID: Guid = Guid {
        a: 0xb4030c06,
        b: 0xdc5f,
        c: 0x4fcc,
        d: [0x87, 0xeb, 0xe5, 0x51, 0x5a, 0x09, 0x35, 0xc0],
    };
    const INSTANCE_KEY: PropertyKey = PropertyKey {
        guid: Guid {
            a: 0x78c34fc8,
            b: 0x104a,
            c: 0x4aca,
            d: [0x9e, 0xa4, 0x52, 0x4d, 0x52, 0x99, 0x6e, 0x57],
        },
        pid: 256,
    };
    const VERSION_KEY: PropertyKey = PropertyKey {
        guid: Guid {
            a: 0xa8b865dd,
            b: 0x2e3d,
            c: 0x4094,
            d: [0xad, 0x97, 0xe5, 0x93, 0xa7, 0x0c, 0x75, 0xd6],
        },
        pid: 3,
    };
    const PROVIDER_KEY: PropertyKey = PropertyKey {
        guid: Guid {
            a: 0xa8b865dd,
            b: 0x2e3d,
            c: 0x4094,
            d: [0xad, 0x97, 0xe5, 0x93, 0xa7, 0x0c, 0x75, 0xd6],
        },
        pid: 9,
    };
    const SERVICE_KEY: PropertyKey = PropertyKey {
        guid: Guid {
            a: 0xa45c254e,
            b: 0xdf1c,
            c: 0x4efd,
            d: [0x80, 0x20, 0x67, 0xd1, 0x46, 0xa8, 0x50, 0xe0],
        },
        pid: 6,
    };
    #[link(name = "cfgmgr32")]
    extern "system" {
        fn CM_Get_Device_Interface_List_SizeW(
            len: *mut u32,
            guid: *const Guid,
            device_id: *const u16,
            flags: u32,
        ) -> u32;
        fn CM_Get_Device_Interface_ListW(
            guid: *const Guid,
            device_id: *const u16,
            buffer: *mut u16,
            len: u32,
            flags: u32,
        ) -> u32;
        fn CM_Get_Device_Interface_PropertyW(
            interface: *const u16,
            key: *const PropertyKey,
            property_type: *mut u32,
            buffer: *mut u8,
            length: *mut u32,
            flags: u32,
        ) -> u32;
        fn CM_Locate_DevNodeW(device: *mut u32, instance: *mut u16, flags: u32) -> u32;
        fn CM_Get_DevNode_PropertyW(
            device: u32,
            key: *const PropertyKey,
            property_type: *mut u32,
            buffer: *mut u8,
            length: *mut u32,
            flags: u32,
        ) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn DeviceIoControl(
            handle: Handle,
            code: u32,
            input: *const c_void,
            input_len: u32,
            output: *mut c_void,
            output_len: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    struct Driver(Handle, DriverIdentity);
    impl Drop for Driver {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn open() -> Result<Driver> {
        // Retry only the documented CR_BUFFER_SMALL case caused by hotplug.
        for _ in 0..4 {
            let mut length = 0u32;
            let status = unsafe {
                CM_Get_Device_Interface_List_SizeW(&mut length, &VHCI_GUID, ptr::null(), 0)
            };
            if status != 0 {
                bail!("枚举 USB/IP 驱动失败，CM 错误 {status}");
            }
            if length < 2 {
                bail!("未发现 USB/IP 0.9.8.1 驱动，请运行客户端安装程序");
            }
            if length > 32768 {
                bail!("USB/IP 接口路径过长");
            }
            let mut buffer = vec![0u16; length as usize];
            let status = unsafe {
                CM_Get_Device_Interface_ListW(
                    &VHCI_GUID,
                    ptr::null(),
                    buffer.as_mut_ptr(),
                    length,
                    0,
                )
            };
            if status == 0x1a {
                continue;
            }
            if status != 0 {
                bail!("读取 USB/IP 驱动路径失败，CM 错误 {status}");
            }
            let paths: Vec<&[u16]> = buffer
                .split(|&c| c == 0)
                .filter(|v| !v.is_empty())
                .collect();
            if paths.len() != 1 {
                bail!("USB/IP 驱动接口数量为 {}，要求恰好一个", paths.len());
            }
            let name: Vec<u16> = paths[0].iter().copied().chain(Some(0)).collect();
            let identity = verify_driver_version(&name)?;
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    0xc0000000,
                    3,
                    ptr::null(),
                    3,
                    0x80,
                    ptr::null_mut(),
                )
            };
            if handle as isize == -1 {
                return Err(std::io::Error::last_os_error())
                    .context("打开 USB/IP 驱动失败，请检查服务权限");
            }
            return Ok(Driver(handle, identity));
        }
        bail!("USB/IP 驱动正在变化，请稍后重试")
    }

    fn verify_driver_version(interface: &[u16]) -> Result<DriverIdentity> {
        // Query the actual interface's PnP driver version rather than a copy of
        // usbip.exe or an assumed System32 path (the driver is package-isolated).
        let mut instance = [0u16; 512];
        let mut length = (instance.len() * 2) as u32;
        let mut property_type = 0u32;
        let status = unsafe {
            CM_Get_Device_Interface_PropertyW(
                interface.as_ptr(),
                &INSTANCE_KEY,
                &mut property_type,
                instance.as_mut_ptr().cast(),
                &mut length,
                0,
            )
        };
        if status != 0 || property_type != 0x12 || length as usize > instance.len() * 2 {
            bail!("无法核实 USB/IP 驱动实例，CM 错误 {status}");
        }
        let instance_id = property_string(&instance, length)?;
        let mut device = 0u32;
        let status = unsafe { CM_Locate_DevNodeW(&mut device, instance.as_mut_ptr(), 0) };
        if status != 0 {
            bail!("无法定位 USB/IP 驱动实例，CM 错误 {status}");
        }
        let identity = DriverIdentity {
            instance_id,
            driver_version: devnode_string(device, &VERSION_KEY, "版本")?,
            driver_provider: devnode_string(device, &PROVIDER_KEY, "提供商")?,
            driver_service: devnode_string(device, &SERVICE_KEY, "服务")?,
        };
        verify_pinned_identity(&identity)?;
        Ok(identity)
    }

    fn property_string(buffer: &[u16], byte_length: u32) -> Result<String> {
        let byte_length = byte_length as usize;
        if byte_length < 2 || byte_length % 2 != 0 || byte_length > buffer.len() * 2 {
            bail!("USB/IP 驱动字符串属性长度无效");
        }
        let units = &buffer[..byte_length / 2];
        if units.last() != Some(&0) || units[..units.len() - 1].contains(&0) {
            bail!("USB/IP 驱动字符串属性格式无效");
        }
        Ok(String::from_utf16(&units[..units.len() - 1])?)
    }

    fn devnode_string(device: u32, key: &PropertyKey, label: &str) -> Result<String> {
        let mut value = [0u16; 512];
        let mut length = (value.len() * 2) as u32;
        let mut property_type = 0u32;
        let status = unsafe {
            CM_Get_DevNode_PropertyW(
                device,
                key,
                &mut property_type,
                value.as_mut_ptr().cast(),
                &mut length,
                0,
            )
        };
        if status != 0 || property_type != 0x12 {
            bail!("无法核实 USB/IP 驱动{label}，CM 错误 {status}");
        }
        property_string(&value, length)
    }

    pub fn diagnostics() -> Result<Vec<DriverDevice>> {
        imported()
    }

    pub fn imported() -> Result<Vec<DriverDevice>> {
        let driver = open()?;
        imported_from(&driver)
    }

    pub fn probe() -> Result<DriverProbe> {
        let driver = open()?;
        let devices = imported_from(&driver)?;
        Ok(DriverProbe {
            identity: driver.1.clone(),
            devices,
        })
    }

    fn imported_from(driver: &Driver) -> Result<Vec<DriverDevice>> {
        for capacity in [4usize, 8, 16, 32, 64, 128, 256, 512, 1024] {
            let mut buffer = vec![0u32; (4 + capacity * 1132) / 4];
            let header_size = 1136u32;
            let mut returned = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    driver.0,
                    IOCTL_GET_IMPORTED,
                    ptr::addr_of!(header_size).cast(),
                    4,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 4) as u32,
                    &mut returned,
                    ptr::null_mut(),
                )
            };
            if ok != 0 {
                if returned as usize > buffer.len() * 4 {
                    bail!("驱动返回长度超出缓冲区");
                }
                let bytes = unsafe {
                    std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), returned as usize)
                };
                return parse_imported(bytes);
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(122) {
                return Err(error).context("读取 USB/IP 已连接设备失败");
            }
        }
        bail!("USB/IP 已连接设备列表超出 1024 个设备")
    }

    pub fn attach(busid: &str, port: u16) -> Result<i32> {
        let driver = open()?;
        let mut request = plugin_request(busid, port)?;
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                driver.0,
                IOCTL_PLUGIN_ONCE,
                ptr::addr_of!(request).cast(),
                request.size,
                ptr::addr_of_mut!(request).cast(),
                8,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error()).context("USB 连接失败");
        }
        if returned != 8 || request.location.port < 1 {
            bail!("USB/IP 驱动连接结果格式与固定版本不一致");
        }
        Ok(request.location.port)
    }

    pub fn detach(port: i32) -> Result<()> {
        // Never allow 'detach all' through the client, preserving other software.
        if port < 1 {
            bail!("释放端口必须为正数");
        }
        let driver = open()?;
        let request = PlugoutRequest { size: 8, port };
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                driver.0,
                IOCTL_PLUGOUT,
                ptr::addr_of!(request).cast(),
                8,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error()).context("释放 USB 设备失败");
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use native::{attach, detach, diagnostics, imported, probe};
#[cfg(not(windows))]
pub fn probe() -> Result<DriverProbe> {
    bail!("USB/IP 虚拟主机驱动只在 Windows 上提供")
}
#[cfg(not(windows))]
pub fn diagnostics() -> Result<Vec<DriverDevice>> {
    bail!("USB/IP 虚拟主机驱动只在 Windows 上提供")
}
#[cfg(not(windows))]
pub fn imported() -> Result<Vec<DriverDevice>> {
    diagnostics()
}
#[cfg(not(windows))]
pub fn attach(_: &str, _: u16) -> Result<i32> {
    bail!("连接设备需要 Windows USB/IP 驱动")
}
#[cfg(not(windows))]
pub fn detach(_: i32) -> Result<()> {
    bail!("释放设备需要 Windows USB/IP 驱动")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_version_is_distinct_from_the_signed_pnp_build() {
        let mut identity = DriverIdentity {
            instance_id: "ROOT\\USB\\0001".into(),
            driver_version: PINNED_PNP_DRIVER_VERSION.into(),
            driver_provider: PINNED_DRIVER_PROVIDER.into(),
            driver_service: PINNED_DRIVER_SERVICE.into(),
        };
        assert!(verify_pinned_identity(&identity).is_ok());
        identity.driver_version = PINNED_DRIVER_VERSION.into();
        assert!(verify_pinned_identity(&identity).is_err());
        identity.driver_version = "12.1.30.404".into();
        assert!(verify_pinned_identity(&identity).is_err());
        identity.driver_version = PINNED_PNP_DRIVER_VERSION.into();
        identity.driver_provider = "Other provider".into();
        assert!(verify_pinned_identity(&identity).is_err());
        identity.driver_provider = PINNED_DRIVER_PROVIDER.into();
        identity.driver_service = "Other service".into();
        assert!(verify_pinned_identity(&identity).is_err());
    }
    #[test]
    fn fixed_abi_and_real_identity() {
        let req = plugin_request("1-2.3", 43210).unwrap();
        assert_eq!(req.size, 1124);
        assert_eq!(&req.location.busid[..5], b"1-2.3");
        assert_eq!(&req.location.service[..5], b"43210");
        assert_eq!(&req.location.host[..9], b"127.0.0.1");
        assert_eq!(req.serial, [0; 16]);
        assert_eq!(req.wsk_events, 0);
        assert_eq!(std::mem::offset_of!(Location, host), 72);
        assert_eq!(std::mem::offset_of!(ImportedDevice, properties), 1100);
        assert_eq!(std::mem::offset_of!(Properties, iserial), 28);
    }
    #[test]
    fn rejects_incompatible_driver_responses() {
        assert!(parse_imported(&[]).is_err());
        assert!(parse_imported(&[0; 5]).is_err());
        assert!(parse_imported(&[0; 1136]).is_err());
        assert!(parse_imported(&[0; 4]).unwrap().is_empty());
    }
    #[test]
    fn parses_aligned_and_unaligned_imported_device() {
        let mut bytes = vec![0u8; 5];
        // Use a deliberately unaligned read buffer; explicit bytes avoid reading
        // uninitialized Rust repr(C) padding while keeping the driver ABI.
        let offset = bytes.len();
        bytes.resize(offset + 1132, 0);
        bytes[offset..offset + 4].copy_from_slice(&7i32.to_le_bytes());
        bytes[offset + 8..offset + 11].copy_from_slice(b"2-1");
        bytes[offset + 40..offset + 45].copy_from_slice(b"44444");
        bytes[offset + 72..offset + 81].copy_from_slice(b"127.0.0.1");
        bytes[offset + 1108..offset + 1110].copy_from_slice(&0x1a86u16.to_le_bytes());
        bytes[offset + 1110..offset + 1112].copy_from_slice(&0x5512u16.to_le_bytes());
        let result = parse_imported(&bytes[1..]).unwrap();
        assert_eq!(result[0].busid, "2-1");
        assert_eq!(result[0].vendor, 0x1a86);
    }
}
