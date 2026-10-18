#!/usr/bin/env bash
# Build from an explicitly checksummed OFFICIAL Ubuntu base; never a deployed disk.
set -Eeuo pipefail
platform=''; base=''; hash=''; binary=''; output=''; base_url=''; user=operator
while (($#)); do
  case "$1" in
    --platform) platform=${2:?}; shift 2;; --base) base=${2:?}; shift 2;;
    --base-sha256) hash=${2:?}; shift 2;; --base-url) base_url=${2:?}; shift 2;;
    --binary) binary=${2:?}; shift 2;; --output) output=${2:?}; shift 2;;
    --username) user=${2:?}; shift 2;;
    -h|--help) echo 'Usage: sudo bash build-image.sh --platform x86_64|uefi-arm64|raspi4|raspi5 --base OFFICIAL_IMAGE --base-sha256 HASH --base-url HTTPS_URL --binary ELF --output NEW_DIRECTORY [--username operator]'; exit 0;;
    *) echo "Unknown argument: $1" >&2; exit 2;;
  esac
done
case $platform in x86_64) arch=x86_64;; uefi-arm64|raspi4|raspi5) arch=aarch64;; *) echo 'Select an explicit boot platform.' >&2; exit 2;; esac
[[ $(uname -m) == "$arch" ]] || { echo 'Run on a matching-architecture Linux builder with libguestfs (a VM is sufficient for construction, not physical acceptance).' >&2; exit 1; }
[[ $EUID == 0 && -f $base && -f $binary && -n $output && ! -e $output ]] || { echo 'Use sudo, existing inputs and a new output directory.' >&2; exit 1; }
[[ $user =~ ^[a-z_][a-z0-9_-]*$ && $hash =~ ^[a-fA-F0-9]{64}$ ]] || exit 2
case $base_url in https://cloud-images.ubuntu.com/*|https://cdimage.ubuntu.com/*) ;; *) echo 'Only explicitly identified official Ubuntu base images are accepted.' >&2; exit 2;; esac
case $platform in raspi*) [[ $base_url == *raspi* ]] || { echo 'Use the Ubuntu ARM64 Raspberry Pi image.' >&2; exit 2; };; *) [[ $base_url != *raspi* ]] || { echo 'Pi boot images are not generic UEFI images.' >&2; exit 2; };; esac
for tool in qemu-img virt-customize python3 gzip sha256sum; do command -v "$tool" >/dev/null; done
[[ $(sha256sum "$base" | cut -d' ' -f1) == "${hash,,}" ]] || { echo 'Official base checksum mismatch.' >&2; exit 1; }
python3 - "$binary" "$arch" <<'PY'
import pathlib,sys
d=pathlib.Path(sys.argv[1]).read_bytes()[:20]
assert d[:6]==b'\x7fELF\x02\x01' and int.from_bytes(d[18:20],'little')=={'x86_64':62,'aarch64':183}[sys.argv[2]], 'Wrong binary architecture'
PY
mkdir -- "$output"
output=$(realpath "$output")
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
input=$base
if [[ $base == *.xz ]]; then xz -dc -- "$base" > "$work/base.img"; input=$work/base.img; fi
format=$(qemu-img info --output=json "$input" | python3 -c 'import json,sys; print(json.load(sys.stdin)["format"])')
image="$output/RustUSBBridge-Ubuntu24.04-0.2.0-rc.1-$platform.img"
qemu-img convert -f "$format" -O raw "$input" "$image"
cat > "$work/provision.sh" <<EOF
#!/bin/bash
set -Eeuo pipefail
if ! getent passwd '$user' >/dev/null; then
  if getent group '$user' >/dev/null; then
    useradd -m -s /bin/bash -g '$user' -G sudo '$user'
  else
    useradd -m -s /bin/bash -G sudo '$user'
  fi
fi
passwd -l '$user'
if [[ '$user' != ubuntu ]] && getent passwd ubuntu >/dev/null; then passwd -l ubuntu; fi
echo '$user' > /etc/rust-usb-image-user
install -d -m 0755 /etc/ssh/sshd_config.d
printf 'PasswordAuthentication yes\nPermitRootLogin no\n' > /etc/ssh/sshd_config.d/00-rust-usb-image.conf
install -d -m 0700 /var/lib/rust-usb-bridge /var/lib/rust-usb-bridge-image
install -d -m 0755 /opt/rust-usb-bridge/public /etc/netplan
echo RustUsbBridge-v1 > /opt/rust-usb-bridge/INSTALLATION_ID
touch /etc/cloud/cloud-init.disabled
rm -f /etc/netplan/50-cloud-init.yaml
cat > /etc/netplan/90-rust-usb-image.yaml <<'NET'
network:
  version: 2
  renderer: networkd
  ethernets:
    wired:
      match:
        name: "e*"
      dhcp4: true
      optional: true
NET
chmod 0600 /etc/netplan/90-rust-usb-image.yaml
# Confirm the image's own kernel has USB/IP; do not borrow builder modules.
if ! find /lib/modules -name 'usbip-host.ko*' -print -quit | grep -q .; then
  for kernel in /lib/modules/*; do apt-get install -y "linux-modules-extra-\$(basename "\$kernel")"; done
fi
find /lib/modules -name 'usbip-host.ko*' -print -quit | grep -q .
systemctl enable ssh rust-usb-firstboot rust-usb-account rust-usb-bridge
EOF
cat > "$work/rust-usb-firstboot.service" <<'UNIT'
[Unit]
Description=Generate independent Rust USB image identity
After=local-fs.target systemd-random-seed.service
Before=ssh.service rust-usb-bridge.service
ConditionPathExists=!/var/lib/rust-usb-bridge-image/initialized
[Service]
Type=oneshot
ExecStart=/opt/rust-usb-bridge/firstboot.sh
RemainAfterExit=yes
[Install]
WantedBy=multi-user.target
UNIT
cat > "$work/rust-usb-account.service" <<'UNIT'
[Unit]
Description=Set first local administrator password
After=rust-usb-firstboot.service
Before=getty@tty1.service
ConditionPathExists=!/var/lib/rust-usb-bridge-image/account-ready
[Service]
Type=oneshot
ExecStart=/opt/rust-usb-bridge/first-account.sh
StandardInput=tty-force
StandardOutput=tty
TTYPath=/dev/tty1
TimeoutStartSec=0
[Install]
WantedBy=multi-user.target
UNIT
cat > "$work/rust-usb-bridge.service" <<'UNIT'
[Unit]
Description=Rust USB Bridge TLS service
Wants=network-online.target
After=network-online.target rust-usb-firstboot.service
Requires=rust-usb-firstboot.service
[Service]
Type=simple
User=root
Group=root
UMask=0077
ExecStartPre=/usr/sbin/modprobe usbip-host
ExecStart=/opt/rust-usb-bridge/usb-bridge-server run --state-dir /var/lib/rust-usb-bridge --listen 0.0.0.0:3241
Restart=on-failure
RestartSec=3
TimeoutStopSec=180
KillSignal=SIGINT
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=full
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
[Install]
WantedBy=multi-user.target
UNIT
virt-customize -a "$image" --install openssh-server,python3,kmod,util-linux,cloud-guest-utils,e2fsprogs \
  --mkdir /opt/rust-usb-bridge \
  --upload "$binary:/opt/rust-usb-bridge/usb-bridge-server" \
  --upload "$here/firstboot.sh:/opt/rust-usb-bridge/firstboot.sh" \
  --upload "$here/first-account.sh:/opt/rust-usb-bridge/first-account.sh" \
  --upload "$work/rust-usb-firstboot.service:/etc/systemd/system/rust-usb-firstboot.service" \
  --upload "$work/rust-usb-account.service:/etc/systemd/system/rust-usb-account.service" \
  --upload "$work/rust-usb-bridge.service:/etc/systemd/system/rust-usb-bridge.service" \
  --run-command 'chmod 0755 /opt/rust-usb-bridge/*.sh /opt/rust-usb-bridge/usb-bridge-server' \
  --upload "$work/provision.sh:/tmp/rust-usb-provision.sh" --run-command 'bash /tmp/rust-usb-provision.sh && rm /tmp/rust-usb-provision.sh' --delete '/etc/ssh/ssh_host_*' \
  --write /etc/machine-id: --delete /var/lib/dbus/machine-id \
  --delete /var/lib/cloud --delete /root/.ssh --delete /home/ubuntu/.ssh --truncate /var/log/wtmp --truncate /var/log/btmp
gzip -n "$image"
python3 - "$output" "$platform" "$base_url" "$hash" <<'PY'
import hashlib,json,pathlib,sys
out=pathlib.Path(sys.argv[1]); image=next(out.glob('*.img.gz'))
h=hashlib.sha256()
with image.open('rb') as f:
 for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
(out/'SHA256SUMS').write_text(h.hexdigest()+'  '+image.name+'\n')
(out/'image-provenance.json').write_text(json.dumps(dict(platform=sys.argv[2],base_url=sys.argv[3],base_sha256=sys.argv[4],image_sha256=h.hexdigest(),physical_boot_acceptance='pending',physical_usb_acceptance='pending'),indent=2)+'\n')
PY
echo "Built $image.gz; boot and real-USB validation are still required."

