# XHTTP uploads and HTTP proxies

Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof).
<!-- Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f. Attribution marker; see the visible credit above. -->

The Rust core supports stream-up, packet-up, and automatic detection of buffered uploads. Packet-up carries the encrypted tunnel through finite HTTP POST requests and a separate streaming GET response. A proxy can forward those requests without understanding Noise or the tunnel's streams. Only the addresses, HTTP Host, TLS name and certificate trust configured by the operator are used.

These capabilities follow the behavior described by the upstream XHTTP guide. The session URLs, framing and authentication belong to dagger-rs. Interoperation with the proprietary DaggerConnect core is not claimed.

## Upload modes

Set `xhttp` inside the physical dialer's listener/path entry:

```json
"xhttp": {
  "mode": "packet-up",
  "host": "tunnel.example.com",
  "edge_addrs": ["203.0.113.10:8443", "203.0.113.11:8443"],
  "up_max_bytes": 65536,
  "up_concurrency": 8,
  "buffer_bytes": 32768,
  "user_agent": "dagger-rs",
  "pad_min": 8,
  "pad_max": 48,
  "socket_buf_bytes": 131072,
  "probe_ms": 8000,
  "session_timeout_sec": 40
}
```

| Option | Default | Behavior |
|---|---|---|
| `mode` | `stream-up` | `stream-up`, `packet-up`, or `auto` |
| `host` | address from `addr` | HTTP Host authority and, for TLS, certificate/SNI hostname; a port is removed from the TLS name |
| `edge_addrs` | empty | Explicit addresses tried in order instead of `addr`; maximum 16, no edge discovery or scanning |
| `up_max_bytes` | 65536 | Maximum finite POST body, 1024–1048576 bytes; receiver must allow at least the sender's maximum |
| `up_concurrency` | 2 | Number of concurrent finite POST workers, 1–16 |
| `buffer_bytes` | 32768 | Bounded byte-stream buffer, 1024–1048576 bytes |
| `user_agent` | `dagger-rs` | Operator supplied ASCII HTTP User-Agent, 1–256 bytes; control characters are rejected |
| `pad_min` / `pad_max` | 0 / 0 | Random alphanumeric `?v=` query padding length, 0–4096 bytes; equal zero disables padding |
| `socket_buf_bytes` | 1048576 | Requested Linux send/receive socket buffers, 65536–67108864 bytes; kernel limits may cap the applied size |
| `probe_ms` | 8000 | Auto-mode streaming probe deadline; capped at half the enclosing dial deadline to leave time for fallback |
| `session_timeout_sec` | 40 | Upload/header idle deadline; maximum 3600 seconds |

The listener accepts either upload shape. Its `mode` does not restrict incoming sessions. Its body limit, buffer, socket buffer and timeout still apply. On the physical dialer, `mode` chooses the upload shape. User-Agent and padding are applied to every outgoing XHTTP request. Padding is validated and removed before routing an incoming request; unknown query parameters are rejected. These fields contain no automatic operating-system, user-account or device identifier.

`stream-up` preserves the original dagger-rs long, bidirectional, chunked POST on one connection for existing configurations. It requires a path that forwards unfinished request bodies. `auto` probes the path with an unfinished chunked request. An immediate acknowledgement selects streaming uploads over separate upload/download connections. If the request is buffered or the probe fails, the core starts a new packet-up session. `packet-up` skips this probe and always sends numbered finite requests. The implementation uses HTTP/1.1; packet uploads require connections separate from the long download response. No HTTP/2 multiplexing or `separate_conns=false` packet mode is claimed.

Failed configured edges receive a 60-second local cooldown. The core retries the configured list immediately if every edge is cooling down. Each TCP/TLS attempt receives a share of the dial deadline, capped at three seconds. Active sessions remain on the edge that accepted them; reconnecting creates a fresh session. A failed or ambiguously acknowledged POST ends the session rather than replaying encrypted stream bytes.

## CDN direction reversal

Set `"reverse": true` in both root configurations. The logical server keeps its port mappings and SOCKS listener, but physically dials the HTTP edge. The logical client becomes the HTTP origin and keeps its target allowlist. Noise authentication keeps the same logical server/client roles.

For the logical server, use the listener's `addr` as the edge endpoint. Put certificate trust and the expected TLS name in `ca_file` and `server_name`. The following listener fragment uses documentation-only example addresses; replace them with your own endpoint and domain:

```json
"reverse": true,
"listeners": [{
  "addr": "203.0.113.10:8443",
  "transport": "xhttps",
  "http_path": "/tunnel-api",
  "server_name": "tunnel.example.com",
  "xhttp": {
    "mode": "packet-up",
    "host": "tunnel.example.com",
    "edge_addrs": ["203.0.113.10:8443", "203.0.113.11:8443"],
    "up_concurrency": 8
  },
  "maps": [{"type":"tcp", "bind":"127.0.0.1:8080", "target":"127.0.0.1:8000"}]
}]
```

For the logical client, use the path's `addr` as the origin bind address and provide the TLS certificate/key there:

```json
"reverse": true,
"paths": [{
  "addr": "0.0.0.0:8443",
  "transport": "xhttps",
  "http_path": "/tunnel-api",
  "cert_file": "origin.pem",
  "key_file": "origin-key.pem",
  "server_public_key": "REPLACE_WITH_THE_LOGICAL_SERVER_PUBLIC_KEY"
}],
"allowed_targets": ["127.0.0.1:8000"]
```

These fragments accompany the usual `mode`, private key and pinned peer settings. They are not complete configuration files. For direct traffic without reversal, the logical server listens and the logical client dials as usual.

Configure your own reverse proxy/CDN to route the Host and path to the origin, preserve request bodies, disable caching, and stream the download response. The core sends `Cache-Control: no-store, no-transform` and `X-Accel-Buffering: no`. TLS verification stays enabled; no insecure TLS switch is provided. A custom origin CA can be supplied explicitly. An HTTP proxy/CDN must support streaming responses; finite uploads do not remove that requirement. External CDN deployments have not been verified merely by the local tests.

## Packet framing and limits

A 32-byte random session identifier is hex encoded. The downloader starts `GET <http_path>/<session>` with an explicit upload-mode header. Packet uploads use `POST <http_path>/<session>/<sequence>` and a `Content-Length`; sequence numbers start at zero. An empty body at the next sequence ends upload. Streaming uploads use a chunked POST to the session URL. Both shapes carry the same encrypted Noise byte stream.

The receiver reorders a window of 16 uploads. Duplicates, old sequence numbers and sequence numbers outside that window receive HTTP 409. It acknowledges a body only after the bounded tunnel stream has accepted its bytes. Upload queues, per-session request slots and global request/session counts are bounded. An oversized finite request receives HTTP 413 before body allocation. An absent session receives HTTP 410. Dropping a tunnel aborts its bridge and removes the session, including during handshake failure.

Download chunks may be split or combined by an HTTP proxy. The decoder uses a fixed 16 KiB copy buffer, accepts ordinary chunk extensions/trailers, and caps an individual proxy chunk at 16 MiB. Empty chunked upload acknowledgements and `Connection: close` responses are supported. Header/trailer sizes remain bounded. Incoming HTTP/1.1 requests require a valid Host authority; ambiguous length/encoding headers and malformed numeric fields are rejected. Wire records are still authenticated by Noise; the HTTP session identifier does not replace peer authentication.

## Local verification

`tests/xhttp.rs` exercises packet-up over HTTP and verified TLS in both physical directions, transferring 1,048,577 TCP bytes with half-close, UDP datagrams of 0–16384 bytes, and SOCKS TCP data. Each case supplies an unavailable edge followed by a working edge; TLS cases also try an edge with the wrong certificate. Host/SNI override and logical pinned identities are checked in both directions. The harness also checks auto fallback through a local proxy that deliberately buffers unfinished chunked requests, in both directions, and direct auto streaming. The proxy verifies the configured User-Agent and 8–48-byte query padding. Unit tests exercise out-of-order uploads, duplicate/far-future rejection, bounded-stream acknowledgement backpressure, invalid chunk/HTTP framing, cancellation cleanup, proxy acknowledgement/connection-close behavior, padding bounds and header-injection rejection.

Test declarations describe what the harness checks; the current release's recorded Linux execution results are in `docs/testing.md` and its attached logs.

Community: https://t.me/ir_spoof — display only; the runtime does not contact this link.
