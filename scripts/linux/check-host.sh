#!/usr/bin/env bash
set -Eeuo pipefail
case $(uname -m) in x86_64|aarch64|amd64|arm64) ;; *) echo 'Unsupported architecture; use 64-bit x86_64 or aarch64 Linux.' >&2; exit 1;; esac
for tool in systemctl modprobe findmnt python3 ssh-keygen; do
  command -v "$tool" >/dev/null || { echo "Missing $tool. On Ubuntu: sudo apt install openssh-server python3 kmod util-linux" >&2; exit 1; }
done
[[ -d /run/systemd/system ]] || { echo 'systemd must be running; containers/WSL are not the standard deployment target.' >&2; exit 1; }
if [[ ! -d /sys/bus/usb/drivers/usbip-host ]]; then
  modprobe -n usbip-host >/dev/null 2>&1 || { echo "usbip-host missing for $(uname -r). Install the matching kernel modules (Ubuntu generic: linux-modules-extra-\$(uname -r)); Pi: verify the raspi kernel provides USB/IP." >&2; exit 1; }
fi
systemctl is-active --quiet ssh || systemctl is-active --quiet sshd || { echo 'Start OpenSSH: sudo systemctl enable --now ssh' >&2; exit 1; }
printf 'HOST_CHECK_OK arch=%s kernel=%s\n' "$(uname -m)" "$(uname -r)"
