//! Reliable KCP byte streams over UDP. Authentication is provided by the same
//! Noise handshake as every other carrier, before streams enter the engine.
use crate::config::{Listener, Transport};
use anyhow::{Context, Result};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_kcp::{KcpConfig, KcpListener, KcpStream};

pub fn configuration() -> KcpConfig {
    KcpConfig {
        mtu: 1400,
        stream: true,
        wnd_size: (128, 128),
        session_expire: Duration::from_secs(60),
        flush_write: true,
        ..KcpConfig::default()
    }
}

pub async fn connect(addr: &str) -> Result<KcpStream> {
    // KCP creates a UDP session without a network handshake; the enclosing
    // Noise handshake supplies authentication and the connection deadline.
    let address = tokio::net::lookup_host(addr)
        .await
        .context("resolve KCP peer")?
        .next()
        .context("KCP peer has no address")?;
    KcpStream::connect(&configuration(), address)
        .await
        .context("open KCP session")
}

pub enum Incoming {
    Tcp(TcpStream),
    Kcp(KcpStream),
    Xhttp(crate::carrier::BoxIo),
    Quantum(crate::carrier::BoxIo),
}

pub enum Acceptor {
    Tcp(TcpListener),
    Kcp(KcpListener),
    Xhttp(crate::xhttp_carrier::Acceptor),
    Quantum(crate::quantum_carrier::QuantumListener),
}

impl Acceptor {
    pub async fn bind(listener: &Listener) -> Result<Self> {
        if matches!(
            listener.transport,
            Transport::QuantumPlus | Transport::QuantumGaming | Transport::Quantum
        ) {
            Ok(Self::Quantum(
                crate::quantum_carrier::QuantumListener::bind(listener).await?,
            ))
        } else if matches!(listener.transport, Transport::Xhttp | Transport::Xhttps) {
            Ok(Self::Xhttp(
                crate::xhttp_carrier::Acceptor::bind(listener).await?,
            ))
        } else if listener.transport == Transport::Kcp {
            Ok(Self::Kcp(
                KcpListener::bind(configuration(), &listener.addr)
                    .await
                    .context("bind KCP UDP listener")?,
            ))
        } else {
            Ok(Self::Tcp(
                TcpListener::bind(&listener.addr)
                    .await
                    .context("bind TCP listener")?,
            ))
        }
    }

    pub async fn accept(&mut self) -> Result<Incoming> {
        match self {
            Self::Tcp(listener) => Ok(Incoming::Tcp(listener.accept().await?.0)),
            Self::Kcp(listener) => Ok(Incoming::Kcp(listener.accept().await?.0)),
            Self::Xhttp(listener) => Ok(Incoming::Xhttp(listener.accept().await?)),
            Self::Quantum(listener) => Ok(Incoming::Quantum(listener.accept().await?)),
        }
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
