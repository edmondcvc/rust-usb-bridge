# Linux 与 Windows 安装

[English](../en/installation.md) · [准备 Linux](linux-setup.md)

## Linux 在线安装

指定明确版本。脚本自动选择 x86_64／aarch64，校验下载压缩包，再调用离线包也使用的安装程序。先阅读下载脚本再执行。

```bash
VERSION=v0.2.0-rc.1
curl -fL "https://github.com/edmondcvc/rust-usb-bridge/releases/download/$VERSION/install.sh" -o install.sh
less install.sh
sudo bash install.sh --version "$VERSION" --listen 0.0.0.0:3241 --server-name usb-bridge
```

默认在 TCP 3241 接收经过身份验证的 TLS 连接。按你自己的防火墙策略仅允许局域网／VPN 访问，不需要开放原始 USB/IP 3240 端口。SSH 默认 TCP 22。全新安装不会预先共享任何 USB。

## 离线安装

在另一台电脑下载架构对应的压缩包和 Release 的 `SHA256SUMS`，转移到 Linux。离线软件包包含服务程序及脚本，**不包含所有发行版和内核依赖**；请在可联网时按[系统准备](linux-setup.md)完成依赖安装。

```bash
# Use aarch64 instead of x86_64 on 64-bit ARM.
sha256sum -c SHA256SUMS --ignore-missing
mkdir -p linux-package
tar -xzf RustUSBBridge-Linux-0.2.0-rc.1-x86_64.tar.gz -C linux-package
cd linux-package
sha256sum -c SHA256SUMS
sudo bash scripts/linux/check-host.sh
sudo bash scripts/linux/install-linux.sh --listen 0.0.0.0:3241 --server-name usb-bridge
```

`--binary 文件` 指定本地 ELF；`--no-start` 安装并启用服务但暂不启动。安装程序检查架构、依赖和已有安装身份，保留现有证书与配对。

## 检查 Linux

```bash
systemctl is-active rust-usb-bridge
sudo systemctl status rust-usb-bridge --no-pager
sudo journalctl -u rust-usb-bridge -b --no-pager -n 80
sudo /opt/rust-usb-bridge/usb-bridge-server --version
```

成功判据：服务为 `active`，程序版本对应本次发布，日志没有启动错误。失败时见[故障处理](troubleshooting.md)。私有证书和配对状态在 `/var/lib/rust-usb-bridge`；公开 CA 和指纹在 `/opt/rust-usb-bridge/public`。

## Windows x64 安装

1. 从同一个 Release 下载 `RustUSBBridge-Setup-0.2.0-rc.1-x64.exe` 和 `SHA256SUMS`。在 PowerShell 运行 `Get-FileHash .\RustUSBBridge-Setup-0.2.0-rc.1-x64.exe -Algorithm SHA256`，比较完整校验值。
2. 升级前结束设备操作，并释放本机 USB 会话。运行安装器，在 Windows 管理员提示中允许安装。应用未购买代码签名，可能显示未知发布者，请核对来源和哈希；随包的上游 USB 驱动保留原数字签名。
3. 驱动要求重启时，保存工作后重启。安装器不会强制重启。设备原有驱动及应用仍按设备要求安装。
4. 在 Windows 设置 → 可选功能中确认已安装 **OpenSSH 客户端**。PowerShell 检查：`Get-Command ssh.exe, ssh-keyscan.exe, ssh-keygen.exe`。完全离线的 Windows 应提前准备与版本匹配的微软可选功能安装介质。
5. 从桌面打开 **Rust USB 客户端**。新安装由你填写 Linux 地址，不绑定示例机器。继续阅读[配对与使用](usage.md)。

程序在 `C:\Program Files\Rust USB Bridge`，后台服务和私有状态在 `C:\ProgramData\RustUsbBridge`。界面连接不到后台时运行 `Get-Service UsbBridgeRust` 检查。此版本软件界面仍为中文；本次提供的是中英双语文档。
