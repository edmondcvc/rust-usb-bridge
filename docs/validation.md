# v0.2.0-rc.1 validation / 验证记录

Recorded on 2026-10-18. These checks qualify an RC; they do not establish physical platform support.

记录日期：2026-10-18。以下检查用于预发布版，不代表所有平台已完成实机验收。

| Check / 检查 | Result / 结果 | Scope / 范围 |
|---|---|---|
| Windows Rust tests / Windows Rust 测试 | 51 passed / 通过 | Client, common library, setup; includes native tray regression test |
| Linux server tests / Linux 服务端测试 | 23 passed / 通过 | x86_64 Ubuntu 24.04 QEMU VM; includes real TCP/TLS pairing with simulated USB state |
| Linux protocol library / Linux 协议库 | 4 passed / 通过 | x86_64 VM; compatibility and input validation |
| Linux installer fixture / Linux 安装脚本隔离测试 | 6 passed / 通过 | Fresh/repeated install, pairing retention, wrong architecture, prerequisites, invalid listener, busy device and missing backup guards; kernel/systemd tools simulated |
| Release tools / 发布工具 | 4 passed / 通过 | Tar executable modes, architecture rejection, corrupt volumes, unsafe volume paths |
| Bilingual docs / 双语文档 | Passed / 通过 | 24 Markdown files; eight corresponding pages per language; local links and HTML rendering |
| Source audit / 源码审计 | Passed / 通过 | Tracked files checked for private state, machine literals and forbidden artifacts |
| Hosted CI / GitHub 持续构建 | Passed / 通过 | Source/docs, native Linux x86_64 and ARM64 tests/builds, Windows tests/build/setup payload verification |
| Windows installer payload / Windows 安装器内载荷 | Passed / 通过 | 909 files verified against package SHA256; no install into the existing production installation |
| Linux x86_64 and ARM64 binaries / 双架构 Linux 二进制 | Built and tested / 已编译与测试 | Static musl ELF; native architecture CI executes server/common tests |
| Published online/offline installation / 公开在线与离线安装 | Passed in VM / 虚拟机通过 | Ubuntu 24.04 kernel 6.8.0-147-generic; repeat install, complete/incomplete backup guards, uninstall/reinstall and retained configuration/CA |
| Image identity initialization / 镜像身份初始化 | Passed / 通过 | Actual Linux executable; two independent CAs and repeat-start identity retention; host/USB operations stubbed |
| Boot images / 启动镜像 | x86_64 virtual boot passed / x86_64 虚拟启动通过 | Four platform builds succeeded; two independent x86_64 boots passed; physical boot and USB remain pending |
| Physical USB matrix / 真实 USB 矩阵 | Pending / 待验收 | x86_64, generic UEFI ARM64, Raspberry Pi 4 and Pi 5; dual Windows device ownership and recovery |

Published RC source `5e183c74e9b834655182a71c55a1173c79295222` passed [hosted CI](https://github.com/edmondcvc/rust-usb-bridge/actions/runs/39121730091). Subsequent image-build fixes at `26fdb64c2fc45919aafb322fd781fa1b94ba497e` passed [all four CI jobs](https://github.com/edmondcvc/rust-usb-bridge/actions/runs/39130619379). The published RC tag and its existing release assets remain pinned to the original release source.

公开 RC 源码 `5e183c74e9b834655182a71c55a1173c79295222` 已通过上述 RC 持续构建；后续镜像修正提交 `26fdb64c2fc45919aafb322fd781fa1b94ba497e` 的四个 CI 任务也全部通过。已发布 RC 的标签和已有附件仍固定在原始发布源码。实际安装维护验证步骤与边界见 [中文记录](zh-CN/support.md)／[English record](en/support.md)。

The Raspberry Pi 4 artifact from [image run 39128753131](https://github.com/edmondcvc/rust-usb-bridge/actions/runs/39128753131) was downloaded and checked: ZIP SHA256 `e4a00c65fc93300c8980e2053c506105f9c01dd7dc5fa90da7320aa4e1efe54a`; both volumes and merged compressed image passed checksums (`d39936cbd5eb99c8e906e8f0b72af7d827caf794e0c2286e5e5c74d6dfb36246`). Read-only filesystem inspection confirmed an empty machine ID, absent SSH host/login keys, empty private service state, locked administrator password, matching tested startup scripts and USB/IP modules in kernel `6.8.0-1064-raspi`. This is not a board boot test.

树莓派 4 构建附件已实际下载，ZIP 哈希、两份分卷及合并后的镜像哈希均通过校验（哈希见上段）。只读文件系统检查确认机器 ID 为空、没有 SSH 主机与登录密钥、私有服务状态为空、管理员密码锁定、启动脚本与已测试源码一致，且内核 `6.8.0-1064-raspi` 包含 USB/IP 模块。这些检查不能替代板卡启动和真实 USB 验收。

The existing native HWND tray restoration is retained. The RC UI was opened in an isolated demo for tutorial screenshots; the current RC installer has not replaced the user's working installation. Actual click, driver restart and end-to-end device checks remain explicit release gates.

保留现有原生 HWND 托盘恢复实现；教程截图来自 RC 的隔离演示。此次未用 RC 安装器覆盖用户正常使用的安装。真实点击、驱动重启和设备端到端检查仍是明确的发布门槛。

The x86_64 image from [run 39131245276](https://github.com/edmondcvc/rust-usb-bridge/actions/runs/39131245276) passed archive/volume checks and read-only filesystem inspection. Two fresh QEMU TCG boots on separate 8-GiB disks reached Ubuntu 24.04.5 login with kernel `6.8.0-147-generic`. Both exposed TLS 1.3 with certificates matching the generated disk identity, expanded the root filesystem to 7,515,123,712 bytes, and generated distinct machine IDs, SSH host keys and service CAs. Both test VMs shut down. No physical USB was attached; first-password interaction and Windows enrollment with the image were not tested. The clean unbooted template, exact image source and public validation metadata are provided separately from the original RC application assets.

x86_64 镜像的 ZIP、分卷及只读文件系统检查通过；两个独立的 8 GiB QEMU TCG 磁盘均启动到 Ubuntu 24.04.5 登录界面（内核见上段）。TLS 1.3 可访问且证书与各自磁盘身份匹配，根文件系统扩容到 7,515,123,712 字节，机器 ID、SSH 主机密钥与服务 CA 各自独立。两台测试虚拟机均已关闭。未接入真实 USB，未验证首次密码交互输入及此镜像的 Windows 配对。公开的是从未启动的纯净模板、镜像对应源码与公开验证元数据；原 RC 应用附件保持不变。
