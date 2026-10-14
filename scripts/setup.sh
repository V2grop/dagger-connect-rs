#!/usr/bin/env bash
# Interactive local configuration and Linux service management. No web calls.
set -euo pipefail
umask 077
[[ $(uname -s) == Linux ]] || { echo 'Setup supports Linux only.' >&2; exit 1; }
command -v python3 >/dev/null || { echo 'Python 3 is required for local service configuration inspection.' >&2; exit 1; }
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
binary=${DAGGER_BIN:-}
if [[ -z $binary ]]; then
  for candidate in "$script_dir/../bin/dagger-rs" "$script_dir/../target/release/dagger-rs"; do
    if [[ -x $candidate ]]; then binary=$candidate; break; fi
  done
fi
if [[ -z $binary ]]; then binary=$(command -v dagger-rs || true); fi
[[ -n $binary && -x $binary ]] || { echo 'Install or build the local dagger-rs binary first.' >&2; exit 1; }
binary=$(realpath -- "$binary")
if (( EUID == 0 )); then default_dir=/etc/dagger-rs; else default_dir=${XDG_CONFIG_HOME:-${HOME:?}/.config}/dagger-rs; fi
config_dir=${DAGGER_CONFIG_DIR:-$default_dir}
config_dir=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$config_dir")
case $config_dir in
  /|/etc|/usr|/usr/local|/var|/home|/root|/opt|/tmp|"${HOME:-/}")
    echo 'Choose a dedicated application configuration subdirectory.' >&2; exit 2 ;;
esac
mkdir -p -- "$config_dir"
config_dir=$(realpath -- "$config_dir")
for path in "$binary" "$config_dir"; do
  [[ ! $path =~ [[:cntrl:]] && $path != *'"'* && $path != *'%'* && $path != *'$'* && $path != *'\'* ]] || { echo 'Paths cannot contain control characters, double quote, %, $, or backslash.' >&2; exit 2; }
done

ask() {
  local label=$1 fallback=${2:-}
  printf '%s%s: ' "$label" "${fallback:+ [$fallback]}"
  if ! read -r REPLY; then echo 'Input closed; setup stopped.' >&2; return 1; fi
  REPLY=${REPLY:-$fallback}
}
# Do not put a shell function/subshell on the left of || or inside if: Bash
# disables errexit throughout its body, even after `set -e` inside that body.
run_action() {
  local status
  set +e
  ( set -e; "$@" )
  status=$?
  set -e
  case $status in
    0) ;;
    10) echo 'Setup cancelled; no service was started.' ;;
    *) printf 'Action failed (exit %s). Read the error above; setup did not complete.\n' "$status" ;;
  esac
}
yes_no() {
  local label=$1 fallback=${2:-no}
  while true; do
    ask "$label (yes/no, 1=yes, 2=no)" "$fallback"
    case ${REPLY,,} in
      q) return 10 ;;
      yes|y|1) REPLY=yes; return 0 ;;
      no|n|2) REPLY=no; return 0 ;;
      *) echo 'Enter yes or no (or 1/2).' ;;
    esac
  done
}
endpoint_pattern='^(\[[0-9a-fA-F:]+\]|[A-Za-z0-9_.-]+):[0-9]+$'
role_prompt() {
  if [[ -z ${1:-} && ${DAGGER_MENU_STYLE:-} == classic ]]; then guide_role; return; fi
  if [[ -n ${1:-} ]]; then role=$1
  else ask 'Role (server=Iran/entry, client=abroad/target)' server || return 1; role=$REPLY; fi
  [[ $role == server || $role == client ]] || { echo 'Choose server or client.'; return 1; }
}
key_prompt() {
  echo 'On the other machine, open option 1/2 to create its key. Copy its PUBLIC key here.'
  echo 'If that key is not ready, press Enter to keep your local key and return to the menu.'
  while true; do
    ask 'Remote public key (64 hex characters; empty=return)'
    peer_key=${REPLY,,}
    [[ -n $peer_key ]] || return 10
    if [[ $peer_key =~ ^[0-9a-f]{64}$ && ! $peer_key =~ ^0+$ ]]; then
      local own_key
      own_key=$(tr -d '\r\n' < "$config_dir/$role-keys/public.key")
      if [[ $peer_key != "${own_key,,}" ]]; then return 0; fi
      echo "That is this machine's key. Enter the OTHER machine's public key."
    else echo 'A valid remote public key is required.'; fi
  done
}
address_prompt() {
  while true; do
    ask "$1" "$2" || return 1; address=$REPLY
    # Restrict JSON interpolation to ordinary host:port/address syntax.
    if [[ $address =~ $endpoint_pattern ]]; then
      local port=${address##*:}
      if (( ${#port} <= 5 )) && (( 10#$port >= 1 && 10#$port <= 65535 )); then return 0; fi
    fi
    echo 'Required: host:port or [IPv6]:port, port 1..65535.'
  done
}
ensure_keys() {
  local directory=$config_dir/$role-keys
  if [[ -f $directory/private.key ]]; then
    echo "Using existing $role keypair."
  else
    "$binary" keygen --out "$directory"
  fi
  [[ -s $directory/private.key && -s $directory/public.key ]] || { echo 'Incomplete keypair; restore the matching private.key and public.key.'; return 1; }
  printf 'Local %s public key: ' "$role"; cat -- "$directory/public.key"; echo
}
key_menu() {
  role_prompt
  ensure_keys
  if [[ ${DAGGER_MENU_STYLE:-} == classic ]]; then
    if [[ $role == server ]]; then echo 'این کلید عمومی ایران است؛ در گزینهٔ 2 سرور خارج وارد کنید.'
    else echo 'این کلید عمومی خارج است؛ در گزینهٔ 1 سرور ایران وارد کنید.'; fi
    echo 'فقط رشتهٔ Public key را کپی کنید؛ کلید خصوصی روی همین سرور می‌ماند.'
  fi
}
certificate_menu() {
  role_prompt || return
  local directory=$config_dir/$role-tls
  ask 'Certificate DNS name or IP presented by this endpoint' localhost
  local name=$REPLY
  "$binary" certgen --out "$directory" --name "$name"
  echo "Endpoint certificate: $directory/cert.pem"
  echo 'Copy cert.pem to the physical dialing peer for trust. Keep key.pem on this endpoint.'
}
transport_prompt() {
  reverse=false
  echo 'Choose a tunnel carrier:'
  if [[ ${DAGGER_MENU_STYLE:-} == classic ]]; then
    local -a carriers=(tcp ws wss http https quantum quantum+ tun xhttp xhttps dc6 quantum-gaming kcp)
    local i
    for i in "${!carriers[@]}"; do printf '   %2d) %s\n' "$((i+1))" "${carriers[$i]}"; done
    ask 'Transport (number or name)' 1
    case $REPLY in
      1) REPLY=tcp ;; 2) REPLY=ws ;; 3) REPLY=wss ;; 4) REPLY=http ;;
      5) REPLY=https ;; 6) REPLY=quantum ;; 7) REPLY=quantum+ ;; 8) REPLY=tun ;;
      9) REPLY=xhttp ;; 10) REPLY=xhttps ;; 11) REPLY=dc6 ;;
      12) REPLY=quantum-gaming ;; 13) REPLY=kcp ;;
    esac
  else
    ask 'Transport (tcp/http/https/ws/wss/xhttp/xhttps/dc6/kcp/quantum/quantum+/quantum-gaming/tun)' tcp
  fi
  transport=$REPLY
  case $transport in tcp|http|https|ws|wss|xhttp|xhttps|dc6|kcp|quantum|quantum+|quantum-gaming|tun) ;; *) echo 'Choose a listed transport.'; return 1 ;; esac
  extra=",\"transport\":\"$transport\""
  case $transport in
    http|https|ws|wss|xhttp|xhttps)
      ask 'HTTP path' /tunnel; local route=$REPLY
      [[ $route =~ ^/[A-Za-z0-9_./~-]*$ ]] || { echo 'Use a plain absolute URL path.'; return 1; }
      extra+=",\"http_path\":\"$route\"" ;;
  esac
  case $transport in
    xhttp|xhttps)
      ask 'Reverse connection roles for CDN? (yes/no)' no
      if [[ $REPLY == yes ]]; then reverse=true; elif [[ $REPLY != no ]]; then echo 'Choose yes or no.'; return 1; fi
      local default_upload=stream-up authority=''
      if [[ $reverse == true ]]; then
        default_upload=packet-up
        if [[ $role == server ]]; then
          echo 'This logical server will dial the HTTP edge. Its port mappings and SOCKS listener remain here.'
        else
          echo 'This logical client will listen as the HTTP origin. Its local target allowlist remains here.'
        fi
      fi
      ask 'XHTTP upload mode (stream-up/packet-up/auto)' "$default_upload"; local upload=$REPLY
      case $upload in stream-up|packet-up|auto) ;; *) echo 'Choose a listed upload mode.'; return 1 ;; esac
      if [[ $role == client && $reverse == false || $role == server && $reverse == true ]]; then
        ask 'HTTP Host / TLS SNI override (empty uses peer address)' ''; authority=$REPLY
        local authority_pattern='^(\[[0-9a-fA-F:]+\]|[A-Za-z0-9.-]+)(:[0-9]+)?$'
        [[ -z $authority || $authority =~ $authority_pattern ]] || { echo 'Use an HTTP host name or IP, optionally with port.'; return 1; }
        if [[ $reverse == true && -z $authority ]]; then echo 'The HTTP edge requires the domain that routes to your origin.'; return 1; fi
      fi
      extra+=",\"xhttp\":{\"mode\":\"$upload\""
      if [[ -n $authority ]]; then extra+=",\"host\":\"$authority\""; fi
      extra+='}' ;;
    quantum|quantum-gaming|tun)
      raw_prompt ;;
  esac
  local tls_role=$role
  if [[ $reverse == true ]]; then
    if [[ $role == server ]]; then tls_role=client; else tls_role=server; fi
  fi
  case $transport in
    https|wss|xhttps)
      mkdir -p -- "$config_dir/$role-tls"
      if [[ $tls_role == server ]]; then
        ask 'Existing server certificate PEM file'; local certificate=$REPLY
        ask 'Existing server private key PEM file'; local tls_key=$REPLY
        [[ -f $certificate && -f $tls_key ]] || { echo 'Both local PEM files are required.'; return 1; }
        if [[ $(realpath -- "$certificate") != "$config_dir/$role-tls/cert.pem" ]]; then cp -- "$certificate" "$config_dir/$role-tls/cert.pem"; fi
        if [[ $(realpath -- "$tls_key") != "$config_dir/$role-tls/key.pem" ]]; then cp -- "$tls_key" "$config_dir/$role-tls/key.pem"; fi
        chmod 600 "$config_dir/$role-tls/key.pem"
        extra+=",\"cert_file\":\"$role-tls/cert.pem\",\"key_file\":\"$role-tls/key.pem\""
      else
        local name
        if [[ $transport == xhttps && -n ${authority:-} ]]; then
          name=$authority
          if [[ $name == \[* ]]; then name=${name#\[}; name=${name%%\]*}
          else name=${name%%:*}; fi
          printf 'TLS will verify the HTTP Host domain/IP: %s\n' "$name"
        else ask 'TLS server name matching certificate (DNS name or IP)'; name=$REPLY; fi
        [[ $name =~ ^[A-Za-z0-9_.:-]+$ ]] || { echo 'Invalid TLS server name.'; return 1; }
        ask 'Local trusted server certificate/CA PEM file'; local certificate=$REPLY
        [[ -f $certificate ]] || { echo 'A local trust certificate is required.'; return 1; }
        if [[ $(realpath -- "$certificate") != "$config_dir/$role-tls/ca.pem" ]]; then cp -- "$certificate" "$config_dir/$role-tls/ca.pem"; fi
        extra+=",\"server_name\":\"$name\",\"ca_file\":\"$role-tls/ca.pem\""
      fi ;;
  esac
}
raw_prompt() {
  raw_peer=''; raw_local_ip=''; raw_port=''
  ask 'Raw packet profile (tcp/udp/icmp/gre/ipip/bip/raw)' tcp; local profile=$REPLY
  case $profile in tcp|udp|icmp|gre|ipip|bip|raw) ;; *) echo 'Choose a listed packet profile.'; return 1 ;; esac
  local peer_role=server
  [[ $role != server ]] || peer_role=client
  ask "Real remote $peer_role IPv4 address"; local peer=$REPLY
  ask 'Local IPv4 address (empty detects from route)' ''; local local_ip=$REPLY
  ask 'Ethernet interface (empty detects from route)' ''; local interface=$REPLY
  ask 'Outer port (both peers use the same port)' 443; local port=$REPLY
  [[ $interface =~ ^[A-Za-z0-9_.-]{0,15}$ && $port =~ ^[0-9]+$ ]] || { echo 'Invalid raw interface or port.'; return 1; }
  if ! python3 - "$peer" "$local_ip" "$port" <<'PY'
import ipaddress,sys
if not sys.argv[1]: raise SystemExit("The real remote peer IPv4 address is required")
for value in sys.argv[1:3]:
    if not value: continue
    ip=ipaddress.IPv4Address(value)
    if ip.is_unspecified or ip.is_multicast or int(ip)==0xffffffff:
        raise SystemExit("Raw endpoints require unicast IPv4 addresses")
if not 1 <= int(sys.argv[3]) <= 65535: raise SystemExit("Outer port must be 1..65535")
PY
  then echo 'Use valid unicast IPv4 addresses and an outer port from 1 to 65535.'; return 1; fi
  port=$((10#$port))
  ask 'IP mode (normal/spoof/dcpi)' normal; local ip_mode=$REPLY src='' dst=''
  case $ip_mode in
    normal|dcpi) ;;
    spoof)
      ask 'Spoofed source IPv4 (empty keeps real address)' ''; src=$REPLY
      ask 'Spoofed destination IPv4 (empty keeps real peer)' ''; dst=$REPLY
      [[ $src =~ ^[0-9.]*$ && $dst =~ ^[0-9.]*$ ]] || { echo 'Use IPv4 addresses.'; return 1; } ;;
    *) echo 'Choose normal, spoof or dcpi.'; return 1 ;;
  esac
  local raw
  raw=$(python3 - "$profile" "$peer" "$local_ip" "$interface" "$port" "$ip_mode" "$src" "$dst" <<'PY'
import json,sys
profile,peer,local,interface,port,mode,src,dst=sys.argv[1:]
r={"profile":profile,"peer_ip":peer,"l4_port":int(port),"dcpi_mode":mode=="dcpi"}
for k,v in [("local_ip",local),("interface",interface),("spoof_src_ip",src),("spoof_dst_ip",dst)]:
    if v:r[k]=v
print(json.dumps(r,separators=(",",":")))
PY
  )
  extra+=",\"raw\":$raw"
  raw_peer=$peer; raw_local_ip=$local_ip; raw_port=$port
  echo 'The raw interface and peer determine packet routing. The configuration address below is derived from those settings.'
}
carrier_address_prompt() {
  case $transport in
    quantum|quantum-gaming|tun)
      if [[ $role == server ]]; then
        local metadata_ip=$raw_local_ip
        [[ -n $metadata_ip ]] || metadata_ip=0.0.0.0
        address=$metadata_ip:$raw_port
      else address=$raw_peer:$raw_port; fi
      printf 'Raw %s address metadata: %s\n' "$role" "$address"
      return 0 ;;
  esac
  if [[ $reverse == true ]]; then
    if [[ $role == server ]]; then address_prompt 'HTTP edge dial address (required)' ''
    else address_prompt 'HTTP origin listen address' '0.0.0.0:7000'; fi
  elif [[ $role == server ]]; then
    local fallback='0.0.0.0:7000'
    [[ $transport != dc6 ]] || fallback='[::]:7000'
    address_prompt 'Tunnel listen address' "$fallback"
  else
    echo 'Enter the real server IP/domain and tunnel port, for example IRAN_IP:7000.'
    address_prompt 'Remote server dial address (required)' ''
  fi
}
configure() {
  local guided=${2:-advanced}
  role_prompt "${1:-}"
  if [[ $guided == guided ]]; then
    echo "راهنما: Enter = مقدار داخل کروشه؛ q = لغو. فقط همین سرور تغییر می‌کند."
    guide_keys
  else
    ensure_keys
    echo 'Exchange only public keys with the other machine. Private keys remain on their own machine.'
    key_prompt
  fi
  local file=$config_dir/$role.json temporary cleanup
  if [[ -e $file ]]; then
    yes_no "جایگزینی تنظیم قبلی همین سرور؟ / Replace existing $file?" no
    [[ $REPLY == yes ]] || return 10
  fi
  if [[ $guided == guided ]]; then guide_transport; else transport_prompt; fi
  temporary=$(mktemp "$config_dir/.config.XXXXXX")
  printf -v cleanup 'rm -f -- %q' "$temporary"
  trap "$cleanup" EXIT
  local forwarding default_forwarding=ports
  [[ $transport != tun ]] || default_forwarding=tun
  if [[ $guided == guided ]]; then forwarding=ports
  else
  ask 'Forwarding mode (ports/tun/tun+ports)' "$default_forwarding"; forwarding=$REPLY
  case $forwarding in ports|tun|tun+ports) ;; *) echo 'Choose ports, tun, or tun+ports.'; rm -f -- "$temporary"; return 1 ;; esac
  if [[ $transport == tun && $forwarding == ports ]]; then
    echo 'The raw tun carrier requires tun or tun+ports forwarding.'
    rm -f -- "$temporary"; return 1
  fi
  fi
  local tun_name='' tun_ip='' tun_peer='' mtu='' forward_port=''
  local kind='' bind='' target='' pool=1 carrier=''
  if [[ $forwarding != ports ]]; then
    ask 'TUN interface name' dagger0; tun_name=$REPLY
    [[ $tun_name =~ ^[A-Za-z0-9_-]{1,15}$ ]] || { echo 'Invalid interface name.'; rm -f -- "$temporary"; return 1; }
    local default_ip=10.77.0.1 default_peer=10.77.0.2
    if [[ $role == client ]]; then default_ip=10.77.0.2; default_peer=10.77.0.1; fi
    ask 'Local tunnel IP (no prefix)' "$default_ip"; tun_ip=$REPLY
    ask 'Remote tunnel IP (no prefix)' "$default_peer"; tun_peer=$REPLY
    [[ $tun_ip =~ ^[0-9a-fA-F.:]+$ && $tun_peer =~ ^[0-9a-fA-F.:]+$ ]] || { echo 'Use literal IP addresses.'; rm -f -- "$temporary"; return 1; }
    ask 'TUN MTU' 1400; mtu=$REPLY
    [[ $mtu =~ ^[0-9]+$ ]] || { echo 'MTU must be numeric.'; rm -f -- "$temporary"; return 1; }
    if [[ $forwarding == tun+ports ]]; then
      ask 'Shared inner TUN forwarding port (both peers use the same port)' 47475; forward_port=$REPLY
      [[ $forward_port =~ ^[0-9]{1,5}$ ]] || { echo 'Forwarding port must be 1024..65535.'; rm -f -- "$temporary"; return 1; }
      forward_port=$((10#$forward_port))
      (( forward_port >= 1024 && forward_port <= 65535 )) || { echo 'Forwarding port must be 1024..65535.'; rm -f -- "$temporary"; return 1; }
      echo 'Mappings and SOCKS use authenticated TCP over the TUN peer route. Both peers must configure tun+ports with the same inner port.'
    fi
    echo 'TUN manages only its local interface and peer route. Additional forwarding/NAT must be configured explicitly.'
  fi
  if [[ $guided == guided ]]; then guide_carrier; else carrier_address_prompt; fi
  carrier=$address
  if [[ $guided == guided ]]; then
    guide_mapping
  elif [[ $forwarding != tun && $role == server ]]; then
    if [[ $reverse == true && $forwarding == ports ]]; then
      ask 'Outbound edge connection pool size' 1; pool=$REPLY
      [[ $pool =~ ^[0-9]+$ ]] || { echo 'Pool size must be a number.'; rm -f -- "$temporary"; return 1; }
      pool=$((10#$pool))
    fi
    ask 'Forwarding type (tcp/udp/socks5)' tcp; kind=$REPLY
    case $kind in
      tcp|udp)
        echo '0.0.0.0 exposes this forwarded port on the server; 127.0.0.1 makes it local-only.'
        address_prompt 'Forwarded bind address' '0.0.0.0:18080'; bind=$address
        echo 'Use the actual service port on the client, e.g. 127.0.0.1:443. The tunnel does not create that service.'
        address_prompt 'Target address on client machine (required)' ''; target=$address
        ;;
      socks5)
        address_prompt 'SOCKS5 listen address (no password; keep loopback unless intentionally exposing)' '127.0.0.1:1080' || { rm -f -- "$temporary"; return 1; }
        bind=$address
        ;;
      *) echo 'Choose tcp, udp, or socks5.'; rm -f -- "$temporary"; return 1 ;;
    esac
  elif [[ $forwarding != tun ]]; then
    echo 'This must match the server target exactly. Use * only for unrestricted SOCKS destinations.'
    ask 'Allowed local target host:port (required)' ''; target=$REPLY
    [[ $target == '*' || $target =~ $endpoint_pattern ]] || { echo 'Invalid target.'; rm -f -- "$temporary"; return 1; }
    if [[ $forwarding == tun+ports ]]; then echo 'One packet tunnel carries the authenticated inner forwarding connection.'
    elif [[ $reverse == true ]]; then echo 'One origin listener accepts the dialing server connection pool.'
    elif [[ $transport == quantum || $transport == quantum-gaming ]]; then echo 'One raw connection is used for this configured peer.'
    else ask 'Connection pool size' 1; pool=$REPLY; fi
    [[ $pool =~ ^[0-9]+$ ]] || { echo 'Pool size must be a number.'; rm -f -- "$temporary"; return 1; }
    pool=$((10#$pool))
  fi
  if [[ $guided == guided ]]; then guide_summary; fi
  python3 - "$temporary" "$role" "$peer_key" "$carrier" "$extra" "$reverse" "$forwarding" \
    "$tun_name" "$tun_ip" "$tun_peer" "$mtu" "$forward_port" "$kind" "$bind" "$target" "$pool" <<'PY'
import json,sys
p,role,peer,carrier,extra,reverse,forwarding,name,address,remote,mtu,port,kind,bind,target,pool=sys.argv[1:]
c={"_comment":"Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof; credit: i\u200br\u2060_spoof","mode":role,"private_key_file":role+"-keys/private.key","reverse":reverse=="true"}
entry=json.loads("{"+extra.lstrip(",")+"}")
entry["addr"]=carrier
if forwarding!="ports":
    c["tun"]={"name":name,"address":address,"peer_address":remote,"mtu":int(mtu)}
    if port:c["tun"]["forwarding_port"]=int(port)
if role=="server":
    c["peer_public_keys"]=[peer]
    entry["maps"]=[]
    if reverse=="true":entry["connection_pool"]=int(pool)
    if kind in ("tcp","udp"):entry["maps"]=[{"type":kind,"bind":bind,"target":target}]
    elif kind=="socks5":c["socks5"]=bind
    c["listeners"]=[entry]
else:
    if forwarding!="tun":c["allowed_targets"]=[target]
    entry.update(server_public_key=peer,connection_pool=int(pool))
    c["paths"]=[entry]
with open(p,"w") as f:
    json.dump(c,f,indent=2)
    f.write("\n")
PY
  if "$binary" -c "$temporary" --check; then
    [[ ! -e $file ]] || cp -p -- "$file" "$file.backup.$(date +%s)"
    mv -- "$temporary" "$file"
    if (( EUID == 0 )) && id dagger-rs >/dev/null 2>&1; then
      chown dagger-rs:dagger-rs -- "$file"
      for directory in "$config_dir/$role-keys" "$config_dir/$role-tls"; do
        if [[ -d $directory ]]; then
          chown dagger-rs:dagger-rs -- "$directory"
          for item in "$directory/private.key" "$directory/public.key" "$directory/cert.pem" "$directory/key.pem" "$directory/ca.pem"; do
            [[ ! -f $item ]] || chown dagger-rs:dagger-rs -- "$item"
          done
        fi
      done
    fi
    echo "Saved $file. Edit JSON to add more mappings, listeners, or paths."
    trap - EXIT
    if [[ $guided == guided ]]; then guide_finish
    else python3 "$script_dir/diagnose.py" "$file" --summary-only; fi
  else
    rm -f -- "$temporary"
    echo 'Configuration was rejected; existing config preserved.'
    return 1
  fi
}
systemctl_cmd() { if (( EUID == 0 )); then systemctl "$@"; else systemctl --user "$@"; fi; }
service_unit_dir() {
  if (( EUID == 0 )); then echo /etc/systemd/system
  else echo "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"; fi
}
install_service() {
  command -v systemctl >/dev/null || { echo 'systemd is not available. Use foreground mode in the advanced menu.'; return 1; }
  role_prompt "${1:-}"
  local file=$config_dir/$role.json
  "$binary" -c "$file" --check || return
  local units user_line capabilities='' families='AF_INET AF_INET6 AF_UNIX' has_tun has_raw has_privileged caps=''
  units=$(service_unit_dir)
  has_tun=$(python3 -c 'import json,sys; print(int(json.load(open(sys.argv[1])).get("tun") is not None))' "$file")
  has_raw=$(python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); print(int(any(e.get("raw") is not None for e in c.get("listeners",[])+c.get("paths",[]))))' "$file")
  has_privileged=$(python3 - "$file" <<'PY'
import json,sys
c=json.load(open(sys.argv[1]))
binds=[]
if c["mode"]=="server":
    binds += [m["bind"] for entry in c.get("listeners",[]) for m in entry.get("maps",[])]
    if c.get("socks5"): binds.append(c["socks5"])
    if not c.get("reverse",False):
        binds += [entry["addr"] for entry in c.get("listeners",[]) if entry.get("raw") is None]
elif c.get("reverse",False):
    binds += [entry["addr"] for entry in c.get("paths",[])]
print(int(any(0 < int(bind.rsplit(":",1)[1]) < 1024 for bind in binds)))
PY
  )
  if [[ $has_tun == 1 ]]; then
    (( EUID == 0 )) || { echo 'Install TUN system services as root so CAP_NET_ADMIN can be granted.'; return 1; }
    caps='CAP_NET_ADMIN'
    families+=' AF_NETLINK'
  fi
  if [[ $has_raw == 1 ]]; then
    (( EUID == 0 )) || { echo 'Install raw packet services as root to grant CAP_NET_RAW.'; return 1; }
    caps+="${caps:+ }CAP_NET_RAW"
    families+=' AF_PACKET'
    [[ $has_tun == 1 ]] || families+=' AF_NETLINK'
  fi
  if [[ $has_privileged == 1 ]]; then
    (( EUID == 0 )) || { echo 'Install services with listening ports below 1024 as root.'; return 1; }
    [[ -z $caps ]] || caps+=' '
    caps+='CAP_NET_BIND_SERVICE'
  fi
  if [[ -n $caps ]]; then printf -v capabilities 'AmbientCapabilities=%s\nCapabilityBoundingSet=%s' "$caps" "$caps"; fi
  if (( EUID == 0 )); then
    if ! id dagger-rs >/dev/null 2>&1; then useradd --system --no-create-home --home-dir "$config_dir" --shell /usr/sbin/nologin dagger-rs; fi
    chown dagger-rs:dagger-rs -- "$config_dir" "$file"
    if [[ -d $config_dir/$role-keys ]]; then
      chown dagger-rs:dagger-rs -- "$config_dir/$role-keys"
      for key in private.key public.key; do
        [[ ! -f $config_dir/$role-keys/$key ]] || chown dagger-rs:dagger-rs -- "$config_dir/$role-keys/$key"
      done
    fi
    if [[ -d $config_dir/$role-tls ]]; then
      chown dagger-rs:dagger-rs -- "$config_dir/$role-tls"
      for pem in cert.pem key.pem ca.pem; do
        [[ ! -f $config_dir/$role-tls/$pem ]] || chown dagger-rs:dagger-rs -- "$config_dir/$role-tls/$pem"
      done
    fi
    user_line='User=dagger-rs'
    runuser -u dagger-rs -- "$binary" -c "$file" --check || { echo 'Service user cannot read a required config/key/certificate file.'; return 1; }
  else
    user_line=''
  fi
  mkdir -p -- "$units"
  cat > "$units/dagger-rs-$role.service" <<UNIT
[Unit]
Description=Independent Rust reverse tunnel ($role)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
$user_line
ExecStart="$binary" --config "$file"
Environment=RUST_LOG=info,dagger_rs=debug
Restart=on-failure
RestartSec=3
TimeoutStopSec=10
UMask=0077
NoNewPrivileges=true
$capabilities
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
RestrictAddressFamilies=$families

[Install]
WantedBy=$(if (( EUID == 0 )); then echo multi-user.target; else echo default.target; fi)
UNIT
  chmod 644 "$units/dagger-rs-$role.service"
  systemctl_cmd daemon-reload
  systemctl_cmd enable "dagger-rs-$role.service"
  # enable --now does not reload an already running process after config edits.
  systemctl_cmd restart "dagger-rs-$role.service"
  sleep 1
  if ! systemctl_cmd is-active --quiet "dagger-rs-$role.service"; then
    echo 'The service failed to stay running. Recent errors:'
    service_action logs "$role"
    return 1
  fi
  echo "Service running: dagger-rs-$role.service (enabled at boot)."
  echo 'A running service alone does not prove the peer is connected or data is forwarding.'
  echo 'Use Diagnostics / Recent Logs and test your forwarded service from the entry side.'
  if (( EUID != 0 )); then echo 'User services stop at logout unless your administrator enables user lingering.'; fi
}
service_action() {
  local action=$1; role_prompt "${2:-}"
  if [[ $action == logs ]]; then
    if (( EUID == 0 )); then journalctl -u "dagger-rs-$role.service" -n 50 --no-pager; else journalctl --user -u "dagger-rs-$role.service" -n 50 --no-pager; fi
  else systemctl_cmd "$action" "dagger-rs-$role.service"; fi
}
diagnose_menu() {
  role_prompt
  local file=$config_dir/$role.json
  "$binary" -c "$file" --check
  python3 "$script_dir/diagnose.py" "$file"
  systemctl_cmd --no-pager status "dagger-rs-$role.service" || true
  service_action logs "$role"
}
linktest_menu() {
  ask 'Link test action (listen/probe)' probe; local action=$REPLY
  local -a args=("$binary" linktest "$action")
  case $action in
    listen)
      address_prompt 'Local diagnostic bind IP:port' 127.0.0.1:47000 || return
      args+=(--bind "$address")
      ask 'Expected remote peer IP (required)'; args+=(--peer "$REPLY")
      ask 'Listen duration in seconds' 900; args+=(--wait-secs "$REPLY") ;;
    probe)
      address_prompt 'Remote diagnostic peer IP:port (required)' '' || return
      args+=(--peer "$address")
      ask 'Local source IP (empty uses routing)' ''; [[ -z $REPLY ]] || args+=(--bind "$REPLY")
      ask 'Reverse listener port (0 selects a free port)' 0; args+=(--local-port "$REPLY")
      ask 'Throughput duration in seconds' 4; args+=(--seconds "$REPLY")
      ask 'Quick test? (yes/no)' yes
      if [[ $REPLY == yes ]]; then args+=(--quick); elif [[ $REPLY != no ]]; then echo 'Choose yes or no.'; return 1; fi ;;
    *) echo 'Choose listen or probe.'; return 1 ;;
  esac
  ask 'Additional diagnostic ports (comma-separated, empty for none)' ''
  [[ -z $REPLY ]] || args+=(--extra-ports "$REPLY")
  "${args[@]}"
}
source "$script_dir/setup-guided.sh"
if [[ ${BASH_SOURCE[0]} == "$0" && ${1:-} == --help ]]; then
  echo 'Usage: dagger-setup (interactive). Override DAGGER_BIN and DAGGER_CONFIG_DIR to use a local build or isolated directory.'
  exit 0
fi
if [[ ${BASH_SOURCE[0]} != "$0" ]]; then return 0; fi
while true; do
  printf '\nDagger Rust — local Linux setup\nConfig directory: %s\n' "$config_dir"
  printf 'Rust rewrite by ir_spoof: https://t.me/ir_spoof\n'
  printf '1) Generate/show local public key\n2) Configure server or client\n3) Validate configuration\n4) Run in foreground\n5) Install and start systemd service\n6) Service status\n7) Restart service\n8) Stop service\n9) Recent logs\n10) Generate local TLS certificate\n11) Test an explicit peer link\n0) Exit\n'
  if ! ask 'Choose' 0; then exit 0; fi
  case $REPLY in
    1) run_action key_menu ;;
    2) run_action configure ;;
    3) if role_prompt; then "$binary" -c "$config_dir/$role.json" --check || true; fi ;;
    4) if role_prompt; then "$binary" -c "$config_dir/$role.json" || true; fi ;;
    5) run_action install_service ;;
    6) run_action service_action status ;;
    7) run_action service_action restart ;;
    8) run_action service_action stop ;;
    9) run_action service_action logs ;;
    10) run_action certificate_menu ;;
    11) run_action linktest_menu ;;
    0) exit 0 ;;
    *) echo 'Choose a listed option.' ;;
  esac
done

# Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
# Attribution copy: Dagger Rust rewrite by i​r⁠_spoof; https://t.me/ir_spoof
