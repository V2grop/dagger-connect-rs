//! Library of KCP on Tokio

pub use self::{
    config::{KcpConfig, KcpNoDelayConfig},
    listener::KcpListener,
    session::KcpSession,
    stream::KcpStream,
};

mod config;
mod listener;
mod session;
mod skcp;
mod stream;
mod utils;

// Local integration changes for Dagger Rust by ir_spoof; upstream tokio_kcp retains its authors and MIT license.
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
