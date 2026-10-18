#!/usr/bin/env bash
# Enable the existing authenticated USB service on LAN/VPN interfaces.
set -Eeuo pipefail
listen=0.0.0.0:3241
apply=0
while (($#)); do
  case "$1" in
    --listen) listen=${2:?missing address}; shift 2;;
    --apply) apply=1; shift;;
    *) printf 'Usage: sudo bash configure-remote-listener.sh [--listen IP:PORT] [--apply]\n' >&2; exit 2;;
  esac
done
python3 - "$listen" <<'PY'
import sys,ipaddress
host,port=sys.argv[1].rsplit(':',1)
ipaddress.ip_address(host.strip('[]'))
assert 1 <= int(port) <= 65535
PY
printf 'USB service listen address: %s\n' "$listen"
if (( ! apply )); then printf 'Preview only. Add --apply to save and restart after all USB connections are released.\n'; exit 0; fi
[[ $EUID -eq 0 ]] || { printf 'Root required.\n' >&2; exit 1; }
test -x /opt/rust-usb-bridge/usb-bridge-server
test -f /var/lib/rust-usb-bridge/config.json
python3 - <<'PY'
from pathlib import Path
for status in Path('/sys/bus/usb/drivers/usbip-host').glob('*/usbip_status'):
    if status.read_text().strip() == '2':
        raise SystemExit('USB is in use. Release all devices before changing network settings.')
PY
directory=/etc/systemd/system/rust-usb-bridge.service.d
file=$directory/remote-listen.conf
install -d -m 755 "$directory"
backup=$(mktemp /tmp/rust-usb-listener.XXXXXX)
had_file=0
if [[ -f $file ]]; then cp -p "$file" "$backup"; had_file=1; fi
rollback() {
  local result=$?
  if (( result != 0 )); then
    if (( had_file )); then cp -p "$backup" "$file"; else rm -f "$file"; fi
    systemctl daemon-reload
    systemctl start rust-usb-bridge || true
  fi
  rm -f "$backup"
}
trap rollback EXIT
# Stop before changing configuration; preserve the prior listener on stop failure.
systemctl stop rust-usb-bridge
printf '[Service]\nExecStart=\nExecStart=/opt/rust-usb-bridge/usb-bridge-server run --state-dir /var/lib/rust-usb-bridge --listen %s\n' "$listen" > "$file"
chmod 644 "$file"
systemctl daemon-reload
systemctl start rust-usb-bridge
systemctl is-active --quiet rust-usb-bridge
printf 'REMOTE_LISTENER_READY\n'
