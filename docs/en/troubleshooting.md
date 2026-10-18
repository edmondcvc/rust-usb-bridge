# Troubleshooting

[中文](../zh-CN/troubleshooting.md)

| Symptom | Action |
|---|---|
| Service will not start | Check journal, ELF architecture, kernel modules and file permissions |
| Host state unknown | Check power, network, address and SSH/TLS ports; cached data does not prove availability |
| Password/permission failure | Verify password SSH and sudo with that password; different sudo passwords are not supported by the shortcut enrollment |
| Host key changed | Verify address/reinstallation; do not silently erase trust to bypass the check |
| Windows driver not ready | Review installer logs/reboot requirement and repair with setup |
| Device owned by another computer | Ask its operator to release; administrator release must match the selected session |
| Missing USB device | Check cables, power, Linux enumeration and other USB exporters |
| Window hidden after close | Open taskbar overflow and single-click the blue USB icon |

Preserve the exact error; do not erase identities or reinstall the OS as a guess. Redact passwords, private keys, invitations, hostnames and real addresses before posting logs. Include OS/CPU/kernel/application version, device VID/PID, reproduction steps and time.

Installation logs: Linux `/var/log/rust-usb-bridge/`, Windows `C:\ProgramData\RustUsbBridge\logs`. Share failures and offline hosts are different states. Boot disks, mounted storage, maintenance input devices and hubs are protected; do not force them into forwarding.
