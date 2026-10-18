# Rust USB Bridge

[中文](README.zh-CN.md)

Share physical USB devices attached to a Linux host with Windows computers. Each device has one owner at a time; different computers can use different devices concurrently.

[Download and install](docs/en/installation.md) | [Start with a new Linux computer](docs/en/linux-setup.md) | [Pair and use devices](docs/en/usage.md) | [Build from source](docs/en/building.md)

## Release status

The first public release is **0.2.0-rc.1**. It is a prerelease, not a claim of production certification. Linux x86_64 and aarch64 binaries are separate; generic UEFI ARM64 and Raspberry Pi 4/5 require different boot images. Windows is x64. See the [support and acceptance matrix](docs/en/support.md).

The application uses Linux `usbip-host`, mutual TLS, and the unchanged signed usbip-win2 0.9.8.1 driver (PnP version 12.1.30.403). Original device applications/drivers remain necessary. The application installer is not commercially code-signed.

## Quick path

1. Prepare Ubuntu Server 24.04 LTS, OpenSSH, Python 3 and a sudo-capable account.
2. Download the pinned release's `install.sh`, review it, then run it with sudo. An offline tarball uses the same installer.
3. Install the Windows setup executable; reboot if the driver installer requests it.
4. Enter the Linux address, username and password, verify the SSH host fingerprint, and select an available USB device.
5. Close the window to keep it in the tray; single-click the blue USB tray icon to restore it.

Storage must be safely ejected before releasing it. A network disconnect is not an emergency stop for machinery. Kernel transport support does not prove compatibility with every physical device.

## More

[Images](docs/en/images.md) · [Offline installation](docs/en/installation.md#offline-installation) · [Remote networking](docs/en/networking.md) · [Troubleshooting](docs/en/troubleshooting.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) · [Changelog](CHANGELOG.md)

MIT covers this application. Third-party components retain their own licenses; see [notices](THIRD_PARTY_NOTICES.md) and `THIRD_PARTY_LICENSES/`. No account password, server identity, private key, or existing pairing is included in a release.
