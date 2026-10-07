# Historical 0.1.0 verification

This is the previous release report, retained for history. See [the current report](testing.md) for 0.2.0.

# Verification report

Tested on 2026-10-06. These results describe the executable and scenarios below; they do not claim compatibility with the original application or constitute an independent security audit.

## Tested artifact and environment

- Binary: `dagger-rs 0.1.0`, Linux x86-64, statically linked musl target.
- Size: **6,058,152 bytes**.
- SHA-256: **`5e280d6340558b1439b8804a643ea9c3d4e8196b553d240522eba5c87c7baea5`**.
- Compiler: Rust/Cargo **1.99.0**, target `x86_64-unknown-linux-musl`, release profile. Cross compilation used Rust's bundled LLD and Zig 0.17.0 for C dependencies; the resulting Linux executables were actually executed in Linux.
- Runtime: **Alpine Linux 3.24.2**, kernel **6.18.52-0-virt**, QEMU **11.1.0** with software CPU emulation, two virtual CPUs and 3 GiB RAM. This was a local isolated VM, not a mocked socket environment.
- Privileged tests used Linux root, `/dev/net/tun`, actual network namespaces, veth links, iproute2, Python 3 and ping. Final inspection found no remaining test namespaces or TUN interfaces.
- ELF inspection confirmed an x86-64 static PIE executable with no interpreter and no `DT_NEEDED` shared-library dependencies. Formatting checks and Linux all-target Clippy with warnings denied passed.
- Release source paths were remapped before the final test run. A binary scan found no build-user name, Windows home-directory paths, or original vendor domains in the final artifact.

## Final release results

**All 16 Rust tests passed**, including the normally ignored privileged test, plus **nine complete TUN carrier scenarios**.

| Group | Result | Coverage |
| --- | --- | --- |
| Library unit tests | 11 passed | Configuration/key/certificate validation, key non-overwrite, pinned Noise authentication, rejected unauthorized peer, ciphertext modification/replay rejection, frame roundtrips and malformed-frame rejection. |
| Carrier integration tests | 2 passed | All nine carriers each forwarded 1,048,577 TCP bytes with half-close, 40 UDP datagrams including empty payloads, and SOCKS5 TCP; additional checks rejected wrong TLS trust, wrong TLS server names and wrong HTTP paths. |
| Concurrent forwarding integration | 1 passed | Four concurrent 1,048,577-byte TCP roundtrips with half-close, UDP including empty datagrams, SOCKS5, exact-target allowlist rejection, and automatic recovery after server restart. |
| IP packet validation | 1 passed | IPv4/IPv6 header and packet length checks, unsupported IP versions, truncated packets and MTU bounds. |
| Privileged TUN lifecycle | 1 passed | Real TUN creation, refusal to attach to an existing interface, preservation of the original interface after refusal, and interface removal when the owning descriptor closes. |
| CLI test harness | 0 tests | Harness completed successfully; CLI behavior is additionally exercised by namespace/setup tests. |
| TUN network namespace matrix | 9 scenarios passed | One scenario per `tcp`, `http`, `https`, `ws`, `wss`, `xhttp`, `xhttps`, `dc6`, `kcp`. Each used two distinct namespaces and a veth underlay, verified IPv4 and IPv6 ICMP and UDP payloads of 0, 1, 512 and 1300 bytes, restarted the server while retaining the client, and verified interface cleanup. DC6 used an IPv6 underlay. TLS variants used a locally generated explicitly trusted certificate. |

The carrier integration harness completed in **7.83 seconds** and the concurrent forwarding test in **4.17 seconds** in the final release run. Sanitized output is saved in [test-results](test-results/), including [unit tests](test-results/unit.txt), [carrier tests](test-results/carriers.txt), [concurrent forwarding](test-results/forwarding.txt), [privileged TUN](test-results/tun-privileged.txt), and one `tun-<carrier>.txt` per namespace scenario. The logs contain no private keys or host-specific absolute filesystem paths.

## Ubuntu installation and systemd verification

The same release hash was tested in a second local QEMU VM running **Ubuntu 24.04.5 LTS**, kernel **6.8.0-142-generic**, systemd **255.4-1ubuntu8.17**, with two virtual CPUs and 1.5 GiB RAM. The following checks passed:

- Installation from the local release bundle, key generation and certificate generation.
- Interactive setup generated server and client services running under the dedicated `dagger-rs` account. `systemd-analyze verify` accepted the units, and both reached `active/running`.
- A 257,024-byte TCP exchange with EOF passed through the services. Restarting the actual server service exercised client reconnection, after which a second exchange passed. Stopping both services left them inactive.
- A TUN server service ran with `CAP_NET_ADMIN` and the required `AF_NETLINK` access. A client in a separate network namespace connected over a veth underlay; the test created real `dgs0`/`dgc0` interfaces, exchanged IPv4 ICMP packets, and stopped cleanly.
- Interactive menus independently generated a TUN server configuration and a WebSocket client configuration. Certificate generation, TLS server configuration and configuration backup were exercised.

Evidence: [environment](test-results/systemd-environment.txt), [port-forwarding services](test-results/systemd-release-ports.txt), [TUN service](test-results/systemd-release-tun.txt), and [interactive menus](test-results/systemd-interactive-menus.txt). These checks cover **system-level systemd services**; per-user systemd services were not tested.

## Failures found during development

The initial IPv6 TUN scenario failed because Linux's IPv6 point-to-point address configuration did not automatically create the required remote host route. The implementation now adds an explicit peer `/128` route on its newly created interface. The complete IPv4/IPv6 matrix passed after that fix; teardown also removes the interface-bound route.

An early script execution failed because of CRLF line endings. Shell scripts were normalized to LF and repository attributes now preserve LF before the successful release installation and service checks.

An earlier **unoptimized debug** run under software CPU emulation failed the concurrent forwarding check and the KCP bulk-transfer deadline. Those tests used a 3-second dead-peer limit and a 15-second bulk deadline. The final optimized release runs passed the same payload sizes and deadlines. Slow emulation and unoptimized cryptography are a plausible explanation, but the earlier failure was not isolated conclusively; the debug failures are not represented as passing results. The final verified release is the artifact identified above.

## Reproduce on Linux

Run ordinary tests as an unprivileged user:

```sh
cargo build --release --locked
cargo test --release --locked --all-targets
```

Use an isolated Linux VM for privileged tests; these create interfaces, routes and namespaces inside that VM. Install Bash, Python 3, iproute2 and ping and ensure `/dev/net/tun` is available. Run the already-built privileged harness without rebuilding dependencies as root:

```sh
test_bin=$(find target/release/deps -maxdepth 1 -type f -name 'tun_linux-*' -executable -print -quit)
sudo "$test_bin" --ignored --nocapture

for carrier in tcp http https ws wss xhttp xhttps dc6 kcp; do
  sudo bash tests/tun_vm.sh "$(realpath target/release/dagger-rs)" "$carrier"
done
```

The namespace script cleans up its own temporary identities, processes, interfaces and namespaces on exit. It does not alter the host's default route. Service installation tests, when run, should likewise use a disposable Linux VM.
