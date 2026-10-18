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
