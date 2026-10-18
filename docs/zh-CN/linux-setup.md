# 准备 Linux 主机

[English](../en/linux-setup.md) · [首页](../../README.zh-CN.md)

## 选择安装路线

| 设备或状态 | 安装方式 |
|---|---|
| 全新 Intel/AMD 64 位电脑 | 官方 Ubuntu Server 24.04 LTS amd64 安装盘，再安装本项目服务 |
| 支持 UEFI 的通用 ARM64 | 使用适合该硬件的 Ubuntu Server ARM64 安装盘或云镜像 |
| 树莓派 4／5 | Ubuntu Server 24.04 LTS 64 位树莓派镜像，不能使用通用 UEFI ARM 镜像替代 |
| 已安装 Ubuntu／Debian | 保留系统，从下面的依赖准备开始 |
| 主机不能联网 | 先在系统中准备依赖，再转移离线软件包 |

系统安装参考 [Ubuntu 官方教程](https://documentation.ubuntu.com/server/how-to/installation/index.html)或[树莓派启动介质教程](https://www.raspberrypi.com/documentation/computers/getting-started.html)。从官方链接下载并核对校验值。写盘前按型号和容量确认目标盘，因为写入会覆盖其数据。预装 `.img` 写到哪块盘就从哪块盘运行；安装 ISO 用于把系统安装到另一个选定磁盘。

全新电脑先接显示器、键盘、有线网络和合适电源。创建自己的管理员账户：客户端建议 `operator`，但服务安装器不会自动创建这个账户。启用 OpenSSH，设置自己的密码；本项目不提供默认密码。更新系统，内核更新后按提示重启。

## 安装依赖并检查

```bash
sudo apt update
sudo apt install openssh-server python3 kmod util-linux ca-certificates curl
sudo systemctl enable --now ssh
uname -m
uname -r
ip -brief address
sudo modprobe usbip-core
sudo modprobe usbip-host
```

架构应显示 `x86_64` 或 `aarch64`，模块加载后应出现 `/sys/bus/usb/drivers/usbip-host`。Ubuntu generic 内核缺少模块时，可执行 `sudo apt install linux-modules-extra-$(uname -r)`，需要时重启。树莓派必须使用与当前 **raspi** 内核匹配的模块，不能安装 x86 generic 内核代替。Debian 和自定义内核按自己的内核配置准备 USB/IP 模块。

用准备在 Windows 上输入的账户执行 `sudo -v`。当前界面的密码配对要求该账户能通过 SSH 密码登录，并能用同一密码取得 sudo 权限。只允许 SSH 密钥的环境可使用[使用教程](usage.md)中的高级证书／配对码方式。

无线网络按系统提供的网络工具配置。用 `ip -brief address` 查看新机器地址，项目没有固定 IP；建议在路由器设置 DHCP 地址保留。系统时间需要正确，以便验证证书有效期。查看主机 SSH 指纹：

```bash
for key in /etc/ssh/ssh_host_*_key.pub; do ssh-keygen -lf "$key"; done
```

随后进入[服务与 Windows 安装](installation.md)。SSH 能连接、模块能加载只是准备成功，还不代表 USB 已经转发。
