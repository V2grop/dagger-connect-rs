#!/usr/bin/env bash
# Install only a local, already-built binary. This script never downloads code.
set -euo pipefail
[[ $(uname -s) == Linux ]] || { echo 'This installer supports Linux only.' >&2; exit 1; }
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
root_dir=$(cd -- "$script_dir/.." && pwd)
if (( EUID == 0 )); then
  prefix=/usr/local
  config_dir=/etc/dagger-rs
else
  prefix=${HOME:?}/.local
  config_dir=${XDG_CONFIG_HOME:-$HOME/.config}/dagger-rs
fi
binary=''
while (( $# )); do
  case $1 in
    --prefix) prefix=${2:?Missing --prefix value}; shift 2 ;;
    --config-dir) config_dir=${2:?Missing --config-dir value}; shift 2 ;;
    --binary) binary=${2:?Missing --binary value}; shift 2 ;;
    -h|--help)
      echo 'Usage: bash scripts/install.sh [--binary ./dagger-rs] [--prefix /usr/local] [--config-dir /etc/dagger-rs]'
      echo 'Uses a local release binary; does not build, download, overwrite configs, or start services.'
      exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done
validate_path() {
  [[ $1 == /* && $1 != *[[:cntrl:]]* && $1 != *'"'* && $1 != *'%'* && $1 != *'$'* && $1 != *'\'* ]] || {
    echo 'Installation paths must be absolute and contain no control characters, quote, %, $, or backslash.' >&2
    exit 2
  }
}
validate_path "$prefix"
validate_path "$config_dir"
command -v python3 >/dev/null || { echo 'Python 3 is required by local setup.' >&2; exit 1; }
config_dir=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$config_dir")
validate_path "$config_dir"
case $config_dir in
  /|/etc|/usr|/usr/local|/var|/home|/root|/opt|/tmp|"${HOME:-/}")
    echo 'Choose a dedicated application subdirectory for --config-dir.' >&2; exit 2 ;;
esac
if [[ -z $binary ]]; then
  for candidate in "$root_dir/bin/dagger-rs" "$root_dir/target/release/dagger-rs"; do
    if [[ -x $candidate ]]; then binary=$candidate; break; fi
  done
fi
[[ -n $binary && -f $binary && -x $binary ]] || {
  echo 'No local Linux release binary found. Supply --binary, or build this source checkout with cargo build --release --locked first.' >&2
  exit 1
}
"$binary" --version
install -d -m 755 "$prefix/bin" "$prefix/share/dagger-rs"
install -d -m 700 "$config_dir"
install -m 755 "$binary" "$prefix/bin/dagger-rs"
install -m 755 "$script_dir/setup.sh" "$script_dir/setup-classic.sh" "$script_dir/install-release.sh" "$prefix/share/dagger-rs/"
wrapper=''
trap '[[ -z $wrapper ]] || rm -f -- "$wrapper"' EXIT
for menu in setup setup-classic; do
  wrapper=$(mktemp "$prefix/bin/.dagger-$menu.XXXXXXXX")
  {
    echo '#!/usr/bin/env bash'
    printf 'export DAGGER_BIN=%q\n' "$prefix/bin/dagger-rs"
    printf 'export DAGGER_CONFIG_DIR=%q\n' "$config_dir"
    printf 'export DAGGER_PREFIX=%q\n' "$prefix"
    printf 'exec bash %q "$@"\n' "$prefix/share/dagger-rs/$menu.sh"
  } > "$wrapper"
  chmod 755 "$wrapper"
  mv -fT -- "$wrapper" "$prefix/bin/dagger-$menu"
  wrapper=''
done
printf '\nInstalled locally. Open setup with:\n  %q\nConfigs and keys: %s\n' "$prefix/bin/dagger-setup" "$config_dir"

# Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
# Attribution copy: Dagger Rust rewrite by i​r⁠_spoof; https://t.me/ir_spoof
