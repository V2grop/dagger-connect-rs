# Raw IPv4 carriers

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

Community: [t.me/ir_spoof](https://t.me/ir_spoof). This is a display-only link.

The supplied `v4.2.8-stable` ELF has SHA-256
`08ae08364b1569d4176725ce879e5d779fe287bf211f66247874aa26a570ac76`.
Its recovered TUN/IPX encoder and decoder implement the following Ethernet II
and IPv4 outer packet layouts. The Rust implementation reproduces these outer
layouts around its own locally authenticated tunnel payloads.

| Profile | IPv4 protocol | Header following IPv4 |
|---|---:|---|
| `tcp` | 6 | 20-byte TCP; data offset 5, PSH + ACK, window 65535 |
| `udp` | 17 | 8-byte UDP; zero UDP checksum permitted for IPv4 |
| `icmp` | 1 | 8-byte ICMP echo request from client, reply from server |
| `bip` | 1 | Same ICMP layout; original also has a raw-IP transmit fast path |
| `gre` | 47 | `00 00 08 00`, then payload |
| `ipip` | 4 | Payload directly after IPv4 |
| `raw` | 253 | Payload directly after IPv4 |
| DCPI | 58 | `da 66 e7 01`, then payload |

Every ordinary packet has a 20-byte IPv4 header, TTL 64, and a valid IPv4
checksum. TCP has a valid IPv4 pseudoheader checksum; ICMP has its own checksum.
DCPI is IPv4 carrying protocol 58. It does not use an IPv6 outer header. Its
IPv4 DF bit is set; the decoder rejects fragments, invalid checksums, unexpected
protocols, invalid lengths and an incorrect four-byte marker. Ethernet padding
is excluded using IPv4's total-length field.

`proto58: true` is an alias for `dcpi_mode: true`; either flag selects this same
IPv4 carrier. The original 4.2.8 validator merges these flags with logical OR.
Although the generated original configuration lists `proto58_src_ipv6` and
`proto58_dst_ipv6`, its validator rejects nonempty values when this mode is
enabled. Those fields do not identify another recovered IPv6 wire protocol.
The Rust configuration accepts the legacy field names with empty defaults and
rejects nonempty values with an explicit error. IPv6 traffic inside the tunnel
is supported independently of the outer IPv4 carrier.

The `raw` options sit inside a listener or client path:

```json
"raw": {
  "profile": "tcp",
  "interface": "eth0",
  "local_ip": "192.0.2.1",
  "peer_ip": "192.0.2.2",
  "l4_port": 443,
  "dcpi_mode": false,
  "sock_buf": 4194304
}
```

The runtime reads the existing route and interface information through
iproute2, binds one `AF_PACKET` socket to the selected interface, and resolves
the next-hop MAC using ARP. It requires `CAP_NET_RAW`; creating the inner TUN
also requires `CAP_NET_ADMIN`. It changes no route, firewall, sysctl or neighbor
entry. The interface must be Ethernet or veth. `interface`, `local_ip` and
`gateway_ip` can be supplied explicitly. `peer_mac` supplies an explicit
next-hop MAC and skips ARP, useful for isolated tests or unusual network paths.

`spoof_src_ip` and `spoof_dst_ip` change the outer IPv4 fields while transmission
continues to the real peer's Ethernet next hop. Both peers must have a network
path that actually delivers those frames. Destination spoofing therefore
requires deliberate network configuration; a normal routed Internet path
may forward or discard the packet according to its destination field. DCPI
and spoofing are mutually exclusive, matching the upstream setup menu.
Peer authentication remains in Noise and does not depend on a claimed outer
source IP.

Quantum's raw TCP disguise adds the recovered timestamp options and sequence /
acknowledgement pattern. The default `tcp_flags: "pa"` emits PSH + ACK with two
NOP bytes and a TCP timestamp option (32-byte TCP header). Including `s` emits
the recovered MSS 1460, SACK-permitted, timestamp and window-scale 8 options
(40-byte header). Supported flag letters are `f`, `s`, `r`, `p`, `a`, `u`.
The timestamp seed is generated locally. Ordinary TUN/IPX keeps its recovered
20-byte TCP header. Quantum's raw TCP IPv4 header sets TOS `0xb8` (DSCP 46,
ECN zero) and the DF bit, directly matching the recovered packet template.
Ordinary TUN/IPX retains TOS zero and no IPv4 flags; DCPI retains its separate
TOS-zero, protocol-58, DF-set template.

The server's outer TCP/UDP source port is `l4_port`, and the client's source port
is `source_port` when configured, otherwise `l4_port + 1` (65534 when the service
port is 65535). Both ends must specify the same `source_port` override. This is
an explicit replacement for the original PSK-derived source port. The packet
socket pins transmission to one configured peer; independent peer pairs use
separate profile configurations.

ICMP identifiers use the sender's configured port and receivers check the
peer's identifier. This direction check discards automatic kernel echo replies
that merely reflect an outgoing tunnel datagram, while authenticated tunnel
responses carry the other peer's identifier. It replaces the original
proprietary payload direction/control checks.

`tests/raw_vm.sh` defines a root-only local namespace matrix. It covers all
seven profiles, DCPI, source spoofing, destination spoofing, their combination,
ARP resolution, Quantum raw transport and the gaming profile. Each scenario
tests inner IPv4 and IPv6 ICMP, UDP and TCP half-close, a server restart and TUN
cleanup. The TCP default scenarios introduce 2% packet loss inside the client
namespace. Execution results belong in the test report; the script's existence
alone does not establish a passed test.

## Reconstruction scope

The Rust payload protocol uses locally pinned Noise identities instead of the
original PSK, license-derived AES keys and proprietary control messages. It
is intended for Rust-to-Rust peers. Outer framing recovery does not imply
interoperability with a licensed DaggerConnect core.

Static evidence addresses in the supplied ELF:

- `0xa69f80`: ordinary TCP, UDP, ICMP and BIP packet construction.
- `0xa6a880`: fallback GRE, IPIP and protocol-253 construction.
- `0xa6b580`: ordinary packet extraction.
- `0xa69be0`: TCP/UDP port rewrite and TCP sequence/checksum rewrite.
- `0xa6d9a0`: original BIP raw-IP socket (`AF_INET`, `SOCK_RAW`, `IPPROTO_RAW`,
  `IP_HDRINCL`), with pcap fallback in `0xa71b40`.
- `0xa7e6e0`: DCPI encoder; `0xa7eb40`: DCPI decoder.
- `0xa6fe00`: dispatch between ordinary and DCPI outer formats.
- `0xa529e0`: Quantum raw `WriteTo`; its IPv4 template stores `0xb80504`
  at byte offset 48 of the gopacket IPv4 object (version 4, IHL 5, TOS `0xb8`)
  and `0x64000000002` at offset 56 (DF, TTL 64, TCP protocol 6).
- `0xa7e160`: IPX configuration validation. Go reflection identifies `proto58`
  at offset `0x90`, the IPv6 strings at `0x98` and `0xa8`, and `dcpi_mode` at
  `0xb8`. The validator ORs the two flags into `proto58` and rejects nonzero
  IPv6 string lengths at `0xa0` and `0xb0`. Constructor `0xa74b20` then uses
  that merged flag for the existing IPv4 protocol-58 encoder/decoder.

These are factual observations from static analysis, not original source code.
