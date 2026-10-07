# Local patch

This is `tokio_kcp` version 0.9.8 from Matrix-Zhang/tokio_kcp, distributed under the MIT license in `LICENSE`. Its upstream README and attribution are retained.

The patch adds `KcpSocket::set_window_size(send, receive)` and `KcpStream::shared_session()`, and reexports the existing public `KcpSession` type. The caller holds the existing session lock before changing the underlying KCP windows. The owned session handle lets dagger-rs run one independently timed controller task per explicitly enabled adaptive stream; a drop guard aborts that task. Dropping `KcpStream` still closes its session. These API additions do not change the KCP wire format or congestion algorithm.

Upstream: https://github.com/Matrix-Zhang/tokio_kcp

Local integration changes for the Dagger Rust rewrite by [ir_spoof](https://t.me/ir_spoof). Upstream ownership and license remain unchanged.
<!-- Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f -->
