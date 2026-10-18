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
| CI definition / CI 定义 | Passed static checks / 静态检查通过 | Actionlint and PowerShell parsing; hosted Actions execution remains pending |
| Windows installer payload / Windows 安装器内载荷 | Passed / 通过 | 907 files verified against package SHA256; no install into the existing production installation |
| Linux x86_64 and ARM64 binaries / 双架构 Linux 二进制 | Built / 已编译 | Static musl ELF built; ARM64 execution pending |
| Boot images / 启动镜像 | Pending / 待验证 | Recipes and workflows supplied; a successful recipe syntax check is not a boot test |
| Physical USB matrix / 真实 USB 矩阵 | Pending / 待验收 | x86_64, generic UEFI ARM64, Raspberry Pi 4 and Pi 5; dual Windows device ownership and recovery |

The existing native HWND tray restoration is retained. The RC UI was opened in an isolated demo for tutorial screenshots; the current RC installer has not replaced the user's working installation. Actual click, driver restart and end-to-end device checks remain explicit release gates.

保留现有原生 HWND 托盘恢复实现；教程截图来自 RC 的隔离演示。此次未用 RC 安装器覆盖用户正常使用的安装。真实点击、驱动重启和设备端到端检查仍是明确的发布门槛。