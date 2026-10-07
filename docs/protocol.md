# Independent protocol, version 1

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

This describes dagger-rs, not the original DaggerConnect wire format. Both peers must use compatible versions and matching carriers.

## Carrier layer

TCP/DC6 expose an ordered byte stream; DC6 additionally requires IPv6 endpoints. KCP provides an ordered stream over UDP. HTTP modes exchange `GET <http_path> HTTP/1.1` with `Upgrade: dagger-rs-v1`, receive status 101, and then carry raw encrypted records. WS modes use a standard WebSocket upgrade on the configured path and binary messages; WebSocket message boundaries are not Noise-record boundaries.

XHTTP stream-up exchanges a long chunked POST and a status-200 chunked response. Packet-up uses a random 32-byte session identifier encoded as 64 hexadecimal characters, a persistent chunked GET download and finite numbered POST uploads. Auto probes streaming before falling back to packet-up. Physical HTTP roles can be reversed while logical Noise roles remain unchanged. Chunk boundaries are independent of record boundaries. HTTPS/WSS/XHTTPS add verified TLS below these exchanges. [XHTTP framing](xhttp.md) describes request paths, acknowledgement ordering, limits and cleanup.

Quantum+ supplies a reliable KCP stream over UDP with a 10+1 Reed-Solomon FEC envelope. Quantum/Gaming supplies that stream over the configured raw IPv4 packet adapter. A clear FEC header contains little-endian sequence/type fields, and the data shard starts with its two-byte little-endian size. The original PSK header encryption is absent; Noise authenticates the stream. Raw outer headers and TCP camouflage are described in [raw-protocol.md](raw-protocol.md).

## Authentication and records

Noise pattern: `Noise_IK_25519_ChaChaPoly_BLAKE2s`. Prologue: ASCII `dagger-rs/reverse-tunnel/v1`. The initiator knows the responder's static key; the responder checks the recovered initiator static key against its allowed list before completing the handshake. Both handshake payloads are empty.

Handshake and transport messages are prefixed by a two-byte unsigned **big-endian** length. Zero-length records are rejected; the maximum encoded record is 65535 bytes. Transport records have a 16-byte authentication tag and Noise-managed directional nonce counters. Modified and replayed records fail authentication. A failed or cancelled record read/write requires discarding the connection; record retries with a reused cipher state are not supported.

## Multiplexing frames

One plaintext record contains one frame. Except heartbeats, frames start with a one-byte tag and a nonzero four-byte big-endian stream ID. The remainder has no extra length field because the record provides the boundary.

| Tag | Frame | Remaining bytes |
| --- | --- | --- |
| 1 | Open | Kind byte (`1` TCP, `2` UDP), then UTF-8 target, 1..1024 bytes. |
| 2 | Opened | Empty. |
| 3 | Data | 0..16384 payload bytes. |
| 4 | Eof | Empty; TCP write half-close. |
| 5 | Close | Empty. |
| 6 | Error | UTF-8 message, at most 1024 bytes. |
| 7 | Ping | Eight-byte big-endian nonce directly after tag; no stream ID. |
| 8 | Pong | Same layout as Ping. |
| 9 | Credit | Four-byte big-endian frame count, 1..16. |

Unknown tags, invalid IDs, invalid UTF-8, oversize payloads, wrong fixed lengths, and trailing bytes on fixed-size frames are rejected. Each UDP Data frame represents one complete datagram. TCP Data frames may split application writes. Credits bound unconsumed data per stream. Control and close processing must not turn a slow target into an unbounded queue.

TUN mode over a stream carrier carries each IP packet in a Data frame with stream ID 1, with Ping/Pong for liveness. There is no Open/Opened exchange or port-forwarding credit negotiation in this mode. With the dedicated `tun` raw carrier, packets use independent authenticated datagrams and a replay window instead of ordered records; [raw-tun.md](raw-tun.md) specifies this framing. IP version, declared packet length, and configured MTU are checked before injection. Configure TUN consistently on both ends; it is not negotiated automatically with a forwarding peer.
