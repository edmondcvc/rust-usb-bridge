#!/usr/bin/env bash
set -Eeuo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
target=${1:-}
if [[ -n $target ]]; then
  case $target in x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|x86_64-unknown-linux-musl|aarch64-unknown-linux-musl) ;; *) echo 'Unsupported Linux target' >&2; exit 2;; esac
  rustup target add "$target"
  cargo build --release --locked --target "$target" -p bridge-server
else
  cargo build --release --locked -p bridge-server
fi
