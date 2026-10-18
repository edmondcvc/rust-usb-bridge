use anyhow::{Context, Result};
use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::Path, ptr};

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}
struct Descriptor(*mut c_void);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

#[link(name = "advapi32")]
extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn SetFileSecurityW(path: *const u16, information: u32, descriptor: *const c_void) -> i32;
    fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
    fn GetTokenInformation(
        token: *mut c_void,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn ConvertSidToStringSidW(sid: *const c_void, text: *mut *mut u16) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

struct Token(*mut c_void);
impl Drop for Token {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn current_user_sid() -> Result<String> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), 0x8, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error()).context("读取运行账户失败");
    }
    let token = Token(token);
    let mut length = 0u32;
    unsafe {
        GetTokenInformation(token.0, 1, ptr::null_mut(), 0, &mut length);
    }
    if length < std::mem::size_of::<*mut c_void>() as u32 || length > 65536 {
        anyhow::bail!("运行账户 SID 长度无效");
    }
    let mut data = vec![0u64; (length as usize + 7) / 8];
    if unsafe { GetTokenInformation(token.0, 1, data.as_mut_ptr().cast(), length, &mut length) }
        == 0
    {
        return Err(std::io::Error::last_os_error()).context("读取运行账户 SID 失败");
    }
    // TOKEN_USER starts with SID_AND_ATTRIBUTES; its first field is PSID.
    let sid = unsafe { ptr::read_unaligned(data.as_ptr().cast::<*mut c_void>()) };
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(std::io::Error::last_os_error()).context("转换运行账户 SID 失败");
    }
    let allocation = Descriptor(text.cast());
    let mut count = 0usize;
    while count < 256 && unsafe { *text.add(count) } != 0 {
        count += 1;
    }
    if count == 256 {
        anyhow::bail!("运行账户 SID 字符串过长");
    }
    let result = String::from_utf16(unsafe { std::slice::from_raw_parts(text, count) })?;
    drop(allocation);
    Ok(result)
}

fn descriptor(sddl: &str) -> Result<Descriptor> {
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error()).context("创建访问控制描述符失败");
    }
    Ok(Descriptor(descriptor))
}

/// Client data access only. FILE_APPEND_DATA (0x4) is also
/// FILE_CREATE_PIPE_INSTANCE and must never be granted to interactive clients.
pub const PIPE_CLIENT_ACCESS: u32 = 0x0012_019b;

pub fn create_pipe(first: bool) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    create_named_pipe(crate::PIPE_NAME, first, false)
}

pub(crate) fn create_named_pipe(
    name: &str,
    first: bool,
    diagnostic: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    // Interactive users can control USB devices; network users cannot enter
    // this pipe. Private certificates never appear in the IPC response.
    let sddl = if diagnostic {
        // An explicit diagnostic backend uses its own PID-qualified name.
        // Only that user and administrators can access it; clients also verify
        // the explicitly requested backend PID before sending any bytes.
        format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{})",
            current_user_sid()?
        )
    } else {
        format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x{PIPE_CLIENT_ACCESS:08x};;;IU)(A;;GA;;;LS)")
    };
    let sd = descriptor(&sddl)?;
    let mut attrs = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: sd.0,
        inherit: 0,
    };
    let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .in_buffer_size(65536)
        .out_buffer_size(65536);
    let pipe = unsafe {
        options.create_with_security_attributes_raw(name, ptr::addr_of_mut!(attrs).cast())
    }
    .context("创建客户端控制管道失败，可能已有后台服务运行")?;
    Ok(pipe)
}

pub(crate) fn open_pipe_client(
    name: &str,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use std::os::windows::{fs::OpenOptionsExt, io::IntoRawHandle};
    // SECURITY_IDENTIFICATION prevents a malicious server from impersonating
    // this caller. Tokio's default ClientOptions requests GENERIC_WRITE, which
    // includes the forbidden create-instance right, so use explicit access.
    let file = std::fs::OpenOptions::new()
        .access_mode(PIPE_CLIENT_ACCESS)
        .share_mode(0)
        .custom_flags(0x4000_0000)
        .security_qos_flags(0x0010_0000 | 0x0001_0000)
        .open(name)?;
    // Ownership transfers to Tokio, including closing the handle if reactor
    // registration fails.
    unsafe {
        tokio::net::windows::named_pipe::NamedPipeClient::from_raw_handle(file.into_raw_handle())
    }
}

#[cfg(test)]
mod pipe_tests {
    #[link(name = "shell32")]
    extern "system" {
        fn IsUserAnAdmin() -> i32;
    }

    #[tokio::test]
    async fn ordinary_user_can_connect_but_cannot_create_additional_instance() {
        // Exercise the actual current medium-integrity interactive token,
        // rather than comparing strings or mocking Windows AccessCheck.
        assert_eq!(
            unsafe { IsUserAnAdmin() },
            0,
            "run this ACL acceptance test without administrator elevation"
        );
        let name = format!(
            r"\\.\pipe\RustUsbBridge.Test.Acl.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let _server = super::create_named_pipe(&name, true, false).unwrap();
        assert_eq!(super::PIPE_CLIENT_ACCESS & 0x4, 0);
        let error = super::create_named_pipe(&name, false, false).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
        let _client = super::open_pipe_client(&name).unwrap();
    }
}

pub fn restrict_directory(path: &Path) -> Result<()> {
    // Production SCM runs as LocalService, so the added SID is already LS.
    // An explicit --direct diagnostic config remains accessible to its actual
    // owner, even when the account's Administrators SID is filtered by UAC.
    let sid = current_user_sid()?;
    let sd = descriptor(&format!(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;LS)(A;OICI;FA;;;{sid})"
    ))?;
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { SetFileSecurityW(name.as_ptr(), 0x80000004, sd.0) } == 0 {
        return Err(std::io::Error::last_os_error()).context("限制客户端私钥目录权限失败");
    }
    Ok(())
}

pub fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) } == 0 {
        return Err(std::io::Error::last_os_error()).context("写入客户端配置失败");
    }
    Ok(())
}
