#!/usr/bin/env bash
set -Eeuo pipefail
marker=/var/lib/rust-usb-bridge-image/account-ready
user=$(cat /etc/rust-usb-image-user)
[[ $user =~ ^[a-z_][a-z0-9_-]*$ ]] || exit 1
echo "Set a new password for $user before SSH password enrollment / 请为账户设置自己的新密码。"
until passwd "$user"; do echo 'Password was not set; try again.'; done
touch "$marker"
echo 'Account ready. IP addresses / IP 地址:'
ip -brief address
echo 'SSH fingerprints / SSH 主机指纹:'
for key in /etc/ssh/ssh_host_*_key.pub; do ssh-keygen -lf "$key"; done
