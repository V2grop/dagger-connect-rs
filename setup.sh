#!/usr/bin/env bash
# Standalone entry point for the second (classic style) V2grop Rust menu.
set -euo pipefail
if [[ ${1:-} == --help ]]; then
  echo 'Usage: sudo bash setup.sh'
  echo 'Installs the latest verified V2grop Linux x86-64 release and opens its classic menu.'
  exit 0
fi
(( $# == 0 )) || { echo 'Unexpected arguments.' >&2; exit 2; }
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'Linux x86-64 is required.' >&2; exit 1; }
(( EUID == 0 )) || { echo 'Run: sudo bash setup.sh' >&2; exit 1; }
if ! command -v curl >/dev/null || ! command -v python3 >/dev/null || [[ ! -s /etc/ssl/certs/ca-certificates.crt ]]; then
  command -v apt-get >/dev/null || { echo 'Install ca-certificates, curl and python3 first.' >&2; exit 1; }
  apt-get update
  apt-get install -y ca-certificates curl python3
fi
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
curl --fail --silent --show-error --location --retry 3 \
  https://raw.githubusercontent.com/V2grop/dagger-connect-rs/main/scripts/install-release.sh \
  -o "$stage/install-release.sh"
bash "$stage/install-release.sh" --menu classic
