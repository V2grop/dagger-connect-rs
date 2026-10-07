# Verification report for 0.2.0

Tested on **2026-10-07** in a local Linux VM. **All 60 unique Rust tests, 70 namespace scenarios, 14 installer/service check groups and 8 example/CLI check groups passed.** Results apply to the artifact below. The [0.1.0 report](testing-0.1.md) describes the previous release.

## Artifact and environment

| Item | Value |
| --- | --- |
| Version | `dagger-rs 0.2.0` |
| Target | Linux x86-64, `x86_64-unknown-linux-musl`, optimized release |
| Size | 7,721,864 bytes |
| SHA-256 | `d95d049b4e0b51874d874f624c0a8afbcc04424b5870d0de3cd9be8f07e994dc` |
| Compiler | Rust/Cargo 1.99.0; Windows GNU cross compilation with Rust LLD and Zig 0.17.0 for C dependencies |
| Runtime | Ubuntu 24.04.5 LTS, kernel 6.8.0-142-generic, systemd 255.4-1ubuntu8.17 |
| VM | QEMU 11.1.0, TCG software emulation, two virtual CPUs, 2 GiB RAM |
| Privileged facilities | `/dev/net/tun`, real namespaces and veth links, iproute2, Python 3 and ping |

The Linux executables were actually run in Linux. Every executable was checked against its [Cargo artifact manifest](test-results/0.2.0/artifact-manifest.json) before execution. ELF inspection confirms static x86-64 PIE with no interpreter or shared-library dependencies, stripped debug/full symbols, a non-executable stack, RELRO and no writable executable load segment. Targeted scans found no build-user/home paths or original vendor domains. See [static inspection](test-results/0.2.0/static-inspection.json) and [environment](test-results/0.2.0/environment.json).

Rust formatting, Bash syntax and Linux all-target Clippy with warnings denied passed. The final test-fixture edit was formatted, compiled and executed again; production code and the binary hash remained unchanged. [Quality output](test-results/0.2.0/quality.txt).

## Rust tests

There are **59 regular tests and one normally ignored privileged test**. Selected regressions were also run independently; repetitions do not increase the unique count.

| Group | Passed | Coverage |
| --- | --- | --- |
| [Library](test-results/0.2.0/dagger_rs.txt) | 41 | Strict configuration, key/certificate preservation, pinned Noise authentication, modified/replayed ciphertext rejection, raw layout vectors/DCPI, FEC recovery/bounds, ACK matching/expiry, bounded adaptive budgets, datagram replay handling, HTTP framing and session limits. |
| [Carrier integration](test-results/0.2.0/carriers.txt) | 2 | All ten ordered carriers: TCP, KCP, HTTP/S, WS/S, XHTTP/S, DC6 and Quantum+. Each forwards 1,048,577 TCP bytes with half-close, UDP including empty datagrams and SOCKS5; wrong TLS trust/name/path is rejected. |
| [Concurrent forwarding](test-results/0.2.0/forwarding.txt) | 1 | Concurrent bulk TCP, UDP/SOCKS, target allowlist denial and recovery after server restart. |
| [Link-test](test-results/0.2.0/linktest.txt) | 6 | IPv4/IPv6 integrity, selected source IP, explicit extra ports, token/size bounds and unreachable-port reporting. |
| [Quantum](test-results/0.2.0/quantum-final.txt) | 4 | 1,048,577-byte exchange with one deterministically lost data shard per 10+1 block, knock/port cleanup, simultaneous reconnect conversations and live adaptive tuning. |
| [XHTTP](test-results/0.2.0/xhttp.txt) | 4 | Plain/TLS packet-up in both logical directions, stream-up selection, buffering-proxy auto fallback and pinned-identity rejection with reversed physical roles. |
| [IP packet validation](test-results/0.2.0/tun_linux.txt) | 1 | IPv4/IPv6 version, length and MTU checks. |
| [Privileged TUN lifecycle](test-results/0.2.0/tun-privileged.txt) | 1 | Real exclusive TUN creation, refusal to attach to an existing interface, and descriptor-owned teardown. |

The CLI test executable contains zero unit tests and completed successfully; actual CLI behavior is covered below. The [adaptive stream regression](test-results/0.2.0/targeted-quantum-auto.txt) exercises congestion control, delayed writes/ACKs and a 30 ms interval. The [idle/drop regression](test-results/0.2.0/targeted-quantum-idle-drop.txt) verifies tuning without application I/O polling, a live wire window of 512 and controller shutdown on Drop.

## Real tunnel matrix

All **70 family-specific scenarios** used separate Linux namespaces and actual TUN/veth traffic:

- [Raw matrix: 48 passed](test-results/0.2.0/raw-matrix.txt). TUN and Quantum each exercised TCP/UDP/ICMP/GRE/IPIP/BIP/protocol-253 outer profiles, DCPI/proto58, source/destination/both spoof fields, plus TUN ARP resolution and Quantum Gaming. Every case used inner IPv4 and IPv6, bidirectional ICMP, 16 UDP datagrams of 0/1/512/1300 bytes, a 262,145-byte TCP exchange, server restart and interface teardown. Captured Ethernet packets verified actual outer source/destination, protocol, TOS/DF and carrier markers in both directions. Plain TCP outer cases injected 2% underlay loss and recorded real netem drops.
- Ordered TUN: **20 passed**, ten carriers times IPv4/IPv6. Each checked ICMP/UDP, server restart while retaining the client and interface cleanup. DC6 used an IPv6 underlay. One `tun-<carrier>.txt` is retained per carrier, including [TCP](test-results/0.2.0/tun-tcp.txt) and [Quantum+](test-results/0.2.0/tun-quantum+.txt).
- [Raw TUN plus forwarding: 2 passed](test-results/0.2.0/tun-forwarding.txt). Both inner families simultaneously exercised ping, 1,048,577-byte TCP/SOCKS half-close and 16 UDP datagrams, target denial, rogue-client/wrong-pin rejection, server restart and teardown. The relay was checked to bind only the actual inner TUN IP on port 47475.

## Offline installer, services and examples

The [installer summary](test-results/0.2.0/installer/summary.json) records **14 passed groups** against the same production hash:

- Self-extracting help, fresh-directory extraction, local installation, tampered-payload rejection, invalid path rejection and one-command configuration menu.
- Dedicated service account, private key permissions, coherent generated TCP and normal/reversed XHTTP/TLS configurations.
- Four actual systemd peer pairs for TCP, XHTTP, reversed XHTTP and reversed XHTTPS. Each started, transferred 262,145 bytes with half-close, restarted the server, transferred again, and stopped.
- Raw TUN packet-only and TUN plus forwarding service pairs, actual ICMP/restart checks, scoped NET_ADMIN/NET_RAW capabilities, and two forwarded TCP exchanges for the combined mode.

The summary's archive hashes identify the installer harness's temporary bundle. Final delivery archives additionally undergo byte/hash comparison and an extraction/offline installation smoke check after this report is included. Tests use system-level services; per-user services were not tested.

The [examples/CLI summary](test-results/0.2.0/examples-cli/summary.json) records **8 passed groups**, all **16 shipped JSON examples** and **15 invalid CLI cases**. Only temporary identity/TLS paths and public-key placeholders were substituted in examples; operational fields were retained. Real IPv4/IPv6 link tests exercised selected source IP, bidirectional payloads, explicit extra ports, exact JSON saving, and nonzero failure results that still save the completed report.

After testing, the owned service units were inactive and no test TUN namespaces remained. The `core-lab` namespace in the environment record belongs to the separately isolated original-core investigation, not a tunnel leak.

## Failures found during development

An adaptive echo fixture initially closed its server 100 ms after a single KCP flush. Diagnostics showed the server had queued its final reply while the client awaited it, and the tuner had already advertised window 512. The fixture now waits for a final client receipt acknowledgement; its 15-second deadline and actual wire assertion remain. Auto tuning also now has an independently guarded one-second task, so adaptation continues during idle periods and stops on Drop. Off creates no controller task.

The deterministic-loss echo similarly closed after an unconfirmed 500 ms delay. The initial final-build run and an isolated retry hit the unchanged 45-second deadline; [initial output](test-results/0.2.0/quantum.txt) is retained. The corrected fixture keeps retransmission alive until the client acknowledges the entire 1 MiB response, without changing loss injection, parity assertions or deadline. It [passed in 1.28 seconds](test-results/0.2.0/quantum-loss-repaired.txt), and the final formatted full Quantum suite passed all four tests again.

The mixed TUN fixture originally included TIME_WAIT sockets in its listener inspection. Restricting that observation to LISTEN sockets (`ss -Hltn`) fixed the false failure; the two-family matrix passed. An earlier installer SSH wrapper expired at 900 seconds even though the guest harness completed all checks and cleanup. The final repeat used bounded, independently logged guest jobs. Windows console text selection paused result collection; clearing it resumed the controller without restarting the tests.

## Reproduce

From the source root on Linux with a current Rust toolchain:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
cargo test --locked --release --all-targets
```

In a disposable Linux environment with Bash, Python 3, iproute2 and ping:

```sh
BIN=target/release/dagger-rs
sudo env DAGGER_TEST_NETEM_LOSS=2% bash tests/raw_vm.sh "$BIN" all
for carrier in tcp kcp http https ws wss xhttp xhttps dc6 quantum+; do
  sudo bash tests/tun_vm.sh "$BIN" "$carrier"
done
sudo bash tests/tun_forwarding_vm.sh "$BIN"
test_bin=$(find target/release/deps -maxdepth 1 -type f -name 'tun_linux-*' -executable -print -quit)
sudo "$test_bin" --ignored --nocapture
bash scripts/package-linux.sh "$BIN" dist
```

The tests configure owned interfaces/routes rather than the host default route and clean up their own identities, processes and namespaces. The private installer orchestration used for this report contains no shipped credentials or original core.

These checks establish local Rust-to-Rust functionality. The independent Noise framing intentionally changes original peer interoperability. Raw outer transport is IPv4 and currently uses one configured peer/pool one; BIP uses the packet-socket path. The original global host-memory governor is replaced by a local bounded governor. Local proxy tests do not establish every external CDN, and spoofed delivery depends on the actual network. See [reconstruction](reconstruction.md), [Quantum tuning](quantum-tuning.md), [raw protocol](raw-protocol.md), [raw TUN](raw-tun.md) and [XHTTP](xhttp.md) for those boundaries.
