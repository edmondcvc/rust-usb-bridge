# 平台支持与验收

[English](../en/support.md)
首发为预发布版本。以下“目标”不是“全部已正式验证”。缺少 ARM 实机，因此 ARM 和树莓派正式支持须等待硬件验收。

| Target | OS baseline | Status |
|---|---|---|
| Linux x86_64 | Ubuntu Server 24.04 LTS | Build / automated checks; public-version physical acceptance pending |
| Generic UEFI ARM64 | Ubuntu Server 24.04 LTS ARM64 | Build target; physical boot and USB pending |
| Raspberry Pi 4 | Ubuntu Server 24.04 LTS raspi 64-bit | Image recipe; board acceptance pending |
| Raspberry Pi 5 | Ubuntu Server 24.04 LTS raspi 64-bit | Image recipe; board acceptance pending |
| Windows x64 | Windows 10 1903+ / Windows 11 | Installer target; public-version clean-machine acceptance pending |
| Debian / other systemd distributions | Matching USB/IP kernel required | Compatibility path; not formally qualified |
| VM | Explicit physical USB passthrough required | Boot/mock tests do not prove physical forwarding |
| Containers / WSL | Host kernel, sysfs, privileges required | Not a supported production installation route |

正式版门槛：每个平台验证全新安装、配对、真实 USB 数据传输、释放、断网和重启恢复；树莓派 4／5 分别记录。两台 Windows 分别使用不同设备，同设备互斥；断开一台不能影响另一台。安装器校验、模拟 USB、ARM runner 或虚拟机测试不能替代这些实物结果。

每条验收记录需要版本、源码提交、产物哈希、系统／内核、硬件型号、操作步骤和真实结果。只有全部目标硬件通过才发布 `v0.2.0`。任何 USB 类型（尤其音视频、加工设备）的兼容性都以对应设备实测为准。

## 已发布 RC 安装验证 — 2026-10-18

使用源码提交 `5e183c74e9b834655182a71c55a1173c79295222` 对应的公开 `v0.2.0-rc.1` 附件，在独立 x86_64 QEMU 虚拟机、Ubuntu 24.04、内核 `6.8.0-147-generic` 中测试。Linux 软件包 SHA256 为 `f647c757e26d5ebcaa1b1d731280f7dc953808fe5cbcab2b7a020440bc758684`，在线安装脚本 SHA256 为 `4b0cff51ee39b1ca701976d181afd88159ab82e4a8878b2ac7992158370d23e0`。

| 操作 | 实际结果 |
|---|---|
| 缺少 USB/IP 模块时在线安装 | 在创建应用安装目录之前拒绝安装 |
| 安装匹配内核模块后重试公开在线安装器 | 服务 active，版本 `0.2.0-rc.1` |
| 校验包内哈希并离线重复安装 | 服务 active；配置和 CA 哈希不变 |
| 使用完整备份回退 | 服务 active；配置和 CA 哈希不变 |
| 请求回退到不完整备份 | 在停止当前服务之前拒绝操作 |
| 卸载后离线重装 | 自有程序和服务已删除；复用保留配置和 CA；服务 active |

回退前后程序均为同一 RC，因此验证的是命令及身份保留机制，不能作为旧版本兼容矩阵。此虚拟机未配对 Windows 客户端、未连接真实 USB；结果不能证明真实配对保留、硬件 USB 转发或预装系统镜像启动已验收。
