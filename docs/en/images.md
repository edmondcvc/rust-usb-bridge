# Clean preinstalled images

[中文](../zh-CN/images.md) · [Support status](support.md)

Images are constructed from checksummed official Ubuntu bases, never from a deployed computer. They include this project's service, OpenSSH and required OS dependencies. VirtualHere, VPN login identities and other workstation applications are not included.

## Select the boot platform

| Platform argument | Base | Boot target |
|---|---|---|
| `x86_64` | Ubuntu 24.04 amd64 cloud disk | Generic x86_64; verify firmware boot mode on the actual machine |
| `uefi-arm64` | Ubuntu 24.04 arm64 cloud disk | Generic ARM64 UEFI; board firmware/drivers must support the Ubuntu base |
| `raspi4` | Ubuntu 24.04 ARM64 Raspberry Pi preinstalled disk | Raspberry Pi 4 |
| `raspi5` | Ubuntu 24.04 ARM64 Raspberry Pi preinstalled disk | Raspberry Pi 5 |

Pi 4 and 5 are separately named and accepted even when the upstream base supports both. Do not write a generic UEFI disk to a Pi and assume it has the required board boot files. Base image catalogs: [Ubuntu cloud images](https://cloud-images.ubuntu.com/noble/current/), [Ubuntu Pi releases](https://cdimage.ubuntu.com/releases/24.04/release/).

## Build on Linux

Use a Linux builder of the **same CPU architecture** as the image with enough disk space for the uncompressed input, output and libguestfs appliance. A VM is acceptable for construction. Install `libguestfs-tools qemu-utils xz-utils python3`, obtain the official image and its official `SHA256SUMS`, then supply the exact hash. Use the architecture-matched service ELF from the same project release.

```bash
sudo apt install libguestfs-tools qemu-utils xz-utils python3 dhcpcd-base
bash scripts/images/build-image.sh \
  --platform x86_64 \
  --base /work/noble-server-cloudimg-amd64.img \
  --base-url https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img \
  --base-sha256 YOUR_OFFICIAL_64_HEX_DIGIT_HASH \
  --binary /work/usb-bridge-server \
  --output /work/new-image-output
```

ARM UEFI uses the arm64 base and `--platform uefi-arm64`. Pi uses an official `...arm64+raspi.img.xz` and `--platform raspi4` or `raspi5`. Never substitute a private machine snapshot. The build refuses incorrect architecture, wrong base checksum, reused output directories and missing USB/IP modules. It records base URL/hash, final image hash and **pending** hardware acceptance in `image-provenance.json`. The manual GitHub **Build clean Ubuntu image** workflow provides the same recipe with native architecture runners and a successful same-commit CI input.

Run the image builder as your normal account in a user-writable directory. Only dependency installation needs sudo; libguestfs edits the guest filesystem without requiring builder root access.

On Ubuntu, supermin can fail when `/boot/vmlinuz-*` is readable only by root. Copy the public kernel into a user-writable working directory and identify its matching modules (see the [libguestfs FAQ](https://www.libguestfs.org/guestfs-faq.1.html)):

```bash
mkdir -p "$PWD/guestfs-work"
sudo install -m 0644 "/boot/vmlinuz-$(uname -r)" "$PWD/guestfs-work/kernel"
export SUPERMIN_KERNEL="$PWD/guestfs-work/kernel"
export SUPERMIN_KERNEL_VERSION="$(uname -r)"
export SUPERMIN_MODULES="/lib/modules/$(uname -r)"
```

Retry with a new output directory. For other appliance startup failures, set `LIBGUESTFS_DEBUG=1 LIBGUESTFS_TRACE=1` to capture the underlying error.

Ubuntu 24.04's `passt` package can reject libguestfs PID files with `Permission denied`. The disposable CI builder removes that optional helper to select libguestfs's QEMU user-networking fallback. For local construction, use a dedicated builder VM with this workaround or a distribution with the corrected helper; inspect dependency changes before removing packages on a shared host. The workflow grants only its build user access to `/dev/kvm` when present. Hardware acceleration is optional but substantially faster than TCG emulation.

## Download, verify and write

Only use image assets actually present in a release; a recipe is not evidence that a boot image has passed acceptance. All RC images remain hardware-unverified until the matrix is updated. If published as 1-GiB volumes, download every part and the matching `.volumes.json`; use the release's `release-tools.py`:

```powershell
py release-tools.py merge --file .\IMAGE.img.gz.volumes.json --output .\merged
```

The merge checks every part and the complete compressed file and refuses to overwrite an output. Decompress `.img.gz` with a standard gzip-capable tool, then write the `.img` using a reputable imaging tool following its official instructions. Confirm the target's model/capacity: this replaces its contents. The image runs from that disk; do not overwrite the Linux system disk currently in use.

## First boot

Attach a display/keyboard and wired DHCP network. The first boot creates a new machine identity, hostname, SSH host keys and TLS identity; no USB devices are preselected. The local screen asks you to choose a password for `operator` (or your build's `--username`). There is **no shared default password**. SSH password login is usable only after you set this password locally. Root SSH password login is disabled. The private image is not intended as an unattended system with a published default credential.

Find the IP on screen or with `ip -brief address`. Root filesystem expansion is attempted only for a plain ext4 root partition and is logged; check `df -h /`, particularly on nonstandard firmware/storage layouts. Wi-Fi credentials are not supplied. Then follow [Windows installation](installation.md) and [pairing](usage.md).

```bash
systemctl status rust-usb-firstboot rust-usb-account rust-usb-bridge ssh --no-pager
sudo journalctl -u rust-usb-firstboot -b --no-pager
df -h /
```

Booting in QEMU proves only that virtual configuration. Each physical ARM machine and Pi model still requires boot/network/kernel-module and real-USB validation before stable support.

