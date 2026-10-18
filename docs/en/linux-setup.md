# Prepare a Linux host

[中文](../zh-CN/linux-setup.md) · [Home](../../README.md)

## Choose a route

| Hardware/state | Route |
|---|---|
| New Intel/AMD 64-bit computer | Official Ubuntu Server 24.04 LTS amd64 installer, then the service package |
| Generic ARM64 with UEFI | Official Ubuntu Server ARM64 installer/cloud image appropriate to the hardware |
| Raspberry Pi 4 or 5 | Ubuntu Server 24.04 LTS 64-bit Raspberry Pi image, not the generic UEFI ARM image |
| Existing Ubuntu/Debian | Keep the OS and start at prerequisites below |
| No internet on the host | Prepare OS prerequisites on the image first; transfer the offline software package |

Follow the [Ubuntu installer guide](https://documentation.ubuntu.com/server/how-to/installation/index.html) or [Raspberry Pi boot-media guide](https://www.raspberrypi.com/documentation/computers/getting-started.html). Download from official links and check the published checksum. Writing boot media overwrites the selected target: identify the disk by model and size. A preinstalled `.img` runs from the written disk; an installer ISO installs the OS onto another selected disk.

For a new machine, attach a display, keyboard, wired network and suitable power supply. Create your own account (the GUI suggests `operator`, but that account is not created by the service installer). Enable OpenSSH during Ubuntu setup. Use a unique password; no default password is supplied by this project. Update the system and reboot after kernel updates.

## Prerequisites

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

Expected architecture: `x86_64` or `aarch64`; expected module directory: `/sys/bus/usb/drivers/usbip-host`. If `modprobe` fails on an Ubuntu generic kernel, try `sudo apt install linux-modules-extra-$(uname -r)` and reboot if required. On Raspberry Pi, use modules matching the installed **raspi** kernel; do not install x86 generic kernel packages. Debian and custom kernels need their own matching USB/IP module configuration.

Run `sudo -v` as the account you will enter on Windows. SSH authentication and sudo must accept that account's password for the current password-based GUI enrollment. Key-only SSH setups can use the advanced CA/code pairing route in [usage](usage.md).

For Wi-Fi use the network configuration supplied by your OS. Find the new address with `ip -brief address`; there is no project-wide fixed IP. Prefer a DHCP reservation on the router. Synchronize the system clock so certificate validity checks work. Check local SSH key fingerprints:

```bash
for key in /etc/ssh/ssh_host_*_key.pub; do ssh-keygen -lf "$key"; done
```

Then continue with [service and Windows installation](installation.md). Success means SSH reaches the intended machine and the correct kernel module loads; it does not yet mean a USB device has been forwarded.
