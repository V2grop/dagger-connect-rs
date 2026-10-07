# dagger-rs

[راهنمای فارسی](README.fa.md) · [Maintenance review](docs/maintenance-review.md)

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

Source and documentation comments carry additional attribution markers for ir_spoof. They use ZERO WIDTH SPACE (U+200B) and WORD JOINER (U+2060) only within comments; visible credits and URLs remain ordinary text. A marker can survive a simple literal replacement of the visible handle, but it cannot prevent deliberate removal.

The community link is displayed locally; the application does not contact Telegram.

An independent Rust tunnel for Linux. Forward TCP/UDP ports, expose a SOCKS5 CONNECT proxy, or connect two Linux TUN interfaces over an encrypted, authenticated connection. Identities and configuration stay under your control: there is no PSK, subscription check, vendor API, telemetry, or runtime binary download.

This is new source with locally authenticated framing. It **does not interoperate with DaggerConnect**. The supplied 4.2.8 core was analyzed to recover Quantum FEC, raw IPv4 layouts, TCP camouflage, spoofing and DCPI behavior. Authentication and payload encryption use local public keys instead of the original PSK and vendor verification. See [reconstruction evidence](docs/reconstruction.md) for the feature inventory and remaining differences.

## Install a published V2grop Linux release

On Ubuntu/Debian x86-64, after a successful release is available:

```sh
sudo apt-get update && sudo apt-get install -y ca-certificates curl python3
curl -fL https://raw.githubusercontent.com/V2grop/dagger-connect-rs/main/scripts/install-release.sh -o install-dagger.sh && sudo bash install-dagger.sh
```

The helper selects one published release, downloads its installer and SHA-256 file, verifies the bundle and opens setup. See [Releases](https://github.com/V2grop/dagger-connect-rs/releases) and [the Persian guide](README.fa.md). Linux CI publishes a maintenance release only after all configured checks pass.

## Install the local Linux release

Place `dagger-rs-linux-x86_64.run` on each Linux host and run:

```sh
sudo bash dagger-rs-linux-x86_64.run
```

This verifies the embedded archive, installs the local executable and opens the configuration/service menu. `--help` lists extraction and installation options. The equivalent archive command is:

```sh
tar -xzf dagger-rs-linux-x86_64.tar.gz && sudo bash dagger-rs-linux/scripts/install.sh && sudo /usr/local/bin/dagger-setup
```

The installer uses the binary inside the local archive. It installs `dagger-rs`, the `dagger-setup` menu, and a configuration directory. The menu generates keys, configures both tunnel roles and carriers, validates settings, runs the application, generates TLS certificates, and creates/starts/enables systemd services when selected. Installation normally uses `/usr/local` and `/etc/dagger-rs`. `install.sh --help` shows local prefix and binary overrides. Bash and Python 3 are needed for the menu; systemd is needed for service actions; TUN/raw modes need iproute2. No installer or runtime vendor contact is used.

Generate a different identity on each machine. Exchange **public** keys through a trusted channel and keep private keys local. Configure both peers and permit the selected transport through your network. KCP and Quantum+ use UDP; ordinary stream carriers use TCP; raw Quantum and TUN use their selected IPv4 protocol. Quantum+ knock also uses a separate UDP port. The software configures its own TUN interface when requested; firewall, forwarding and NAT policy remain administrator settings.

## Carriers and forwarding modes

All carriers carry authenticated Noise IK sessions. TLS adds certificate verification for the TLS variants; it does not replace peer-key authentication.

| Carrier | Network behavior |
| --- | --- |
| `tcp` | Encrypted records over TCP; default. |
| `kcp` | Reliable KCP stream over UDP. This is a separate, genuine KCP mode, not a Quantum alias. |
| `http` / `https` | HTTP/1.1 upgrade followed by encrypted records, optionally inside TLS. |
| `ws` / `wss` | Binary WebSocket messages, optionally inside TLS. |
| `xhttp` / `xhttps` | HTTP/1.1 stream-up, packet-up or automatic selection; verified TLS, edge failover, Host/SNI override and reversed physical roles. |
| `dc6` | Encrypted TCP with literal IPv6 tunnel addresses. |
| `quantum+` | Reliable KCP over UDP with recovered 10-data/1-parity FEC and optional knock. |
| `quantum` / `quantum-gaming` | Reliable KCP/FEC over raw IPv4 packets; TCP camouflage and gaming timing profile. |
| `tun` | Individual authenticated IP datagrams over raw TCP, UDP, ICMP, GRE, IPIP, BIP or protocol 253. |

TCP forwarding supports half-close. UDP forwarding supports datagrams up to 16 KiB. SOCKS5 supports TCP CONNECT; username/password authentication and UDP ASSOCIATE are not implemented. Linux TUN mode carries layer-3 packets over any configured carrier. Set `tun.forwarding_port` on both peers to also carry port maps/SOCKS through an authenticated TCP relay over their inner TUN IPs; the setup menu offers `tun+ports`. The `tun` carrier preserves IP datagram loss/reordering; the stream carriers use reliable ordered delivery. TUN needs `/dev/net/tun`, `iproute2`, and root or `CAP_NET_ADMIN`. Raw carriers also need `CAP_NET_RAW` and an Ethernet/veth underlay.

Raw carriers support source/destination IPv4 spoof addresses and DCPI, using IPv4 protocol 58 and the recovered `da66e701` marker. These options alter outer packet fields; delivery depends on the actual network path. See [raw carrier settings](docs/raw-protocol.md), [raw TUN framing](docs/raw-tun.md) and [XHTTP settings](docs/xhttp.md). Local proxy tests do not establish compatibility with every external CDN. No carrier claims resistance to traffic analysis or compatibility with original peers.

## How the core works

Each peer has a local public/private identity. The client pins the server's public key, and the server explicitly allows client keys. A Noise IK handshake establishes encrypted sessions without a vendor service. TCP maps, UDP maps and SOCKS5 requests share those sessions; the client checks the destination allowlist before opening each forwarded connection. Bounded queues and stream credits control memory, and heartbeat/retry logic reopens a failed tunnel for new connections.

Stream carriers provide ordered bytes through TCP, WebSocket, HTTP bodies or KCP. Quantum adds the recovered 10-data/1-parity FEC around KCP, using UDP for Quantum+ or selected raw IPv4 envelopes for Quantum. Gaming changes the timing/window profile. The optional local tuner measures matching ACKs and throughput; [Quantum tuning](docs/quantum-tuning.md) explains its limits.

TUN mode reads and writes complete Linux IP packets. With the `tun` carrier, packets are individually encrypted and replay-checked over raw IPv4, preserving datagram loss and reordering. With other carriers they use a reliable stream. Spoofing and DCPI change only configured outer packet fields. XHTTP can split uploads into finite POSTs with a streaming download, or use stream-up; reversed mode swaps the physical HTTP dial/listen sides while retaining the logical identity roles.

Runtime connections go to configured peers and forwarding destinations. No update, activation, licensing or telemetry request is made. The Telegram link is displayed text. Authentication is local, and the new authenticated framing intentionally changes compatibility with original DaggerConnect peers.

## Manual two-host setup

```sh
# On the server:
dagger-rs keygen --out keys/server
# On the client:
dagger-rs keygen --out keys/client
```

Copy [examples/server.json](examples/server.json) and [examples/client.json](examples/client.json) to `config/server.json` and `config/client.json` on their respective hosts. Replace the public-key placeholders with the opposite host's public key, set the client's server address, and adjust maps and allowlists. Paths to keys and TLS files resolve relative to the config file.

```sh
dagger-rs --config config/server.json --check
dagger-rs --config config/server.json
# On the client:
dagger-rs --config config/client.json --check
dagger-rs --config config/client.json
```

The examples forward server TCP `127.0.0.1:18080` to client `127.0.0.1:8080`, and server UDP `127.0.0.1:15353` to client `127.0.0.1:5353`. Those destination services must already run on the client. SOCKS binds server loopback port 1080. An empty client `allowed_targets` denies all forwarded targets; `"*"` explicitly allows any target. Multiple attached clients should have equivalent destination reachability because sessions are pooled, not selected by a named destination owner.

For TLS, generate a local certificate on the server:

```sh
dagger-rs certgen --out keys/tls --name tunnel.example.com
```

Set the listener's `cert_file` and `key_file`; copy only `cert.pem` to the client as `ca_file`, and set its `server_name` to `tunnel.example.com`. Omit `ca_file` to use the bundled public certificate roots with a publicly trusted server certificate. Verification is always enabled. See [TLS examples](examples/server-wss.json), [TUN examples](examples/server-tun.json), and the [configuration reference](docs/configuration.md).

## Build and checks

Use a current stable Rust toolchain and Linux native build tools. No Go toolchain or original binary is needed.

```sh
cargo build --locked --release
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --release --all-targets
bash scripts/package-linux.sh target/release/dagger-rs dist
```

The executable is `target/release/dagger-rs`. Tests use temporary local keys and loopback networking. In an isolated Linux test machine with `iproute2`, Python 3, and ping, run `sudo bash tests/tun_vm.sh target/release/dagger-rs` for privileged IPv4/IPv6 namespace traffic, reconnect, and interface cleanup checks. CI targets Linux and runs this separate TUN check. Keep operational keys and configuration outside the source archive. `Cargo.lock` fixes dependency resolution. The MIT license covers this new source, not the original application.

The [verification report](docs/testing.md) identifies the tested executable, Linux tunnel scenarios, installation/service checks, reproduction commands and development failures. Historical results are distinguished from the current release.

The setup menu includes an [explicit-peer link tester](docs/linktest.md) for TCP/UDP RTT, loss, reverse connectivity and bidirectional throughput. Its CLI prints JSON and can save a report. It contacts only selected peers and ports.

Read [architecture](docs/architecture.md), [wire protocol](docs/protocol.md), [security](docs/security.md), and [reconstruction evidence](docs/reconstruction.md) for implementation details and limits.

Dependency and runtime attribution is preserved in [third-party notices](docs/third-party.md) and `licenses/third-party`. Keep those files with redistributed binaries; the project MIT license does not replace upstream dependency licenses.
