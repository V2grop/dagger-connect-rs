# Quantum tuning

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

Quantum, Quantum Gaming and Quantum+ use a KCP stream with the recovered 10-data/1-parity FEC envelope. Raw Quantum uses the recovered TCP camouflage header; Quantum+ uses UDP. Noise authenticates and encrypts the stream using locally pinned public keys. This source does not implement the original PSK header cipher or vendor verification, and is not wire-compatible with DaggerConnect.

Each listener or client path accepts this optional `quantum` object:

```json
{
  "mtu": 1350,
  "peer_mtu": null,
  "profile": "default",
  "knock": true,
  "knock_timeout_ms": 3000,
  "max_peers": 256,
  "sndwnd": 1024,
  "rcvwnd": 1024,
  "socket_buf_bytes": 4194304,
  "nodelay": null,
  "interval_ms": null,
  "resend": null,
  "congestion_control": null,
  "write_delay": null,
  "ack_no_delay": null,
  "tuner": {
    "mode": "off",
    "min_buffer_bytes": 262144,
    "max_buffer_bytes": 16777216,
    "min_window_bytes": 262144,
    "max_window_bytes": 16777216,
    "memory_budget_mb": 128,
    "queue_delay_ms": null
  }
}
```

`sndwnd` and `rcvwnd` are KCP segment counts, accepted from 128 through 8192. Static defaults are 1024. Gaming caps each window at 1024 and enables immediate writes and KCP nodelay; normal raw Quantum delays writes and leaves nodelay disabled. Both raw profiles default to a 10 ms update interval, fast retransmit after 2 ACKs, disabled KCP congestion control, and delayed input ACKs. Quantum+ defaults to immediate writes and input ACKs. Raw carriers do not use the UDP knock.

The six nullable timing fields override these defaults. `nodelay` selects KCP's reduced retransmission timing. `interval_ms` accepts 10..5000 ms. `resend` accepts 0..255 ACKs; zero disables fast retransmission. `congestion_control: true` enables KCP congestion control, mapping to the original `nc=0`. `write_delay: true` batches writes until the update tick; false flushes after each write. `ack_no_delay: true` immediately flushes input ACKs. These overrides apply to Quantum+ and raw Quantum; they do not lift the gaming window cap. For the recovered gaming branch that delays writes when its congestion setting is `on`, use `write_delay: true` explicitly.

`mtu` is the configured envelope size. Optional raw-only `peer_mtu` applies the recovered per-peer limit, taking `min(mtu, peer_mtu)` before the raw reserve. The raw effective envelope is `min(max(min(mtu, peer_mtu) - 100, 512), 1500)`; without `peer_mtu`, use `mtu` alone. UDP uses `min(mtu, 1500)`. Both MTU settings accept 512..9000. The clear FEC envelope takes 8 bytes, which are subtracted before configuring the KCP engine. This is a local configured limit, not a negotiated network probe. Raw capacity is checked against the actual interface and selected outer format. `socket_buf_bytes` sets this process's receive/send socket buffers from 256 KiB through 64 MiB; it does not change host sysctls. In Quantum raw mode this field controls the same raw descriptor otherwise configured through `raw.sock_buf`. Direct unreliable TUN still uses `raw.sock_buf`.

Set `tuner.mode` to `auto` to enable local adaptation. `off` is the default and uses the fixed windows and socket buffers above without a controller task. Auto begins with the minimum configured window-byte budget, then measures stream byte rates and matching KCP ACKs. Each stream owns one local controller task with an independent Tokio timer, so it runs while application I/O is idle or read/write halves wait on different tasks. Dropping the stream aborts its controller. It maintains a 0.4 new-sample throughput EWMA, updates socket buffer targets every 1 second, and updates live KCP windows every 2 seconds. Missed ticks are skipped rather than replayed in a burst, and rates use actual elapsed time. ACK samples require matching conversation, sequence and echoed timestamp; outstanding stamps are capped at 8192 per conversation and expire after 60 seconds. RTT estimates also expire after 60 seconds without a sample; the minimum RTT baseline refreshes after 600 seconds. No external probe or hostname is needed.

The recovered window policy uses twice the estimated bandwidth-delay product plus 256 segments for normal mode or 128 for gaming. At high utilization it requests at least 25% growth; one update can grow at most 2 times. Queue inflation above 1.5 times baseline plus 20 ms caps normal growth at 75% of the current window. Gaming uses 1 times baseline plus 15 ms. `queue_delay_ms` overrides the 20/15 ms allowance. Outside urgent inflation or a shrinking memory budget, lower targets are held for 9 window updates, then shrink by at most one eighth per update. Windows derived from byte budgets are clamped to 256..8192 segments, with a 1024 gaming cap. Auto mode requires `max_window_bytes` to cover the 256-segment minimum at the configured KCP MTU.

Buffer sizing is an independent local policy: four bandwidth-delay budgets, bounded by the configured buffer ranges. Each conversation produces a buffer request; the listener adds these requests and applies one common target to its shared network socket. A quiet peer therefore cannot overwrite a busier peer's request. The aggregate socket target is capped by `max_buffer_bytes`, 64 MiB and one quarter of `memory_budget_mb`. Client paths use the same rule with one conversation. Relay requests are removed from the bounded registry on 60-second inactivity expiry or listener teardown. Dropping an accepted stream stops its controller immediately; its relay reservation can remain until expiry.

`memory_budget_mb` is shared across active relay conversations, reserving targets for two socket queues and two KCP windows per conversation. Auto admission is additionally bounded by the minimum allocation budget, so adding sessions cannot multiply that budget without bound. Minimum byte ranges, the 256-segment floor and the configured memory budget must permit at least one conversation. Window targets can take up to 2 seconds to adjust when the peer count changes. This is a budget for queue/window targets, not a hard RSS limit: Linux may double or clamp `SO_RCVBUF`/`SO_SNDBUF`, and application/KCP bookkeeping consumes additional memory. The original host-memory classification and global memory policy have not been claimed as identical.

The policy constants and timing were recovered from the supplied v4.2.8 ELF: KCP configuration at `0xa56460`, throughput update at `0x929760`, window target at `0x976d80`, and the per-connection controller at `0x977640`. The Tokio KCP dependency is vendored under its MIT license with a locked window setter and an owned session getter for the guarded controller; see [`vendor/tokio_kcp/DAGGER-RS-PATCH.md`](../vendor/tokio_kcp/DAGGER-RS-PATCH.md).
