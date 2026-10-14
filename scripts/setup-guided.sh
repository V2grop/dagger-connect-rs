#!/usr/bin/env bash
# Sourced by setup.sh. Guided port forwarding; the Rust configuration is shared.
guide_ask() {
  ask "$@"
  [[ ${REPLY,,} != q ]] || return 10
}
guide_port() {
  local label=$1 fallback=${2:-}
  while true; do
    guide_ask "$label (number only, e.g. 2030)" "$fallback"
    REPLY=$(python3 -c 'import sys; print(sys.argv[1].translate(str.maketrans("۰۱۲۳۴۵۶۷۸۹٠١٢٣٤٥٦٧٨٩", "01234567890123456789")))' "$REPLY")
    if [[ $REPLY =~ ^[0-9]{1,5}$ ]] && (( 10#$REPLY >= 1 && 10#$REPLY <= 65535 )); then
      port=$((10#$REPLY)); return
    fi
    echo '[!] Invalid port. Enter 1..65535 only; no IP and no colon.'
  done
}
guide_host() {
  local label=$1 fallback=${2:-} purpose=${3:-target}
  while true; do
    guide_ask "$label (IP/domain only, no port)" "$fallback"
    if host=$(python3 - "$REPLY" "$purpose" <<'PY'
import ipaddress,re,sys
host,purpose=sys.argv[1:]
host=host.strip()
try:
    ip=ipaddress.ip_address(host.strip('[]'))
except ValueError:
    if (not re.fullmatch(r'(?=.{1,253}$)[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?',host)
        or host.isdigit() or re.fullmatch(r'[0-9.]+',host)
        or any(not label or len(label)>63 or label.startswith('-') or label.endswith('-') for label in host.split('.'))):
        raise SystemExit(1)
    if purpose=='iran' and (host.lower()=='localhost' or host.lower().endswith('.localhost')):
        raise SystemExit(1)
    print(host.lower())
else:
    if ip.is_unspecified or ip.is_multicast or (purpose=='iran' and (ip.is_loopback or ip.version!=4)):
        raise SystemExit(1)
    print(str(ip))
PY
    ); then return; fi
    echo '[!] Invalid host. Enter an IP/domain; no http:// and no port.'
    if [[ $purpose == iran ]]; then echo '[!] Use the reachable IRAN IPv4, not ABROAD IP, 127.0.0.1 or 0.0.0.0. IPv6 tunnel: use Advanced Setup.'; fi
  done
}
guide_endpoint() {
  address=$1:$2
  [[ $1 != *:* ]] || address="[$1]:$2"
}
guide_role() {
  while true; do
    echo '[HOST] Which server is THIS terminal connected to?'
    echo '1) [IRAN] Server     2) [ABROAD] Client     q) Back'
    guide_ask '[HOST] This Server (1=IRAN, 2=ABROAD)'
    case $REPLY in 1|server) role=server; return ;; 2|client) role=client; return ;; esac
    echo '[!] Enter 1 for IRAN or 2 for ABROAD.'
  done
}
guide_keys() {
  local here=IRAN there=ABROAD other_option=2
  if [[ $role == client ]]; then here=ABROAD; there=IRAN; other_option=1; fi
  printf '\n[HOST] THIS SERVER: %s (%s)\n' "$here" "$role"
  echo '[i] Client = ABROAD server. User = phone / VPN app.'
  ensure_keys
  printf '[KEY] %s Public Key above: copy it to the %s server.\n' "$here" "$there"
  printf '[KEY] Paste the OTHER server public key here: %s.\n' "$there"
  printf '[i] Get it on %s: menu 15, then role %s.\n' "$there" "$other_option"
  echo '[i] Key not ready? Empty Enter = Back. Your local key is kept.'
  while true; do
    guide_ask "[KEY] $there Public Key (OTHER machine, 64 hex)"
    peer_key=${REPLY,,}
    [[ -n $peer_key ]] || return 10
    if [[ $peer_key =~ ^[0-9a-f]{64}$ && ! $peer_key =~ ^0+$ ]]; then
      local own_key
      own_key=$(tr -d '\r\n' < "$config_dir/$role-keys/public.key")
      [[ $peer_key == "${own_key,,}" ]] || return 0
      printf '[!] This is your %s key. Enter the %s key. OTHER machine required.\n' "$here" "$there"
    else echo '[!] Paste only the 64-character PUBLIC key; no label, no private key.'; fi
  done
}
guide_transport() {
  reverse=false
  echo
  echo '[TUNNEL] Select the SAME transport on IRAN and ABROAD.'
  echo '1) TCP    2) Quantum+    3) KCP'
  echo '[i] Enter = TCP. More transports: menu 12 [Advanced].'
  while true; do
    guide_ask '[TUNNEL] Transport (BOTH servers)' 1
    case ${REPLY,,} in
      1|tcp) transport=tcp; break ;;
      2|quantum+) transport=quantum+; break ;;
      3|kcp) transport=kcp; break ;;
      *) echo '[!] Enter 1, 2 or 3.' ;;
    esac
  done
  extra=",\"transport\":\"$transport\""
}
guide_carrier() {
  local iran_host=0.0.0.0
  if [[ $role == client ]]; then
    echo '[IRAN] ABROAD connects to IRAN. Use IRAN public IPv4 or a domain pointing to it.'
    echo '[i] Format example: 203.0.113.10. Enter YOUR real IRAN IP.'
    guide_host '[IRAN] Server IP' '' iran
    iran_host=$host
  else
    echo '[IRAN] Tunnel Listen IP: 0.0.0.0 (automatic). ABROAD connects here.'
    echo '[i] 0.0.0.0 = all interfaces on THIS IRAN server; not the ABROAD IP.'
  fi
  echo '[TUNNEL] Server-to-server port; NOT the user port or VPN inbound port.'
  echo '[i] Use the SAME port on both servers. Enter = 7000.'
  guide_port '[IRAN] Tunnel Port' 7000
  guide_endpoint "$iran_host" "$port"
}
guide_target() {
  echo
  echo '[ABROAD] Target = your existing VPN inbound / service on ABROAD.'
  echo '[i] Use the VPN INBOUND port, not the panel login port. The service must already exist.'
  echo '[LOCAL] 127.0.0.1 here ALWAYS means ABROAD localhost, even in IRAN setup.'
  echo '[i] Enter = service on ABROAD itself. Docker/LAN: enter its IP reachable from ABROAD.'
  guide_host '[ABROAD] Service IP (Client Local IP)' 127.0.0.1 target
  local target_host=$host
  echo '[i] Example: ABROAD inbound uses port 2030 -> enter 2030. Required; no default.'
  guide_port '[ABROAD] Service Port (Client Inbound Port)'
  guide_endpoint "$target_host" "$port"
  target=$address
}
guide_mapping() {
  if [[ $role == server ]]; then
    echo
    echo '[ABROAD] Service Protocol: match your VPN inbound. 1=TCP, 2=UDP.'
    while true; do
      guide_ask '[PROTO] 1) TCP   2) UDP' 1
      case ${REPLY,,} in 1|tcp) kind=tcp; break ;; 2|udp) kind=udp; break ;; *) echo '[!] Enter 1 or 2.' ;; esac
    done
    echo '[USER] Phone / VPN app connects to IRAN public IP + this User Port.'
    echo '[IRAN] User Listen IP: 0.0.0.0 (all IRAN interfaces). Default port: 18080.'
    while true; do
      guide_port '[IRAN] User Port (VPN App Port)' 18080
      if [[ $port != "${carrier##*:}" || ( $transport == tcp && $kind == udp ) || ( $transport != tcp && $kind == tcp ) ]]; then break; fi
      echo '[!] User Port and Tunnel Port must differ when using the same protocol on IRAN.'
    done
    bind=0.0.0.0:$port
  else
    echo '[ABROAD] Allowed Target: use the SAME Service IP + Service Port entered in IRAN setup.'
  fi
  guide_target
}
guide_summary() {
  echo
  echo '========== [CHECK] REVIEW BEFORE SAVE =========='
  if [[ $role == server ]]; then
    echo '[HOST] THIS SERVER: IRAN'
    printf '[USER] IRAN User Listen: %s (%s)\n' "$bind" "$kind"
    printf '[TUNNEL] IRAN Listen: %s\n' "$carrier"
  else
    echo '[HOST] THIS SERVER: ABROAD'
    printf '[TUNNEL] ABROAD connects to IRAN: %s\n' "$carrier"
  fi
  printf '[TUNNEL] Transport (BOTH servers): %s\n' "$transport"
  printf '[TARGET] ABROAD Service (local to ABROAD, not IRAN): %s\n' "$target"
  echo '[i] ABROAD Service IP + Port must match in BOTH configurations.'
  echo '[i] 1=Save, 2=Cancel and keep old config, q=Back.'
  guide_ask '[SAVE] Save Config? (1=yes, 2=no)' 1
  case ${REPLY,,} in 1|yes|y) ;; *) return 10 ;; esac
}
guide_finish() {
  echo
  if [[ $role == server ]]; then
    printf '[NEXT] On ABROAD: menu 2, transport %s, IRAN Tunnel Port %s.\n' "$transport" "${carrier##*:}"
    printf '[USER] VPN app: IRAN public IP + port %s. Keep the ABROAD inbound credentials/settings.\n' "${bind##*:}"
  else
    echo '[NEXT] On IRAN: menu 1, paste this ABROAD public key.'
  fi
  printf '[TARGET] SAME ABROAD Service on both sides: %s\n' "$target"
  echo '[CHECK] Menu 13: diagnose tunnel / service.'
}
