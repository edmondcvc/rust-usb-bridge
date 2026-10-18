#!/usr/bin/env bash
# Download a pinned release. Both online and offline routes use install-linux.sh.
set -Eeuo pipefail
version=v0.2.0-rc.1
repo=edmondcvc/rust-usb-bridge
args=()
while (($#)); do
  case "$1" in
    --version) version=${2:?missing version}; shift 2;;
    --listen|--server-name) args+=("$1" "${2:?missing value}"); shift 2;;
    --no-start) args+=("$1"); shift;;
    -h|--help) echo 'Usage: sudo bash install.sh [--version v0.2.0-rc.1] [--listen IP:PORT] [--server-name NAME] [--no-start]'; exit 0;;
    *) echo "Unknown option: $1" >&2; exit 2;;
  esac
done
[[ $version =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo 'Invalid version' >&2; exit 2; }
case $(uname -m) in x86_64|amd64) arch=x86_64;; aarch64|arm64) arch=aarch64;; *) echo 'Only x86_64 and aarch64 Linux are supported' >&2; exit 2;; esac
for command in curl python3 tar sha256sum; do command -v "$command" >/dev/null || { echo "Missing dependency: $command" >&2; exit 1; }; done
[[ $EUID -eq 0 ]] || { echo 'Run with sudo after reviewing this script' >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
asset=RustUSBBridge-Linux-${version#v}-$arch.tar.gz
base=https://github.com/$repo/releases/download/$version
curl --fail --location --proto '=https' --tlsv1.2 "$base/$asset" -o "$work/$asset"
curl --fail --location --proto '=https' --tlsv1.2 "$base/SHA256SUMS" -o "$work/SHA256SUMS"
python3 - "$work" "$asset" <<'PY'
import hashlib,pathlib,sys,tarfile
root=pathlib.Path(sys.argv[1]); name=sys.argv[2]
entries=[line.split() for line in (root/'SHA256SUMS').read_text().splitlines() if line.strip()]
matches=[parts[0] for parts in entries if len(parts)==2 and parts[1].lstrip('*')==name]
assert len(matches)==1 and hashlib.sha256((root/name).read_bytes()).hexdigest()==matches[0], 'Release checksum mismatch'
with tarfile.open(root/name) as archive:
    for item in archive.getmembers():
        path=pathlib.PurePosixPath(item.name)
        assert not path.is_absolute() and '..' not in path.parts and (item.isfile() or item.isdir()), 'Unsafe archive entry'
PY
tar -xzf "$work/$asset" -C "$work"
bash "$work/scripts/linux/install-linux.sh" "${args[@]}"
