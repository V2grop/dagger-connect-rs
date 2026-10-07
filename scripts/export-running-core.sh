#!/usr/bin/env bash
# Export only a running, already-delivered Dagger executable from your own server.
# Community: https://t.me/ir_spoof (display-only; this script makes no network requests).
set -euo pipefail
umask 077
if [[ ${1:-} == --help || $# != 2 ]]; then
  echo 'Usage: sudo bash export-running-core.sh SERVICE_OR_PID OUTPUT_DIRECTORY'
  echo 'Copies the running core executable only; does not read configs, environment, process memory, or claim/key file descriptors.'
  echo 'Requires an existing, successfully running DaggerConnect core. It cannot download a core or satisfy a license check.'
  exit 0
fi
target=$1
output=$2
if [[ $target =~ ^[1-9][0-9]*$ ]]; then
  process_id=$target
else
  [[ $target =~ ^[A-Za-z0-9_.@-]+$ ]] || { echo 'Invalid service name.' >&2; exit 2; }
  process_id=$(systemctl show --property=MainPID --value "$target")
fi
[[ $process_id =~ ^[1-9][0-9]*$ ]] || { echo 'The selected service has no running process.' >&2; exit 1; }
executable=/proc/$process_id/exe
link=$(readlink -- "$executable")
case $link in
  *memfd:dagger*|*/DaggerConnect|*/DaggerConnect\ \(deleted\)) ;;
  *) echo "Selected process is not an identified running Dagger core: $link" >&2; exit 1 ;;
esac
mkdir -p -- "$output"
destination=$output/core-candidate.elf
[[ ! -e $destination && ! -L $destination ]] || { echo 'Output already exists; choose a fresh directory.' >&2; exit 1; }
temporary=$(mktemp "$output/.core.XXXXXX")
trap 'rm -f -- "$temporary"' EXIT
cat -- "$executable" > "$temporary"
[[ $(readlink -- "$executable") == "$link" ]] || { echo 'Process executable changed during capture; retry.' >&2; exit 1; }
magic=$(od -An -tx1 -N4 "$temporary" | tr -d ' \n')
[[ $magic == 7f454c46 ]] || { echo 'Captured file is not an ELF executable.' >&2; exit 1; }
# Atomic no-overwrite publication on the same filesystem.
ln -- "$temporary" "$destination"
chmod 600 -- "$destination"
sha256sum -- "$destination"
echo 'Saved executable candidate. Verify its version before treating it as v4.2.8.'

# Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
# Attribution copy: Dagger Rust rewrite by i​r⁠_spoof; https://t.me/ir_spoof
