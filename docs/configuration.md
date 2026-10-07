# Configuration reference

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

Strict JSON rejects unknown settings, including original vendor `psk` fields. Bind addresses are literal IP:port, with brackets for IPv6. Peer addresses/targets can use hostnames except `dc6`, which requires literal IPv6. Key and TLS file paths resolve relative to the config file. Key hex strings are exactly 64 characters; all-zero keys and placeholder text are rejected.

| Field | Role | Meaning/default |
| --- | --- | --- |
| `_comment` | Both | Optional non-operational comment metadata. Discarded when loading; supplied examples use it for project attribution because JSON has no comment syntax. |
| `mode` | Both | Required `server` or `client`. |
| `reverse` | Both | Default false. XHTTP/XHTTPS only: logical server dials, logical client listens. Must agree on both ends. |
| `private_key_file` | Both | Required local Noise private hex key; Unix mode must exclude group/other access. |
| `heartbeat_sec` | Both | Default 10; 1..3600 seconds. |
| `dead_timeout_sec` | Both | Default 40; greater than heartbeat, at most 86400. |
| `max_streams` | Both | Default 256; 1..65536. Per-session stream ceiling with additional forwarding admission limits. |
| `max_connections` | Both | Default 16; 1..1024. Total configured client pools must fit. |
| `peer_public_keys` | Server | Required nonempty authorized client-key list. |
| `listeners` | Server | Required nonempty list of tunnel listeners. |
| `listeners[].addr` | Server | Local carrier bind address. |
| `listeners[].connection_pool` | Server | Default 1; physical outgoing connections in reversed XHTTP mode. |
| `listeners[].maps[]` | Server | `type`: `tcp` or `udp`, local `bind`, client-side `target`. |
| `socks5` | Server | Optional local SOCKS5 TCP CONNECT bind. |
| `paths` | Client | Required nonempty server connection paths. |
| `paths[].addr` | Client | Server endpoint; TCP or UDP according to carrier. |
| `paths[].server_public_key` | Client | Required pinned server key. |
| `paths[].connection_pool` | Client | Default 1; 1..1024. |
| `paths[].retry_interval` | Client | Default 1; 1..30 seconds; reconnect delay capped at 30. |
| `paths[].dial_timeout` | Client | Default 10; 1..300 seconds, including connection/handshake. |
| `allowed_targets` | Client | Exact `host:port` strings. Empty denies all; `"*"` explicitly allows any target. |
| `tun` | Both | Optional layer-3 mode described below. |

## Carrier settings

Every listener and path can specify `transport`: `tcp` (default), `kcp`, `http`, `https`, `ws`, `wss`, `xhttp`, `xhttps`, `dc6`, `quantum+`, `quantum`, `quantum-gaming`, or `tun`. Both ends must match. KCP and Quantum+ use UDP; Quantum/Gaming/TUN use raw IPv4; the other carriers use TCP. `dc6` requires `[IPv6]:port` at both ends.

`http_path` defaults to `/tunnel` and must match on both ends for HTTP/WebSocket modes. It is an ASCII absolute path, at most 512 bytes, without whitespace, control characters, query, or fragment. XHTTP has a nested `xhttp` object with `mode` (`stream-up`, `packet-up`, `auto`), optional `host` and `edge_addrs`, upload body/concurrency bounds, buffering limits, streaming-probe timeouts, User-Agent and query padding. See [XHTTP settings](xhttp.md).

TLS listeners (`https`, `wss`, `xhttps`) require `cert_file` and `key_file`. TLS clients require `server_name` matching the certificate, and may set `ca_file` to a PEM trust certificate. Without `ca_file`, bundled public roots are used. TLS settings are rejected on non-TLS carriers. Noise peer keys remain required in addition to certificates. `certgen --out keys/tls --name tunnel.example.com` generates local PEM files without overwriting existing files; copy only the certificate to the opposite machine.

With `reverse: true`, the logical server's listener supplies the remote origin address, `ca_file` and `server_name`; the logical client's path supplies its local bind address, `cert_file` and `key_file`. An XHTTP `host` override selects the HTTP authority and TLS SNI hostname. The allowlisted client key and pinned server key still refer to logical tunnel roles.

Quantum transports accept a nested `quantum` object: `mtu` defaults to 1350 (512..9000), `profile` defaults to `default` with optional `gaming`, `knock` defaults to true, `knock_timeout_ms` defaults to 3000 (500..30000), and `max_peers` defaults to 256 (1..4096). Quantum+ uses an additional UDP knock port: data port + 10000, or data port - 1000 when the first value would overflow. Both ends must agree on knock. Raw Quantum/Gaming require `raw` settings and one connection; see [raw protocol](raw-protocol.md). The gaming transport forces its gaming profile. Send/receive windows, socket buffers, and an opt-in adaptive RTT/window controller are described in [Quantum tuning](quantum-tuning.md), including its memory accounting boundaries.

Raw Quantum/Gaming/TUN accept `raw.profile` (`tcp`, `udp`, `icmp`, `gre`, `ipip`, `bip`, `raw`), required real `peer_ip`, optional interface/local address/next-hop settings, `l4_port`, socket buffers, `dcpi_mode` (or its `proto58` alias), and optional `spoof_src_ip`/`spoof_dst_ip`. DCPI and spoof settings are mutually exclusive. The original legacy `proto58_src_ipv6` and `proto58_dst_ipv6` settings are rejected when nonempty; protocol 58 here is an IPv4 outer layout. Raw carriers use one configured peer and connection pool one. `addr` is configuration metadata; the actual raw peer is `raw.peer_ip`. The setup menu derives consistent endpoint metadata.

## TUN settings

```json
"tun": {
  "name": "dagger0",
  "address": "10.77.0.1",
  "peer_address": "10.77.0.2",
  "mtu": 1400
}
```

Addresses are plain IP literals **without a prefix**, use the same address family, and must differ. Reverse local/peer addresses on the other host. Names contain 1..15 ASCII letters, digits, underscores, or hyphens. MTU defaults to 1400, ranges from 576 to 9000, and must be at least 1280 for IPv6. TUN requires exactly one server listener or one client path with `connection_pool: 1`. Optional `forwarding_port` (1024..65535) enables the existing authenticated port/SOCKS multiplexer over an inner TCP connection between the two TUN IPs. Set the same port on both peers; the setup menu defaults this explicitly selected relay to 47475. Maps, SOCKS and `allowed_targets` require that field in TUN mode. Without it, only IP packet forwarding runs. `transport: "tun"` selects independent raw IP datagrams and requires this block plus `raw` settings. Other carriers can also carry TUN packets. Raw TUN's inner MTU must fit the selected interface MTU after outer headers and 47 bytes of authenticated datagram overhead; an incompatible MTU is rejected at runtime.

`--check` parses configuration, loads the private key, and validates relevant TLS files. It does not bind sockets, verify peer reachability, configure TUN, resolve target DNS, or establish a tunnel. `RUST_LOG=debug` enables operational diagnostics.
