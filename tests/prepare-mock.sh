#!/usr/bin/env bash
# Make three simulated physical USB entries inside an isolated test directory.
# This script does not load modules, run sudo, or write the real /sys or /proc.
set -euo pipefail
umask 077

if [[ $# != 1 || "$1" != /* ]]; then
  printf 'Usage: bash prepare-mock.sh /absolute/new/mock-fixture-directory\n' >&2
  exit 2
fi
FIXTURE=$(realpath -m -- "$1")
case "$FIXTURE" in
  /|/sys|/sys/*|/proc|/proc/*|/dev|/dev/*|/etc|/etc/*|/boot|/boot/*)
    printf 'Refusing a system or hardware directory.\n' >&2
    exit 2
    ;;
esac
if [[ -e "$FIXTURE" ]]; then
  printf 'The fixture directory must be new; refusing to overwrite existing files.\n' >&2
  exit 2
fi
mkdir -p -- "$FIXTURE/sys/bus/usb/devices" \
  "$FIXTURE/sys/bus/usb/drivers/usb" \
  "$FIXTURE/sys/bus/usb/drivers/usbip-host" \
  "$FIXTURE/sys/class/net" "$FIXTURE/proc/self"
: > "$FIXTURE/proc/self/mountinfo"
: > "$FIXTURE/proc/swaps"
for attribute in bind unbind; do
  : > "$FIXTURE/sys/bus/usb/drivers/usb/$attribute"
done
for attribute in bind unbind match_busid rebind; do
  : > "$FIXTURE/sys/bus/usb/drivers/usbip-host/$attribute"
done

for number in 2 3 4; do
  busid="1-$number"
  device="$FIXTURE/sys/bus/usb/devices/$busid"
  interface="$FIXTURE/sys/bus/usb/devices/$busid:1.0"
  mkdir -p -- "$device" "$interface"
  # Synthetic IDs identify this fixture; no real device or laser is emulated.
  printf 'f000\n' > "$device/idVendor"
  printf '%04x\n' "$number" > "$device/idProduct"
  printf 'SIMULATED-BYTE-ECHO-%s\n' "$number" > "$device/serial"
  printf 'Rust bridge test fixture\n' > "$device/manufacturer"
  printf 'SIMULATED USB byte echo %s (no hardware)\n' "$number" > "$device/product"
  printf '1\n' > "$device/busnum"
  printf '%s\n' "$number" > "$device/devnum"
  printf '480\n' > "$device/speed"
  printf '0100\n' > "$device/bcdDevice"
  printf 'ff\n' > "$device/bDeviceClass"
  printf '00\n' > "$device/bDeviceSubClass"
  printf '00\n' > "$device/bDeviceProtocol"
  printf '1\n' > "$device/bConfigurationValue"
  printf '1\n' > "$device/bNumConfigurations"
  printf '1\n' > "$device/usbip_status"
  printf 'usb\n' > "$device/mock-driver"
  : > "$device/usbip_sockfd"
  printf 'ff\n' > "$interface/bInterfaceClass"
  printf '00\n' > "$interface/bInterfaceSubClass"
  printf '00\n' > "$interface/bInterfaceProtocol"
done
printf 'Mock fixture v1: isolated files; three synthetic byte-echo devices; no physical USB.\n' > "$FIXTURE/fixture-description.txt"
printf 'Created isolated simulated fixture at %s\n' "$FIXTURE"
printf 'Use server --mock --sysfs-root %s/sys --proc-root %s/proc with a separate new state directory.\n' "$FIXTURE" "$FIXTURE"
