#!/usr/bin/env bash
# Download a published V2grop maintenance bundle, verify it and open setup.
set -euo pipefail
menu=original
while (( $# )); do
  case $1 in
    --menu) menu=${2:?Missing menu}; shift 2 ;;
    -h|--help) echo 'Usage: bash install-release.sh [--menu original|classic]'; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done
case $menu in original|classic) ;; *) echo 'Menu must be original or classic.' >&2; exit 2 ;; esac
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || {
  echo 'This release installer supports Linux x86-64 only.' >&2; exit 1;
}
for required in curl python3 sha256sum mktemp; do
  command -v "$required" >/dev/null || { echo "Missing command: $required" >&2; exit 1; }
done
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
repo=V2grop/dagger-connect-rs
if ! curl --fail --silent --show-error --location --retry 3 \
    "https://api.github.com/repos/$repo/releases/latest" -o "$stage/release.json"; then
  echo 'No published release could be fetched. Check GitHub Releases and try again.' >&2
  exit 1
fi
tag=$(python3 - "$stage/release.json" <<'PY'
import json, re, sys
with open(sys.argv[1]) as f:
    release = json.load(f)
tag = release.get('tag_name', '')
if not re.fullmatch(r'v0\.2\.1-v2grop\.[0-9]+', tag):
    raise SystemExit('Unexpected release tag; installation stopped.')
assets = {asset['name'] for asset in release.get('assets', [])}
required = {'dagger-rs-linux-x86_64.run', 'dagger-rs-linux-x86_64.run.sha256'}
if not required <= assets:
    raise SystemExit('Release assets are incomplete; installation stopped.')
print(tag)
PY
)
base="https://github.com/$repo/releases/download/$tag"
for name in dagger-rs-linux-x86_64.run dagger-rs-linux-x86_64.run.sha256; do
  curl --fail --silent --show-error --location --retry 3 "$base/$name" -o "$stage/$name"
done
(cd -- "$stage" && sha256sum --check --strict dagger-rs-linux-x86_64.run.sha256)
printf 'Installing verified release %s\n' "$tag"
if (( EUID == 0 )); then
  bash "$stage/dagger-rs-linux-x86_64.run" --menu "$menu"
else
  sudo bash "$stage/dagger-rs-linux-x86_64.run" --menu "$menu"
fi
