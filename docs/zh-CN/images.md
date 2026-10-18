# 纯净预装系统镜像

[English](../en/images.md) · [支持状态](support.md)

镜像从经过校验的 Ubuntu 官方基础镜像构建，不从已部署电脑克隆。内容包含本项目服务、OpenSSH 和必要系统依赖，不包含 VirtualHere、他人 VPN 登录身份或原工作站应用。

## 选择启动平台

| 平台参数 | 基础镜像 | 启动目标 |
|---|---|---|
| `x86_64` | Ubuntu 24.04 amd64 云磁盘 | 通用 x86_64，实际机器的固件启动模式须验证 |
| `uefi-arm64` | Ubuntu 24.04 arm64 云磁盘 | 通用 ARM64 UEFI，板卡固件和驱动须支持该 Ubuntu 系统 |
| `raspi4` | Ubuntu 24.04 ARM64 树莓派预装磁盘 | 树莓派 4 |
| `raspi5` | Ubuntu 24.04 ARM64 树莓派预装磁盘 | 树莓派 5 |

即使上游镜像同时兼容 Pi 4／5，本项目也分别命名和验收。通用 UEFI ARM 镜像不能直接当成树莓派镜像使用。基础镜像目录：[Ubuntu 云镜像](https://cloud-images.ubuntu.com/noble/current/)、[Ubuntu 树莓派镜像](https://cdimage.ubuntu.com/releases/24.04/release/)。

## 在 Linux 构建

使用与镜像 **相同 CPU 架构** 的 Linux 构建机，磁盘空间须能存放未压缩输入、输出和 libguestfs 环境；构建机可以是虚拟机。安装依赖，下载官方镜像和对应官方 `SHA256SUMS`，填写准确哈希，并使用同版本、对应架构的服务 ELF。

```bash
sudo apt install libguestfs-tools qemu-utils xz-utils python3
sudo bash scripts/images/build-image.sh \
  --platform x86_64 \
  --base /work/noble-server-cloudimg-amd64.img \
  --base-url https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img \
  --base-sha256 YOUR_OFFICIAL_64_HEX_DIGIT_HASH \
  --binary /work/usb-bridge-server \
  --output /work/new-image-output
```

通用 ARM 使用 arm64 基础盘和 `--platform uefi-arm64`。树莓派使用官方 `...arm64+raspi.img.xz` 和 `--platform raspi4`／`raspi5`。不允许用私有机器快照代替。程序拒绝架构不符、哈希错误、已存在的输出目录和缺少 USB/IP 内核模块的镜像。`image-provenance.json` 记录来源、基础镜像哈希、最终镜像哈希及 **待验证** 的硬件验收状态。GitHub 的 **Build clean Ubuntu image** 手动工作流在对应架构 runner 上使用同一流程，要求输入同一提交成功的 CI 编号。

## 下载、校验与写盘

只使用 Release 中实际提供的镜像附件；有构建脚本不等于镜像已经验收。RC 镜像在更新验收矩阵前均标为硬件待验证。若按 1 GiB 分卷发布，下载所有分卷及 `.volumes.json`，运行随 Release 提供的 `release-tools.py`：

```powershell
py release-tools.py merge --file .\IMAGE.img.gz.volumes.json --output .\merged
```

合并程序验证各分卷及完整压缩文件，拒绝覆盖已有输出。用支持 gzip 的工具解压 `.img.gz`，再按正规写盘工具的官方说明写入 `.img`。按型号与容量核对目标盘，因为写入会覆盖全部内容。镜像写到哪块盘就从哪块盘运行，不要覆盖当前正在运行的 Linux 系统盘。

## 第一次启动

连接显示器、键盘和有 DHCP 的有线网络。首次启动生成独立机器身份、主机名、SSH 主机密钥和 TLS 身份，不预选 USB 共享。本机屏幕要求为 `operator`（或构建时指定的 `--username`）设置新密码，**没有共用默认密码**。在本机完成密码设置后，才能使用 SSH 密码配对。root 的 SSH 密码登录关闭；镜像不是带公开默认密码的无人值守系统。

在屏幕或通过 `ip -brief address` 查看地址。仅对普通 ext4 根分区尝试扩容并记录结果，特别是非标准固件或存储布局，应检查 `df -h /`。镜像不包含 Wi-Fi 密码。随后按 [Windows 安装](installation.md)和[配对使用](usage.md)继续。

```bash
systemctl status rust-usb-firstboot rust-usb-account rust-usb-bridge ssh --no-pager
sudo journalctl -u rust-usb-firstboot -b --no-pager
df -h /
```

QEMU 启动通过只证明对应虚拟配置。每种实体 ARM 电脑和 Pi 型号仍需完成启动、网络、内核模块和真实 USB 验证后，才能宣布正式支持。
