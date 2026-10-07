# Scope and evidence

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

The requested reference artifact was `DaggerLauncher (1)`, a 6,955,237-byte Linux x86-64 Go ELF with SHA-256 `cb3652fee0776896ceac4569b497449b510c0c4ee0d5a9bdf5f1ee82e7962ed9`.

Ghidra MCP imported and analyzed it. A separate Go metadata parser recovered 7,237 function entries; the application main function is at `0x6a7600`. Main decompilation timed out, so analysis used bounded smaller functions, call graphs, type metadata, and isolated string-decoder emulation. During that initial analysis, the launcher was not run. Later bounded public-URL retrieval attempts failed; no licensed request or fabricated authentication was submitted.

REA MCP was invoked, but its native backend could not open this Linux ELF on the Windows host. That limitation was recorded in its evidence ledger. REA did not supply runtime verification of the application.

The original launcher:

- Accepts a JSON configuration, with fields `mode`, `psk`, `paths[].addr`, and `listeners[].addr` established by Go type metadata at `0x7a9980` and its loader at `0x687880`.
- Requires a nonempty `psk` and has artifact/license verification paths involving HTTP, JSON, HMAC, SHA-256, and Ed25519. String decoding recovered channel/version settings and license/attestation errors.
- Writes separately supplied executable bytes to a sealed Linux memory file and uses `syscall.Exec` (`0x69d0c0`, helper `0x6a1520`). It is a launcher for a separate tunneling core.

The public [DaggerConnect repository](https://github.com/itsFLoKi/daggerConnect), inspected at commit `e188cd913fe925a0487cb5eca919187a52025e3b`, contained installation scripts and documentation, not that core's source. Its installer explicitly distinguishes the launcher and licensed core.

Full Git history contained an older public `DaggerConnect` core at commit `efc40b7` dated 2026-01-07, removed in commit `89822d5` on 2026-01-30. This 11,194,514-byte Linux x86-64 Go 1.21.5 ELF has SHA-256 `3fa9b3beb7e1caa7671b6dfe26ea0e531492b2cbf56d217996324f07a9125784` and retains symbols and DWARF. Ghidra MCP imported it and focused decompilation established TCP, KCP, WebSocket/TLS transports and smux multiplexing. Its KCP defaults include MTU 1400, send/receive windows 1024, NoDelay `(1,10,2,1)`, AES block crypt derived from its original PSK, and 10/3 FEC shards. These details informed the transport inventory; the new implementation does not reuse original credentials, PSK crypto, or claim wire compatibility.

The historical core contains no application Quantum, Quantum+, modern TUN, XHTTP, or DC6 implementation. A string named `decodeQuantum` belongs to Go base64 decoding and is not evidence of Quantum transport. Current public release assets examined contained launchers. A bounded follow-up search inspected the 20 newest public forks: their root file inventories contained no core candidate, and all 20 had no release assets. The historical branded core URL on the main host (HTTPS) and Iranian host (HTTP and HTTPS) returned HTTP 404. These checks do not prove that no public copy exists elsewhere; they establish the search boundary. The current signed delivery protocol does not establish a public unauthenticated core download URL.

The user subsequently chose a standalone Rust tunneling tool and launcher scripts without PSK/vendor verification, then requested Linux and broader transport coverage. This repository implements that scope independently: TCP/UDP forwarding, SOCKS5 CONNECT, standard carriers, and optional Linux layer-3 TUN using locally pinned public keys. It contains no license bypass or downloaded core. Public-key authentication protects the new tunnel independently of the original vendor's authorization system.

The user later supplied the actual candidate `v4.2.8-stable`: a 17,798,468-byte Linux x86-64 Go ELF with SHA-256 `08ae08364b1569d4176725ce879e5d779fe287bf211f66247874aa26a570ac76`. Executed inside a no-egress Linux network namespace in the disposable test VM, its `-v` output is `DaggerConnect v4.2.8-stable`. This establishes the file identity and its internal version; no independently published release checksum was obtained.

Ghidra imported that core. Go metadata recovered 15,711 function entries. Correcting the Go text base to `0x402780` was essential before matching call sites to functions. Focused decompilation recovered Quantum's 10-data/1-parity Reed-Solomon FEC, separate UDP Quantum+ knock, gaming KCP parameters, raw IPv4 packet layouts, TCP camouflage, spoofing fields and DCPI marker/protocol. Private analysis artifacts stay outside the source bundle. The original binary, decompiled functions, embedded keys and private credentials are not redistributed.

| Feature | Rust recreation | Compatibility boundary |
| --- | --- | --- |
| TCP, KCP, HTTP/TLS, WebSocket/TLS, DC6 | Ordered authenticated streams, reverse maps and TUN forwarding. | Independent Noise and multiplexing frames. |
| Quantum+ | UDP KCP, 10+1 FEC and optional separate-port knock. | PSK knock token/header encryption removed; Noise authenticates payloads. |
| Quantum/Gaming | Raw IPv4 KCP/FEC with recovered camouflage, timing profiles and opt-in RTT/window tuning. | Independent authenticated payloads; original host-memory classification and global buffer governor are replaced by a documented local governor. |
| Raw TUN | Per-packet unreliable IP delivery, TCP/UDP/ICMP/GRE/IPIP/BIP/protocol-253 outer layouts. | Independent datagram handshake, encryption and replay framing. BIP uses the packet-socket path. |
| IP spoofing and DCPI | Local source/destination spoof fields; protocol 58 and `da66e701` marker. | Delivery depends on the administrator's network. |
| XHTTP | Stream-up, packet-up, auto detection/fallback, reversed roles, edge failover, Host/SNI, padding and User-Agent. | HTTP/1.1 with independent session framing; external CDN behavior is not established by local tests. |
| Authentication/privacy | Local pinned public keys; no PSK, vendor verification, runtime download or telemetry. | Intentionally changes original peer interoperability. |

The new protocol is described in [protocol.md](protocol.md), [raw-tun.md](raw-tun.md), [raw-protocol.md](raw-protocol.md), [Quantum tuning](quantum-tuning.md) and [xhttp.md](xhttp.md). Test declarations alone do not establish successful execution; [testing.md](testing.md) identifies the actual tested artifact and scenarios.

Analysis tools: [Ghidra MCP](https://github.com/bethington/ghidra-mcp), [REA](https://github.com/morluto/rea).
