# Security model

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

The client pins the server's static public key; the server authorizes configured client keys. Noise IK authenticates and encrypts records using X25519, ChaCha20-Poly1305, and BLAKE2s. TLS carriers also verify certificates and server names. The two trust mechanisms are independent. No insecure-certificate switch is provided.

No application code contacts a subscription service, vendor API, external identity server, or telemetry service. It does not download another executable. Normal network access consists of configured tunnel peers and authorized target traffic, including ordinary system DNS lookup for hostnames. Installing Rust dependencies from source still uses the usual package registry unless dependencies are already cached.

Private Noise identities and TLS private keys stay local and must not be committed or put in release archives. Generated private files use Unix mode 0600 and are never overwritten. Exchange public keys and trust certificates over a trusted channel. Key rotation requires updating peer pins. TUN and service setup require elevated privileges; unprivileged port forwarding does not.

In forwarding mode, `allowed_targets` compares exact requested strings, not resolved IPs or CIDR ranges. `localhost:8080` differs from `127.0.0.1:8080`. DNS names may resolve differently over time. An empty allowlist denies access; `"*"` grants access to any target the client can reach. A compromised authorized server can request any allowed destination. In TUN mode, Linux routes and firewall rules govern ordinary IP packet access. When `tun.forwarding_port` is enabled, its additional authenticated TCP/SOCKS relay enforces the forwarding allowlist; that allowlist does not restrict ordinary IP packets carried by the TUN.

Port maps and SOCKS have no additional user authentication. Keep their listeners on loopback or apply firewall and destination-service authentication. Authorize only client identities appropriate for the server's shared forwarding setup. Public-key authorization does not assign separate destination namespaces to clients.

The original vendor's licensing and crypto material are not reused. Quantum uses recovered raw packet layouts and KCP/FEC; Quantum+ uses UDP KCP/FEC; direct raw TUN preserves unreliable IP delivery. The independent authenticated payloads change original wire interoperability. Raw TUN uses a bounded replay window and explicit authenticated nonces so reordering and loss do not force ordered delivery. HTTP/WebSocket/TCP camouflage provides no promise of indistinguishability, censorship resistance, or evasion. The project has functional tests but has not undergone an independent cryptographic or penetration audit.

Raw carriers need `CAP_NET_RAW`; creating a TUN interface needs `CAP_NET_ADMIN`. Raw receive paths pin their configured peer and validate packet lengths/checksums before processing authenticated payloads. IP spoof and DCPI settings control outer headers locally; they add no external identity or verification dependency. XHTTP request/session/body limits bound HTTP resource use, and TLS verification stays enabled for all edge attempts.
