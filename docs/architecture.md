# Architecture

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

Normally the client establishes outbound connections to one or more server paths. A carrier supplies an ordered byte stream: TCP, IPv6 TCP, KCP, HTTP upgrade, WebSocket, XHTTP bodies, or Quantum KCP/FEC. In reversed XHTTP mode the logical server connects to the client's HTTP origin; the logical authentication and forwarding roles stay the same. TLS verifies the physical listener's certificate first. Noise IK authenticates both static peer identities and encrypts every application record. The client pins the server's public key; the server allows only configured client public keys.

## Port forwarding

Server map listeners accept local TCP connections or UDP datagrams. They choose an authenticated session associated with that tunnel listener and send an Open frame with the target and transport kind. Before dialing, the client checks the exact target string against `allowed_targets`. A successful open is acknowledged; failures close that stream rather than granting access to another target.

TCP data travels in bounded 16 KiB chunks and preserves ordering and half-close. Per-stream credits bound in-flight frames so a slow destination does not block all other streams. UDP maps associate each sender with a connected client UDP socket; associations expire after idle time. A UDP payload is one Data frame and is limited to 16 KiB.

SOCKS5 accepts unauthenticated TCP CONNECT requests on the configured server socket. Requests use the same target authorization and stream machinery as maps. SOCKS can select any authenticated server session, whereas each map uses only sessions connected to its listener. The application does not match public-key identities to separate per-client destination namespaces.

Clients maintain configured connection pools and reconnect with bounded backoff. Heartbeats detect inactive peers. A lost tunnel terminates its active streams; reconnect supports new streams, not resumption or replay of an interrupted application connection. Resource limits cap sessions, streams, queues, and record sizes. Shutdown drops owned task trees and socket resources.

## Linux TUN

Optional TUN mode creates a point-to-point interface with the configured local and peer IP addresses. IP packets travel through the authenticated carrier. It uses one server listener or one client path with pool size one. Optional `tun.forwarding_port` starts an additional authenticated TCP multiplexer between the inner TUN addresses, allowing port maps, SOCKS and target allowlists alongside ordinary IP traffic. The kernel transports this TCP connection through the existing packet pump. Without that explicit port, TUN mode forwards IP packets only. It requires `iproute2`, `/dev/net/tun`, and suitable privileges. Only the point-to-point interface is configured; choosing additional routes, forwarding, NAT, and firewall policy is the administrator's responsibility.

With `transport: "tun"`, each IP packet is an independent encrypted datagram on a raw packet socket. Its authenticated session uses explicit nonces and a sliding replay window, permitting packet loss and reordering. Other carriers carry length-delimited IP packets on reliable streams. [Raw TUN framing](raw-tun.md) describes handshakes, heartbeats and reconnection. [Raw IPv4 carriers](raw-protocol.md) describes the recovered TCP/UDP/ICMP/GRE/IPIP/BIP layouts, TCP camouflage, spoofing and DCPI. These outer formats wrap this project's independent Noise payloads.

## Source layout

| Module | Responsibility |
| --- | --- |
| `config.rs` | Strict JSON, limits, key and certificate generation. |
| `carrier.rs` | TLS, HTTP, WebSocket, and streaming HTTP adapters. |
| `kcp_carrier.rs` | Reliable UDP stream carrier. |
| `quantum_carrier.rs` | 10+1 FEC, UDP knock, Quantum UDP/raw stream adapters. |
| `quantum_tuning.rs` | Local RTT/throughput measurements, bounded adaptive socket buffers and KCP windows. |
| `xhttp_carrier.rs` | Bounded packet uploads, streaming probes, HTTP session cleanup. |
| `raw_packet.rs` | IPv4/transport checksums, outer packet layouts and validation. |
| `raw_socket.rs` | Linux packet socket, interface discovery and next-hop ARP. |
| `raw_tun.rs` | Authenticated unreliable IP datagrams and replay protection. |
| `transport.rs` | Noise handshake, peer authorization, encrypted records. |
| `wire.rs` | Multiplexing frame encoding and validation. |
| `engine.rs` | Sessions, forwarding, flow control, reconnects and liveness. |
| `tun_device.rs` | Linux virtual network interface. |
| `main.rs` | CLI, validation, shutdown. |
| `linktest.rs` | Explicit-peer TCP/UDP connectivity and throughput diagnostics. |

HTTP carriers expose their protocol headers/path to the network unless wrapped in TLS. The payload remains protected by Noise in every mode. No component fetches an application core or contacts a licensing service.
