# Rust USB Bridge · USB 连接中心

[English](README.md)

将 Linux 主机上的实体 USB 设备通过网络连接到 Windows。每台物理设备同时由一台电脑使用，多台电脑可以分别使用不同设备。

[下载安装](docs/zh-CN/installation.md) | [从零安装 Linux](docs/zh-CN/linux-setup.md) | [配对与使用教程](docs/zh-CN/usage.md) | [源码构建](docs/zh-CN/building.md)

## 发布状态

首个公开版本为 **0.2.0-rc.1 预发布版**。Linux 提供 x86_64 和 aarch64 两种独立程序；通用 UEFI ARM64 与树莓派 4／5 使用不同启动镜像。Windows 客户端为 x64。正式支持状态以[平台与验收矩阵](docs/zh-CN/support.md)为准。

项目使用 Linux `usbip-host`、双向 TLS 以及保持原样的官方签名 usbip-win2 0.9.8.1 驱动（PnP 版本 12.1.30.403）。设备原有应用和功能驱动仍需安装。本项目安装器未购买商业代码签名。

## 最短使用路线

1. 准备 Ubuntu Server 24.04 LTS、OpenSSH、Python 3 和具有 sudo 权限的用户。
2. 下载指定版本的 `install.sh`，阅读后用 sudo 执行。离线压缩包调用同一个安装程序。
3. 安装 Windows 客户端，驱动提示重启时先重启。
4. 填写 Linux 地址、用户名和密码，核对 SSH 主机指纹，再连接空闲 USB。
5. 关闭窗口会进入托盘；单击蓝色 USB 托盘图标即可恢复窗口。

存储设备须先安全弹出再释放。网络断开不是加工设备的急停。“内核支持 USB/IP”不等于所有实体设备都已验证兼容。

## 更多内容

[系统镜像](docs/zh-CN/images.md) · [离线安装](docs/zh-CN/installation.md#离线安装) · [异地连接](docs/zh-CN/networking.md) · [故障处理](docs/zh-CN/troubleshooting.md) · [参与开发](CONTRIBUTING.md) · [安全报告](SECURITY.md) · [更新记录](CHANGELOG.md)

本项目应用代码采用 MIT 许可证。第三方组件保留各自许可证，见[第三方声明](THIRD_PARTY_NOTICES.md)和 `THIRD_PARTY_LICENSES/`。发布包不包含密码、已有主机身份、私钥或配对记录。
