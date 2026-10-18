# Linux and Windows installation

[中文](../zh-CN/installation.md) · [Prepare Linux](linux-setup.md)

## Online Linux installation

Use an explicit release version. The script selects x86_64/aarch64, verifies the downloaded archive checksum, and calls the same installer used offline. Read the downloaded script before running it.

```bash
VERSION=v0.2.0-rc.1
curl -fL "https://github.com/edmondcvc/rust-usb-bridge/releases/download/$VERSION/install.sh" -o install.sh
less install.sh
sudo bash install.sh --version "$VERSION" --listen 0.0.0.0:3241 --server-name usb-bridge
```

The default listener accepts authenticated TLS on TCP 3241. Restrict access to your LAN/VPN through your normal firewall policy; do not open raw USB/IP port 3240. SSH is TCP 22 unless configured otherwise. No USB devices are shared by a fresh installation.

## Offline installation

Download the architecture-matched tarball and release `SHA256SUMS` on another computer. Transfer both. This software bundle contains the service and scripts, **not all distribution/kernel dependencies**: prepare those using [Linux setup](linux-setup.md) while internet is available.

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

`--binary FILE` selects a local service ELF; `--no-start` installs/enables the service without starting it. The installer checks architecture, prerequisites and existing installation ownership. Existing identities and pairings are kept.

## Check Linux

```bash
systemctl is-active rust-usb-bridge
sudo systemctl status rust-usb-bridge --no-pager
sudo journalctl -u rust-usb-bridge -b --no-pager -n 80
sudo /opt/rust-usb-bridge/usb-bridge-server --version
```

Success: service is `active`, version matches the release, and the log has no startup error. For failures see [troubleshooting](troubleshooting.md). Certificates and pairing state live in `/var/lib/rust-usb-bridge` (private); only the public CA/fingerprint are exposed under `/opt/rust-usb-bridge/public`.

## Windows x64 installation

1. Download `RustUSBBridge-Setup-0.2.0-rc.1-x64.exe` and `SHA256SUMS` from the same release. In PowerShell run `Get-FileHash .\RustUSBBridge-Setup-0.2.0-rc.1-x64.exe -Algorithm SHA256` and compare the complete value.
2. End device operations and release this computer's USB sessions before upgrading. Run setup and approve Windows administrator elevation. The unsigned application may show an unknown publisher; verify its origin and hash. The included upstream USB driver retains its own signature.
3. If the driver requires a reboot, save your work and reboot. Setup never forcibly reboots Windows. Install the original device driver/application when needed.
4. In Settings → Optional features, make sure **OpenSSH Client** is installed. Confirm in PowerShell: `Get-Command ssh.exe, ssh-keyscan.exe, ssh-keygen.exe`. On disconnected Windows systems, prepare the matching Microsoft optional-feature installation media beforehand.
5. Open **Rust USB 客户端** from the desktop. A new installation asks for your Linux address; it is not tied to a demonstration machine. Continue with [pairing and usage](usage.md).

Installed files are in `C:\Program Files\Rust USB Bridge`; service/private state is in `C:\ProgramData\RustUsbBridge`. Check `Get-Service UsbBridgeRust` if the UI cannot reach its background service. Current interface labels remain Chinese; this release provides bilingual documentation, not a translated GUI.
