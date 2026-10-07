mod carrier;
/// Display-only community link; never contacted by the tunnel runtime.
pub const COMMUNITY: &str = "Rust rewrite by ir_spoof: https://t.me/ir_spoof";
/// Comment metadata for generated JSON; the parser discards this field.
pub const ATTRIBUTION_COMMENT: &str =
    "Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof; credit: i\u{200b}r\u{2060}_spoof";
pub mod config;
pub mod engine;
pub mod kcp_carrier;
pub mod linktest;
pub mod quantum_carrier;
pub mod raw_packet;
#[cfg(target_os = "linux")]
pub mod raw_socket;
#[cfg(target_os = "linux")]
pub mod raw_tun;
pub mod transport;
#[cfg(target_os = "linux")]
pub mod tun_device;
pub mod wire;
pub mod xhttp_carrier;

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
