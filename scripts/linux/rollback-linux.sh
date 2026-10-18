#!/usr/bin/env bash
set -Eeuo pipefail
umask 077
INSTALL_DIR=/opt/rust-usb-bridge
BACKUP_NAME=''
if (($#)); then
  [[ $# == 2 && $1 == --backup && $2 =~ ^[0-9]{8}T[0-9]{6}Z$ ]] || { echo 'Usage: sudo bash rollback-linux.sh [--backup YYYYMMDDTHHMMSSZ]' >&2; exit 2; }
  BACKUP_NAME=$2
fi
STATE_DIR=/var/lib/rust-usb-bridge
[[ $EUID -eq 0 ]] || { printf '%s\n' 'Run with sudo on the Linux console.' >&2; exit 1; }
[[ ! -L "$INSTALL_DIR" && -f "$INSTALL_DIR/INSTALLATION_ID" && $(cat "$INSTALL_DIR/INSTALLATION_ID") == RustUsbBridge-v1 ]] || { printf '%s\n' 'Missing application installation identity; refusing unscoped rollback.' >&2; exit 1; }
[[ ! -L "$STATE_DIR" ]] || { printf '%s\n' 'Unexpected state directory symlink.' >&2; exit 1; }
if [[ -e /etc/systemd/system/rust-usb-bridge.service ]]; then
  [[ ! -L /etc/systemd/system/rust-usb-bridge.service ]] || { printf '%s\n' 'Unexpected service unit symlink; refusing rollback.' >&2; exit 1; }
  grep -Eq '^ExecStart=/opt/rust-usb-bridge/usb-bridge-server run ' /etc/systemd/system/rust-usb-bridge.service || { printf '%s\n' 'Unexpected service unit; refusing rollback.' >&2; exit 1; }
fi
if [[ -n $BACKUP_NAME ]]; then
  backup="$INSTALL_DIR/backups/$BACKUP_NAME"
  [[ ! -L "$backup" && -f "$backup/usb-bridge-server" && -f "$backup/rust-usb-bridge.service" ]] || { echo 'Backup is incomplete.' >&2; exit 1; }
fi
install -d -m 0700 /var/log/rust-usb-bridge
LOG=/var/log/rust-usb-bridge/rollback-$(date -u +%Y%m%dT%H%M%SZ).log
exec > >(tee -a "$LOG") 2>&1
for status in /sys/bus/usb/devices/*/usbip_status; do
  [[ ! -f "$status" || $(cat "$status") != 2 ]] || { echo 'Release all USB devices before rollback.' >&2; exit 1; }
done
systemctl stop rust-usb-bridge.service || { systemctl is-active --quiet rust-usb-bridge.service && exit 1 || true; }
systemctl disable rust-usb-bridge.service 2>/dev/null || true
[[ ! -x "$INSTALL_DIR/usb-bridge-server" ]] || "$INSTALL_DIR/usb-bridge-server" restore --state-dir "$STATE_DIR"
printf '%s\n' 'Rust bridge stopped. Only its recorded matching USB bindings have been restored.'
printf '%s\n' 'VirtualHere was not modified. Reconnect the device in the original Windows VirtualHere client.'
printf '%s\n' 'If VirtualHere was deliberately stopped for migration, start its original service only after the preceding restore command succeeds.'
printf 'Log: %s\n' "$LOG"

if [[ -n $BACKUP_NAME ]]; then
  install -m 0755 "$backup/usb-bridge-server" "$INSTALL_DIR/usb-bridge-server"
  install -m 0644 "$backup/rust-usb-bridge.service" /etc/systemd/system/rust-usb-bridge.service
  systemctl daemon-reload
  systemctl enable rust-usb-bridge.service
  systemctl start rust-usb-bridge.service
  systemctl is-active --quiet rust-usb-bridge.service
  echo 'Previous binary restored; server identity and pairings retained.'
fi
