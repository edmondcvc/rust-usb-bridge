#![cfg_attr(windows, windows_subsystem = "windows")]
use anyhow::{bail, Context, Result};
use std::{
    fs,
    io::{self, Cursor},
    path::{Component, Path, PathBuf},
    process::Command,
};
include!(concat!(env!("OUT_DIR"), "/payload.rs"));

fn check_payload(data: &[u8]) -> Result<serde_json::Value> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(Cursor::new(data))?;
    if archive.len() > 10000 {
        bail!("安装包文件数量异常");
    }
    let mut checksums = std::collections::BTreeMap::new();
    let mut total = 0u64;
    let mut manifest = None;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let relative = file.enclosed_name().context("安装包包含非法路径")?;
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            bail!("安装包路径或类型无效");
        }
        total = total.checked_add(file.size()).context("安装包大小溢出")?;
        if total > 512 * 1024 * 1024 {
            bail!("安装包解压大小超限");
        }
        if file.is_dir() {
            continue;
        }
        let name = relative.to_string_lossy().replace('\\', "/");
        if checksums.contains_key(&name) {
            bail!("安装包包含重复文件");
        }
        if name == "SHA256SUMS" {
            if manifest.is_some() {
                bail!("安装包包含重复校验清单");
            }
            let mut text = String::new();
            file.read_to_string(&mut text)?;
            manifest = Some(text);
        } else {
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            checksums.insert(name, format!("{:x}", hash.finalize()));
        }
    }
    for required in [
        "bridge-client.exe",
        "scripts/install-windows.ps1",
        "vendor/USBip-0.9.8.1-x64.exe",
        "LICENSE",
        "THIRD_PARTY_NOTICES.md",
    ] {
        if !checksums.contains_key(required) {
            bail!("安装包缺少 {required}");
        }
    }
    let mut verified = std::collections::BTreeSet::new();
    for line in manifest
        .context("安装包缺少校验清单")?
        .lines()
        .filter(|s| !s.is_empty())
    {
        let (hash, name) = line.split_once("  ").context("校验清单格式无效")?;
        if checksums.get(name).map(String::as_str) != Some(hash)
            || !verified.insert(name.to_owned())
        {
            bail!("安装包文件校验失败：{name}");
        }
    }
    if verified.len() != checksums.len() {
        bail!("安装包包含未列入校验清单的文件");
    }
    Ok(
        serde_json::json!({"ok":true,"version":env!("CARGO_PKG_VERSION"),"files":checksums.len(),"payload_bytes":data.len(),"uncompressed_bytes":total,"all_sha256_verified":true}),
    )
}

fn extract(data: &[u8], destination: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data))?;
    if archive.len() > 10000 {
        bail!("安装包文件数量异常");
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let relative = file.enclosed_name().context("安装包包含非法路径")?;
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!("非法安装包路径");
        }
        if file
            .unix_mode()
            .map(|m| m & 0o170000 == 0o120000)
            .unwrap_or(false)
        {
            bail!("安装包不允许符号链接");
        }
        total = total.checked_add(file.size()).context("安装包大小溢出")?;
        if total > 512 * 1024 * 1024 {
            bail!("安装包解压大小超限");
        }
        let output = destination.join(relative);
        if file.is_dir() {
            fs::create_dir_all(&output)?;
        } else {
            fs::create_dir_all(output.parent().unwrap())?;
            let mut target = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?;
            io::copy(&mut file, &mut target)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
fn message(text: &str, confirm: bool) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let flags = if confirm {
        MB_YESNO | MB_ICONQUESTION
    } else {
        MB_OK | MB_ICONINFORMATION
    };
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(text).as_ptr(),
            wide("USB 网络共享 — Rust 免费版").as_ptr(),
            flags,
        ) == IDYES
    }
}
#[cfg(not(windows))]
fn message(text: &str, _: bool) -> bool {
    eprintln!("{text}");
    true
}

#[cfg(windows)]
fn elevated() -> Result<bool> {
    use windows_sys::Win32::{Foundation::*, Security::*, System::Threading::*};
    let mut token: HANDLE = std::ptr::null_mut();
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut returned = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        if ok == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(elevation.TokenIsElevated != 0)
    }
}

#[cfg(windows)]
fn windows_directory() -> Result<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    let n = unsafe {
        windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if n == 0 || n >= buffer.len() {
        bail!("无法定位 Windows 目录");
    }
    let directory = PathBuf::from(String::from_utf16(&buffer[..n])?);
    if !directory
        .to_string_lossy()
        .to_ascii_lowercase()
        .starts_with("c:\\")
    {
        bail!("此安装包仅使用 C:；Windows 目录不在 C:，安装已取消。");
    }
    Ok(directory)
}

#[cfg(windows)]
fn staging_directory() -> Result<PathBuf> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG},
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CreateDirectoryW, GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT,
            INVALID_FILE_ATTRIBUTES,
        },
    };
    let parent = windows_directory()?.join("Temp");
    let attributes = unsafe { GetFileAttributesW(wide(&parent.to_string_lossy()).as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        bail!("Windows 临时目录无效或已被重定向，安装已取消。");
    }
    let mut random = [0u8; 16];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            random.as_mut_ptr(),
            random.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        bail!("无法生成安全安装目录，Windows 错误 {status}");
    }
    let nonce: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let directory = parent.join(format!("RustUsbBridge-{nonce}"));
    // Create the directory with its final protected DACL in one operation.
    // Elevated code is never extracted to a user-writable staging directory.
    let mut descriptor = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)").as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let ok = unsafe { CreateDirectoryW(wide(&directory.to_string_lossy()).as_ptr(), &attributes) };
    let error = io::Error::last_os_error();
    unsafe {
        LocalFree(descriptor);
    }
    if ok == 0 {
        return Err(error).context("创建管理员安装目录失败");
    }
    Ok(directory)
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--check-payload") {
        let report = check_payload(PAYLOAD)?;
        let output = args
            .windows(2)
            .find(|a| a[0] == "--output")
            .map(|a| PathBuf::from(&a[1]))
            .context("校验安装包需要 --output C:\\路径\\check.json")?;
        if !output.is_absolute() { bail!("校验报告需要绝对路径"); }
        fs::write(output, serde_json::to_vec_pretty(&report)?)?;
        return Ok(());
    }
    if args.iter().any(|a| a == "--version") {
        message(concat!("USB 网络共享 Rust 版 ", env!("CARGO_PKG_VERSION")), false);
        return Ok(());
    }
    if PAYLOAD.is_empty() {
        bail!("当前文件是开发构建，尚未包含安装内容。请使用 dist 中的正式安装包。");
    }
    check_payload(PAYLOAD)?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        if !elevated()? {
            if !message("安装 USB 网络共享 Rust 免费版？\n\n将安装客户端和官方已签名 USB/IP 驱动。需要管理员授权，USB 设备可能短暂重新连接，完成后可能需要重启。请先结束加工、文件传输、视频或音频任务。\n\n已有软件和本软件的配对配置会保留。", true) { return Ok(()); }
            let exe = std::env::current_exe()?;
            let result = unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    wide("runas").as_ptr(),
                    wide(&exe.to_string_lossy()).as_ptr(),
                    wide("--install").as_ptr(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                )
            };
            if result as isize <= 32 {
                bail!("管理员授权未完成，未安装。");
            }
            return Ok(());
        }
    }
    #[cfg(windows)]
    let staging = staging_directory()?;
    #[cfg(not(windows))]
    let staging = {
        bail!("安装程序只支持 Windows x64");
        #[allow(unreachable_code)]
        PathBuf::new()
    };
    extract(PAYLOAD, &staging)?;
    let script = staging.join("scripts/setup-run.ps1");
    if !script.is_file() {
        bail!("安装包缺少安装脚本");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = Command::new(
            windows_directory()?.join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        )
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .arg("-PackageDir")
        .arg(&staging)
        .creation_flags(0x08000000)
        .output()?;
        let bootstrap_log = staging.join("installer-bootstrap.log");
        let mut log = output.stdout;
        log.extend_from_slice(&output.stderr);
        std::fs::write(&bootstrap_log, &log)?;
        let status = output.status;
        match status.code() {
            Some(0) => { message("安装完成。请打开桌面的“Rust USB 客户端”连接设备。日志位于 C:\\ProgramData\\RustUsbBridge\\logs。", false); }
            Some(3010) => { message("安装完成，需要重启 Windows 后再连接 USB。请保存工作并按正常方式重启。日志位于 C:\\ProgramData\\RustUsbBridge\\logs。", false); }
            _ => {
                let details = fs::read(staging.join("install-result.json")).ok()
                    .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok());
                bail!("{}\n\n详细记录：{}", installation_failure(status.code(), details.as_ref()), bootstrap_log.display());
            },
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        if std::env::args().any(|arg| arg == "--check-payload") {
            eprintln!("{error:#}");
        } else {
            message(&format!("{error:#}"), false);
        }
        std::process::exit(1);
    }
}

fn installation_failure(code: Option<i32>, details: Option<&serde_json::Value>) -> String {
    let stage = details.and_then(|v| v["stage"].as_str()).unwrap_or("执行安装程序");
    let reason = details.and_then(|v| v["error"].as_str()).unwrap_or("安装脚本未能正常启动，详细记录包含 Windows 返回的错误。");
    let reason: String = reason.chars().take(1400).collect();
    format!("安装未完成：{stage}\n\n{reason}\n\n错误代码：{}", code.map(|v| v.to_string()).unwrap_or_else(|| "进程被中断".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_errors_show_reason_and_plain_exit_code() {
        let detail = serde_json::json!({"stage":"注册后台服务", "error":"访问被拒绝"});
        let message = installation_failure(Some(1), Some(&detail));
        assert!(message.contains("注册后台服务"));
        assert!(message.contains("访问被拒绝"));
        assert!(!message.contains("Some(1)"));
        assert!(installation_failure(None, None).contains("进程被中断"));
    }
    #[test]
    fn invalid_zip_is_rejected() {
        assert!(extract(b"invalid", Path::new("unused")).is_err());
    }

    fn package_fixture(tamper: bool, omit: bool) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        use std::io::Write;
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let mut manifest = String::new();
        for name in [
            "bridge-client.exe",
            "scripts/install-windows.ps1",
            "vendor/USBip-0.9.8.1-x64.exe",
            "LICENSE",
            "THIRD_PARTY_NOTICES.md",
        ] {
            if omit && name == "LICENSE" {
                continue;
            }
            let content = name.as_bytes();
            archive.start_file(name, options).unwrap();
            archive.write_all(content).unwrap();
            let hash = if tamper && name == "bridge-client.exe" {
                format!("{:x}", Sha256::digest(b"changed"))
            } else {
                format!("{:x}", Sha256::digest(content))
            };
            manifest.push_str(&format!("{hash}  {name}\n"));
        }
        archive.start_file("SHA256SUMS", options).unwrap();
        archive.write_all(manifest.as_bytes()).unwrap();
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn embedded_package_checks_all_files_and_required_content() {
        assert!(check_payload(&package_fixture(false, false)).is_ok());
        assert!(check_payload(&package_fixture(true, false)).is_err());
        assert!(check_payload(&package_fixture(false, true)).is_err());
    }
}

