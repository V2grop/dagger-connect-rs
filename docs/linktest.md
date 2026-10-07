# Explicit-peer link diagnostics

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

The link tester contacts only the peer IP and ports supplied by the operator. It does not load a tunnel configuration, derive ports from a PSK, resolve hostnames, select public websites, or contact DaggerConnect services. Both machines must run this Rust implementation of the diagnostic protocol.

The original 4.2.8 CLI exposes `linktest listen|probe`, a reverse listener port, extra ports, timed probes, a quick mode and JSON output. Its help also selects public website/SNI and PSK-derived port defaults. The Rust implementation preserves the local diagnostic purpose while requiring explicit endpoints. It uses an independent bounded diagnostic protocol; it does not claim compatibility with the original link-test wire format.

## Measurements

Each probe establishes a TCP session with the listener, then measures:

- TCP echo round-trip time and exact 4 KiB payload integrity.
- UDP echo round-trip time, duplicate suppression, packet loss and exact payload integrity. Normal mode sends 16 datagrams; quick mode sends four.
- Reverse TCP and UDP connectivity: the remote listener sends probes to a TCP/UDP pair temporarily bound on the probing machine.
- TCP upload and download throughput using sequenced 32 KiB blocks with deterministic contents. The receiver checks the sequence and every byte.
- TCP and UDP echo results for each explicitly listed extra remote port.

The TCP and UDP listeners share the selected base port. Extra ports must be explicitly enabled on both machines. The reverse listener also uses one port for both TCP and UDP. A reverse port of zero asks the OS to allocate a free port. A firewall or NAT must permit that inbound port for reverse tests to pass; the tester never modifies either.

Throughput is transferred bytes divided by measured elapsed time, reported as decimal megabits per second. It includes TCP framing, scheduling and final response delay in the elapsed interval, so it is an application measurement rather than a line-rate claim. A transfer stops after the selected duration or 256 MiB, whichever comes first. Quick mode caps each direction at 250 ms. UDP loss is reported directly; the diagnostic UDP path does not retransmit.

`LinkReport::passed()` requires successful forward/reverse TCP checks, no UDP loss or integrity failures, nonempty intact upload/download transfers, and success on every requested extra port. Individual failures remain in the serializable report so a closed port or one-way route can be identified.

## Command line

Start an explicitly pinned listener on the remote machine:

```sh
dagger-rs linktest listen --bind 192.0.2.10:39000 --peer 192.0.2.20 --extra-ports 39001
```

Probe it from the other machine:

```sh
dagger-rs linktest probe --peer 192.0.2.10:39000 --bind 192.0.2.20 --local-port 39002 --extra-ports 39001 --seconds 4 --save link-report.json
```

Replace the documentation addresses with your own peers. Completed probes print JSON; a failed individual check returns a nonzero exit status after printing the report. Invalid arguments or failure to establish the diagnostic session return an error before a report is available. `--quick` limits probe traffic and duration. `--help` lists listener timeout and probe timeout controls. No public peer, website or extra port is selected by default.

## Library API

```rust,no_run
use dagger_rs::linktest::{self, ListenOptions, ProbeOptions};
use std::time::Duration;

# async fn example() -> anyhow::Result<()> {
// On the listening machine: these example addresses are documentation ranges.
linktest::listen(ListenOptions {
    bind: "192.0.2.10:39000".parse()?,
    expected_peer: "192.0.2.20".parse()?,
    extra_ports: vec![39001],
    wait: Duration::from_secs(60),
    keep: false,
}).await?;

// On the probing machine: bind pins outgoing TCP/UDP source addresses too.
let report = linktest::probe(ProbeOptions {
    peer: "192.0.2.10:39000".parse()?,
    bind: "192.0.2.20".parse()?,
    local_port: 39002,
    extra_ports: vec![39001],
    seconds: Duration::from_secs(4),
    timeout: Duration::from_secs(5),
    quick: false,
}).await?;
println!("{}", serde_json::to_string_pretty(&report)?);
# Ok(())
# }
```

The listener requires one explicit expected peer IP. The peer and local bind must use the same IP family. Listen time is 1–3600 seconds, throughput duration is 50 ms–30 seconds, and each probe timeout is 50 ms–60 seconds. At most 16 unique nonzero extra ports are accepted. `keep: false` stops the listener after an authorized probe sends its final request; `keep: true` retains it until the listen time expires. Dropping or aborting the listener future closes all listening sockets and spawned connection handlers.

## Bounds and session authorization

The TCP session handshake generates a random 256-bit bearer token. The listener binds that token to the expected source IP, limits its lifetime to three minutes, and permits at most 128 TCP operations and 4096 UDP echoes per token. There are at most 64 live tokens and 16 concurrent TCP handlers. Unknown, expired, wrong-source and oversized UDP requests receive no response. UDP responses have exactly the request length, with an 8 KiB request limit. TCP frames have a 64 KiB limit, transfer frames are bounded to 256 MiB per operation, and a handler expires after 90 seconds. Reverse requests can target only the TCP connection's peer IP and a nonzero requested port, with at most 32 UDP datagrams.

The transient token prevents unauthenticated UDP reflection and accidental cross-session traffic. It is exchanged over plain TCP; it does not provide encryption or cryptographic peer identity against a network observer. Diagnostic payloads contain test patterns, not forwarded application traffic. The tunnel's separate Noise authentication and encryption remain independent of this diagnostic session. Tokens are never included in reports or logs.

Raw GRE/IPIP, HTTP, TLS/SNI and WebSocket reachability are not probed by this module. Raw carrier coverage lives in `tests/raw_vm.sh`; encrypted network carrier coverage lives in `tests/carriers.rs`. A result from ordinary TCP/UDP diagnostics should not be interpreted as success for those different carriers.

## Verification

`cargo test --test linktest` creates actual local sockets and checks IPv4 and IPv6 in both directions, upload/download integrity, an explicit extra port, the selected `127.0.0.2` source IP, unknown-token rejection, oversized datagram rejection and a closed-port failure report. It requires no root privileges, websites, DNS, TUN devices or host routing changes. The Linux IPv6 loopback interface must be available for the IPv6 case.

Community: <https://t.me/ir_spoof> (display only; never contacted by diagnostics).
