#!/usr/bin/env bash
# Incremental install only. Never partitions, formats, reimages, or changes VirtualHere.
set -Eeuo pipefail
umask 077
INSTALL_DIR=/opt/rust-usb-bridge
STATE_DIR=/var/lib/rust-usb-bridge
UNIT=/etc/systemd/system/rust-usb-bridge.service
LISTEN=0.0.0.0:3241
SERVER_NAME=usb-bridge
HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
BINARY="$HERE/../../usb-bridge-server"
START=1
usage() { printf '%s\n' 'Usage: sudo bash install-linux.sh [--binary FILE] [--listen IP:3241] [--server-name NAME] [--no-start]'; }
while (($#)); do
  case "$1" in
    --binary) BINARY=${2:?missing binary}; shift 2;;
    --listen) LISTEN=${2:?missing listen address}; shift 2;;
    --server-name) SERVER_NAME=${2:?missing name}; shift 2;;
    --no-start) START=0; shift;;
    -h|--help) usage; exit 0;;
    *) usage; exit 2;;
  esac
done
[[ $EUID -eq 0 ]] || { printf '%s\n' 'Run once with sudo on the Linux console.' >&2; exit 1; }
case $(uname -m) in x86_64|amd64) ARCH=x86_64;; aarch64|arm64) ARCH=aarch64;; *) echo 'Unsupported architecture: use x86_64 or aarch64.' >&2; exit 1;; esac
bash "$HERE/check-host.sh"
python3 - "$BINARY" "$ARCH" <<'PY'
import pathlib,sys
data=pathlib.Path(sys.argv[1]).read_bytes()[:20]
assert data[:6]==b'\x7fELF\x02\x01', 'Expected little-endian ELF64'
assert int.from_bytes(data[18:20],'little')=={'x86_64':62,'aarch64':183}[sys.argv[2]], 'Binary architecture mismatch'
PY
[[ -f "$BINARY" ]] || { printf 'Missing binary: %s\n' "$BINARY" >&2; exit 1; }
[[ "$LISTEN" =~ ^[a-zA-Z0-9_.-]+:[0-9]{1,5}$ && "$SERVER_NAME" =~ ^[a-zA-Z0-9._-]+$ ]] || { printf '%s\n' 'Use an IPv4 address or DNS name with a numeric port, and a valid server name.' >&2; exit 2; }
PORT=${LISTEN##*:}
((10#$PORT >= 1 && 10#$PORT <= 65535)) || { printf '%s\n' 'Port must be between 1 and 65535.' >&2; exit 2; }
command -v systemctl >/dev/null
command -v modprobe >/dev/null
command -v findmnt >/dev/null
ROOT_SOURCE=$(findmnt -n -o SOURCE /)
[[ -n "$ROOT_SOURCE" ]] || { printf '%s\n' 'Cannot identify mounted Linux root; refusing installation.' >&2; exit 1; }
for directory in "$INSTALL_DIR" "$STATE_DIR"; do
  [[ ! -L "$directory" && (! -e "$directory" || -d "$directory") ]] || { printf 'Unexpected directory or symlink: %s\n' "$directory" >&2; exit 1; }
done
if [[ -d "$INSTALL_DIR" ]] && [[ -n $(find "$INSTALL_DIR" -mindepth 1 -maxdepth 1 -print -quit) ]]; then
  [[ -f "$INSTALL_DIR/INSTALLATION_ID" ]] && [[ $(cat "$INSTALL_DIR/INSTALLATION_ID") == RustUsbBridge-v1 ]] || { printf '%s\n' 'Existing destination lacks this application identity; refusing overwrite.' >&2; exit 1; }
fi
if [[ -e "$UNIT" ]]; then
  [[ ! -L "$UNIT" ]] && grep -Eq '^ExecStart=/opt/rust-usb-bridge/usb-bridge-server run ' "$UNIT" || { printf '%s\n' 'An unrelated service unit occupies the selected name.' >&2; exit 1; }
fi
install -d -m 0755 "$INSTALL_DIR" "$INSTALL_DIR/public"
printf '%s\n' RustUsbBridge-v1 > "$INSTALL_DIR/INSTALLATION_ID"
chmod 0644 "$INSTALL_DIR/INSTALLATION_ID"
install -d -m 0700 "$STATE_DIR" /var/log/rust-usb-bridge
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
LOG=/var/log/rust-usb-bridge/install-$STAMP.log
exec > >(tee -a "$LOG") 2>&1
printf 'Incremental Rust USB install. Root filesystem: %s\n' "$ROOT_SOURCE"
printf '%s\n' 'Other USB services and original device drivers are retained.'
modprobe usbip-core
modprobe usbip-host
[[ -d /sys/bus/usb/drivers/usbip-host ]] || { printf '%s\n' 'usbip-host unavailable; install kernel modules matching uname -r before retrying.' >&2; exit 1; }
BACKUP="$INSTALL_DIR/backups/$STAMP"
install -d -m 0700 "$BACKUP"
[[ ! -f "$INSTALL_DIR/usb-bridge-server" ]] || cp -p -- "$INSTALL_DIR/usb-bridge-server" "$BACKUP/usb-bridge-server"
[[ ! -f "$UNIT" ]] || cp -p -- "$UNIT" "$BACKUP/rust-usb-bridge.service"
WAS_ACTIVE=0
systemctl is-active --quiet rust-usb-bridge.service && WAS_ACTIVE=1 || true
WAS_ENABLED=0
systemctl is-enabled --quiet rust-usb-bridge.service && WAS_ENABLED=1 || true
if ((WAS_ACTIVE)); then
  for status in /sys/bus/usb/devices/*/usbip_status; do
    [[ ! -f "$status" || $(cat "$status") != 2 ]] || { printf '%s\n' 'USB is in use. Release all devices before updating.' >&2; exit 1; }
  done
  systemctl stop rust-usb-bridge.service
fi
recover() {
  local rc=$?
  if ((rc)); then
    printf 'Installation failed (%s); preserving state and restoring previous bridge binary/unit. Log: %s\n' "$rc" "$LOG" >&2
    systemctl stop rust-usb-bridge.service 2>/dev/null || true
    if [[ -f "$BACKUP/rust-usb-bridge.service" ]]; then install -m 0644 "$BACKUP/rust-usb-bridge.service" "$UNIT";
    else systemctl disable rust-usb-bridge.service 2>/dev/null || true; rm -f -- "$UNIT"; fi
    if [[ -f "$BACKUP/usb-bridge-server" ]]; then install -m 0755 "$BACKUP/usb-bridge-server" "$INSTALL_DIR/usb-bridge-server";
    else rm -f -- "$INSTALL_DIR/usb-bridge-server"; fi
    systemctl daemon-reload
    if [[ -f "$BACKUP/rust-usb-bridge.service" ]]; then
      if ((WAS_ENABLED)); then systemctl enable rust-usb-bridge.service || true;
      else systemctl disable rust-usb-bridge.service 2>/dev/null || true; fi
    fi
    if ((WAS_ACTIVE)); then systemctl start rust-usb-bridge.service || true; fi
  fi
  exit "$rc"
}
trap recover EXIT
install -m 0755 "$BINARY" "$INSTALL_DIR/usb-bridge-server.new"
mv -- "$INSTALL_DIR/usb-bridge-server.new" "$INSTALL_DIR/usb-bridge-server"
if [[ ! -f "$STATE_DIR/config.json" ]]; then
  "$INSTALL_DIR/usb-bridge-server" init --state-dir "$STATE_DIR" --server-name "$SERVER_NAME" --server-name "${LISTEN%:*}"
else
  printf '%s\n' 'Keeping existing server identity, pairings, and share configuration.'
fi
for required in ca.pem ca-key.pem server.pem server-key.pem fingerprint.txt config.json; do
  [[ -s "$STATE_DIR/$required" ]] || { printf 'Incomplete existing server state: %s\n' "$required" >&2; exit 1; }
done
install -m 0644 "$STATE_DIR/ca.pem" "$INSTALL_DIR/public/ca.pem"
install -m 0644 "$STATE_DIR/fingerprint.txt" "$INSTALL_DIR/public/fingerprint.txt"
for script in rollback-linux.sh uninstall-linux.sh; do
  if [[ -f "$HERE/$script" ]]; then install -m 0755 "$HERE/$script" "$INSTALL_DIR/$script"; fi
done
cat > "$UNIT" <<EOF
[Unit]
Description=Rust USB Bridge TLS service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=root
Group=root
UMask=0077
ExecStartPre=$(command -v modprobe) usbip-host
ExecStart=$INSTALL_DIR/usb-bridge-server run --state-dir $STATE_DIR --listen $LISTEN
Restart=on-failure
RestartSec=3
TimeoutStopSec=180
KillSignal=SIGINT
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=full
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF
chmod 0644 "$UNIT"
systemctl daemon-reload
systemctl enable rust-usb-bridge.service
if ((START)); then
  systemctl start rust-usb-bridge.service
  systemctl is-active --quiet rust-usb-bridge.service
fi
printf '\nInstalled. Public certificate: %s/public/ca.pem\n' "$INSTALL_DIR"
printf 'Public fingerprint: %s/public/fingerprint.txt\n' "$INSTALL_DIR"
printf 'Show one-time pairing code only when pairing: sudo %s/usb-bridge-server pair-code --state-dir %s\n' "$INSTALL_DIR" "$STATE_DIR"
if ((!START)); then printf '%s\n' 'Prepared service is stopped now and enabled for the next normal boot (--no-start).'; fi
printf 'Log: %s\nA fresh install selects no devices; existing selected shares are preserved and may reconnect when the service starts.\n' "$LOG"
