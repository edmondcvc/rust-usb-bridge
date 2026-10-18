# 纯净预装系统镜像

[English](../en/images.md) · [支持状态](support.md)

镜像从经过校验的 Ubuntu 官方基础镜像构建，不从已部署电脑克隆。内容包含本项目服务、OpenSSH 和必要系统依赖，不包含 VirtualHere、他人 VPN 登录身份或原工作站应用。

## RC 镜像下载与验证状态

[v0.2.0-rc.1 发布页](https://github.com/edmondcvc/rust-usb-bridge/releases/tag/v0.2.0-rc.1)提供 x86_64 预览镜像。将对应的 `.img.gz.part001`、`.img.gz.volumes.json`、来源和验证 JSON、`SHA256SUMS-images-x86_64-0.2.0-rc.1` 以及 `release-tools.py` 下载到同一目录。本镜像只有一个分卷，请保留 `.part001` 文件名。使用以下命令合并并校验：

```powershell
py release-tools.py merge --file .\RustUSBBridge-Ubuntu24.04-0.2.0-rc.1-x86_64.img.gz.volumes.json --output .\merged
```

镜像使用源码 `26fdb64c2fc45919aafb322fd781fa1b94ba497e` 构建，对应完整源码为 `RustUSBBridge-ImageSource-0.2.0-rc.1-26fdb64.zip`。原 RC 应用安装包和标签仍使用源码 `5e183c74e9b834655182a71c55a1173c79295222`。

两次独立 x86_64 QEMU 启动已通过：服务与 TLS 可访问、机器 ID／SSH 主机密钥／服务 CA 各自独立、初始配对状态为空、根分区扩展到 8 GiB 虚拟磁盘。物理机器启动、真实 USB 传输、首次密码的交互输入及使用此镜像与 Windows 配对仍待验证。通用 UEFI ARM64、树莓派 4、树莓派 5 镜像均已构建成功，进一步启动检查前暂不附加到 Release。详见[验证记录](../validation.md)。

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
sudo apt install libguestfs-tools qemu-utils xz-utils python3 dhcpcd-base dpkg-dev
printf 'dhcpcd-base\n' | sudo tee "/usr/lib/$(dpkg-architecture -qDEB_HOST_MULTIARCH)/guestfs/supermin.d/zz-rust-usb-dhcp"
bash scripts/images/build-image.sh \
  --platform x86_64 \
  --base /work/noble-server-cloudimg-amd64.img \
  --base-url https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img \
  --base-sha256 YOUR_OFFICIAL_64_HEX_DIGIT_HASH \
  --binary /work/usb-bridge-server \
  --output /work/new-image-output
```

通用 ARM 使用 arm64 基础盘和 `--platform uefi-arm64`。树莓派使用官方 `...arm64+raspi.img.xz` 和 `--platform raspi4`／`raspi5`。不允许用私有机器快照代替。程序拒绝架构不符、哈希错误、已存在的输出目录和缺少 USB/IP 内核模块的镜像。`image-provenance.json` 记录来源、基础镜像哈希、最终镜像哈希及 **待验证** 的硬件验收状态。GitHub 的 **Build clean Ubuntu image** 手动工作流在对应架构 runner 上使用同一流程，要求输入同一提交成功的 CI 编号。

镜像构建程序使用普通账户运行，输入与输出目录应由该账户读写。只有安装依赖需要 sudo；libguestfs 可以修改镜像内的文件，不需要构建机 root 权限。

Ubuntu 的 `/boot/vmlinuz-*` 可能只有 root 可读，导致 supermin 启动失败。将公开内核复制到可写的工作目录，并指定对应模块（参见 [libguestfs 官方 FAQ](https://www.libguestfs.org/guestfs-faq.1.html)）：

```bash
mkdir -p "$PWD/guestfs-work"
sudo install -m 0644 "/boot/vmlinuz-$(uname -r)" "$PWD/guestfs-work/kernel"
export SUPERMIN_KERNEL="$PWD/guestfs-work/kernel"
export SUPERMIN_KERNEL_VERSION="$(uname -r)"
export SUPERMIN_MODULES="/lib/modules/$(uname -r)"
```

换一个新输出目录后重试。其他启动失败可设置 `LIBGUESTFS_DEBUG=1 LIBGUESTFS_TRACE=1`，记录具体原因。

Ubuntu 24.04 的 `passt` 软件包可能以 `Permission denied` 拒绝创建 libguestfs PID 文件。一次性 CI 构建机移除此可选辅助程序，使用 libguestfs 的 QEMU 用户态网络后备方式。本地构建请在独立虚拟机中采用此办法，或使用已修复该辅助程序的系统；共享主机移除软件包前应检查依赖变化。工作流仅向当前构建用户授予存在的 `/dev/kvm` 访问权限；硬件加速不是必需条件，但比 TCG 模拟快得多。

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

