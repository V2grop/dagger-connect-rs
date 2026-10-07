#!/usr/bin/env bash
# Produce a self-contained local installation bundle from an existing build.
set -euo pipefail
[[ $(uname -s) == Linux ]] || { echo 'Package on Linux using a Linux release binary.' >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary=${1:-$root/target/release/dagger-rs}
output=${2:-$root/dist}
[[ -x $binary ]] || { echo "Missing executable: $binary" >&2; exit 1; }
"$binary" --version
mkdir -p -- "$output"
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
bundle=$stage/dagger-rs-linux
mkdir -p -- "$bundle/bin" "$bundle/scripts"
install -m 755 "$binary" "$bundle/bin/dagger-rs"
install -m 755 "$root/scripts/install.sh" "$root/scripts/setup.sh" "$root/scripts/setup-classic.sh" "$root/scripts/install-release.sh" "$bundle/scripts/"
cp -- "$root/LICENSE" "$root/README.md" "$root/README.fa.md" "$bundle/"
cp -R -- "$root/docs" "$root/examples" "$root/licenses" "$bundle/"
mkdir -p -- "$bundle/vendor/tokio_kcp"
cp -- "$root/vendor/tokio_kcp/LICENSE" "$root/vendor/tokio_kcp/DAGGER-RS-PATCH.md" "$bundle/vendor/tokio_kcp/"
(cd -- "$bundle" && sha256sum bin/dagger-rs > SHA256SUMS)
archive=$output/dagger-rs-linux-$(uname -m).tar.gz
tar -C "$stage" -czf "$archive" dagger-rs-linux
(cd -- "$output" && sha256sum "$(basename -- "$archive")" > "$(basename -- "$archive").sha256")
bash "$root/scripts/package-selfextracting.sh" "$archive" "${archive%.tar.gz}.run" "$(uname -m)"
printf 'Local bundle: %s\n' "$archive"
echo 'Copy the .run file to the matching Linux machine and run it with sudo bash to install and open setup.'

# Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
# Attribution copy: Dagger Rust rewrite by i​r⁠_spoof; https://t.me/ir_spoof
