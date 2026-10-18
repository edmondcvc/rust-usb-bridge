#!/usr/bin/env bash
set -Eeuo pipefail
INSTALL_DIR=/opt/rust-usb-bridge
STATE_DIR=/var/lib/rust-usb-bridge
[[ $EUID -eq 0 ]] || { printf '%s\n' 'Run with sudo.' >&2; exit 1; }
"$INSTALL_DIR/rollback-linux.sh"
# Preserve machine identity, pairings, and logs for an intentional reinstall.
[[ "$INSTALL_DIR" == /opt/rust-usb-bridge ]] || exit 1
rm -f -- /etc/systemd/system/rust-usb-bridge.service
rm -f -- "$INSTALL_DIR/usb-bridge-server" "$INSTALL_DIR/public/ca.pem" "$INSTALL_DIR/public/fingerprint.txt"
systemctl daemon-reload
printf 'Removed own executable/service only. State retained at %s; logs retained at /var/log/rust-usb-bridge.\n' "$STATE_DIR"
printf '%s\n' 'Original VirtualHere, OS configuration, network, and USB drivers were not removed.'
