# Verification report for 0.2.1

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker. -->

Tested on **2026-10-07** in a disposable Linux VM with **no network adapter**. All **60 unique Rust tests**, formatting, all-target Clippy, shell syntax, **16 shipped examples**, **8 example/CLI check groups** and offline installer/menu checks passed. Loopback networking remained available to the tunnel tests. The [full summary](test-results/0.2.1/summary.json) records 33 passed checks and the exact executable hashes.

This release adds ir_spoof attribution, ignored `_comment` metadata in example/generated configurations, and publication cleanup. Transport implementations retain the previous release's behavior. The **70 namespace scenarios and 14 service/installer groups belong to the separate [0.2.0 verification](testing-0.2.0.md)**; they were not repeated for this attribution release and are not presented as current-binary results. The [0.1.0 report](testing-0.1.md) is also retained as historical evidence.

## Artifact and environment

| Item | Value |
| --- | --- |
| Version | `dagger-rs 0.2.1` |
| Target | Linux x86-64, `x86_64-unknown-linux-musl`, optimized release |
| Size | 7,712,936 bytes |
| SHA-256 | `fccb58880f6dfd53e74349b5780f662b3b2c9f1fea6c21d9e3c738b830457eb7` |
| Compiler | Rust/Cargo 1.99.0; Windows GNU cross compilation with Rust LLD and Zig 0.17.0 for C dependencies |
| Runtime | Ubuntu 24.04.5 LTS, kernel 6.8.0-142-generic |
| VM | QEMU 11.1.0, TCG, two virtual CPUs, 2 GiB RAM, no network adapter |
| Privileged facilities | Real Linux `/dev/net/tun` and root for the isolated lifecycle test |

All test executables and the production binary were hash-checked against the [artifact manifest](test-results/0.2.1/artifact-manifest.json) before Linux execution. [Static inspection](test-results/0.2.1/static-inspection.json) records executable properties and publication-path scans. The VM received its inputs through a read-only CD image and exported results through a file-backed serial console; no SSH, vendor service or external website was involved.

The path-clean rebuild reused unchanged ring 0.17.14 C/assembly compilation from a hash-pinned cache after verifying all 391 dependency source files against the locked crate. All Rust sources were rebuilt with generic path remapping. [Build provenance](test-results/0.2.1/build-provenance.json) records the dependency and static-library hashes; final ELF inspection checks that private workspace strings and debug sections are absent.

## Current Rust tests

There are **59 regular tests and one normally ignored privileged TUN test**. The CLI executable contains zero unit tests; its actual commands are tested separately.

| Group | Passed | Coverage |
| --- | --- | --- |
| [Library](test-results/0.2.1/rust-dagger_rs.txt) | 41 | Strict configuration, pinned Noise authentication, ciphertext/replay rejection, raw/DCPI layout vectors, FEC recovery, ACK matching and bounded adaptive budgets. |
| [Carrier integration](test-results/0.2.1/rust-carriers.txt) | 2 | Ten ordered carriers, 1,048,577-byte TCP exchanges with half-close, UDP including empty datagrams, SOCKS5 and negative TLS/path checks. |
| [Concurrent forwarding](test-results/0.2.1/rust-forwarding.txt) | 1 | Bulk TCP with UDP/SOCKS, target denial and server restart. |
| [Link-test](test-results/0.2.1/rust-linktest.txt) | 6 | IPv4/IPv6 integrity, source selection, extra ports and bounded failures. |
| [Quantum](test-results/0.2.1/rust-quantum.txt) | 4 | 10+1 FEC with deterministic shard loss, knock cleanup, reconnect and live adaptive tuning. |
| [XHTTP](test-results/0.2.1/rust-xhttp.txt) | 4 | Plain/TLS packet-up, reversed roles, stream-up, buffering-proxy fallback and pinned-identity rejection. |
| [IP validation](test-results/0.2.1/rust-tun_linux.txt) | 1 | IPv4/IPv6 packet lengths, versions and MTU. |
| [Privileged TUN lifecycle](test-results/0.2.1/tun-privileged.txt) | 1 | Actual exclusive creation, existing-interface refusal and descriptor-owned teardown. |

[Rust formatting](test-results/0.2.1/rust-fmt.txt), [Linux all-target Clippy with warnings denied](test-results/0.2.1/rust-clippy.txt), and Bash syntax for all 11 shell scripts/templates passed without a formatting repair.

## Attribution, configurations and installation

The actual binary's [help](test-results/0.2.1/core-help.txt) displays the ordinary ir_spoof handle and channel URL. Generated [server](test-results/0.2.1/generated-server.txt) and [client](test-results/0.2.1/generated-client.txt) JSON include `_comment` attribution. The field is accepted and ignored by configuration loading; it does not change transport data or trigger a network request. Additional U+200B/U+2060 attribution variants occur only in comments or non-operational comment metadata; no bidirectional controls or modified functional URLs are used.

The [examples/CLI summary](test-results/0.2.1/examples-cli/summary.json) records all **16 example files**, **15 invalid-command cases**, local identity/certificate generation, generated JSON and actual IPv4/IPv6 link tests. Only temporary identities, certificate paths and public-key placeholders were substituted in examples; operational fields and the new comment metadata were retained. Cleanup recorded no errors.

Linux packaging, self-extracting help, fresh-directory extraction, local installation, installed version and actual menu launch all passed. The extracted and installed executables matched the production binary byte for byte. These first-pass package hashes appear in the summary; they identify the temporary bundle used before this report was included. Final delivery is repackaged with this report and separately subjected to extraction/installation/menu checks and archive-content comparison. Final hashes and that smoke-test proof accompany the delivery as `SHA256SUMS` and `DELIVERY-VERIFICATION.json`.

Publication checks scan source, binary text, decompressed archives, archive metadata and the self-extracting payload for private identifiers, machine-specific home paths, private keys and unwanted artifacts. Visible ir_spoof credit and dependency license/authorship notices are retained. Local build caches, original binaries and private reverse-engineering/VM harness files are excluded from release bundles.

## Reproduce

On Linux with a current Rust toolchain:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
cargo test --locked --release --all-targets
test_bin=$(find target/release/deps -maxdepth 1 -type f -name 'tun_linux-*' -executable -print -quit)
sudo "$test_bin" --ignored --nocapture
bash scripts/package-linux.sh target/release/dagger-rs dist
bash dist/dagger-rs-linux-x86_64.run --help
```

The previous [namespace/service report](testing-0.2.0.md) provides the additional privileged protocol-matrix commands. Runtime tests use local test identities and explicitly selected peers. Test evidence is retained verbatim rather than adding attribution comments to machine-generated logs or changing historical records.

These checks establish local Rust-to-Rust functionality. The independent Noise framing intentionally changes original peer interoperability. Raw outer transport is IPv4 with one configured peer/pool one; BIP uses the packet-socket path. A local bounded governor replaces the original global host-memory classification. Local HTTP proxy tests do not establish every external CDN, and spoofed delivery depends on the network path. See [reconstruction](reconstruction.md), [raw protocols](raw-protocol.md), [raw TUN](raw-tun.md), [Quantum tuning](quantum-tuning.md) and [XHTTP](xhttp.md) for the boundaries.
