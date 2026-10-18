#!/usr/bin/env bash
set -Eeuo pipefail
umask 077
state=/var/lib/rust-usb-bridge
marker=/var/lib/rust-usb-bridge-image
install -d -m 0700 "$state" "$marker"
if [[ -e $marker/initialized ]]; then exit 0; fi
# systemd generates a new machine-id from the empty template before this unit.
suffix=$(cut -c1-8 /etc/machine-id)
hostnamectl set-hostname "usb-bridge-$suffix"
ssh-keygen -A
modprobe usbip-core
modprobe usbip-host
if [[ ! -e $state/config.json ]]; then
  /opt/rust-usb-bridge/usb-bridge-server init --state-dir "$state" --server-name usb-bridge --server-name "usb-bridge-$suffix"
fi
install -d -m 0755 /opt/rust-usb-bridge/public
install -m 0644 "$state/ca.pem" /opt/rust-usb-bridge/public/ca.pem
install -m 0644 "$state/fingerprint.txt" /opt/rust-usb-bridge/public/fingerprint.txt
printf '%s\n' 'Identity ready; no devices are selected for sharing.'
touch "$marker/initialized"
# Expansion is attempted only for a plain ext4 root partition, never LVM/RAID.
root=$(findmnt -n -o SOURCE /)
parent=$(lsblk -ndo PKNAME "$root" 2>/dev/null || true)
number=$(cat "/sys/class/block/$(basename "$root")/partition" 2>/dev/null || true)
if [[ -n $parent && $number =~ ^[0-9]+$ && $(findmnt -n -o FSTYPE /) == ext4 ]]; then
  growpart "/dev/$parent" "$number" || echo 'Root partition not expanded; inspect disk layout if extra capacity is required.'
  resize2fs "$root" || echo 'Root filesystem expansion failed; inspect before using extra capacity.'
fi
