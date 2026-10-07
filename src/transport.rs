//! Authenticated, encrypted TCP records with locally pinned X25519 identities.
//! A failed/cancelled send or receive requires dropping the connection; do not retry a record.
use crate::wire::Frame;
use crate::{
    carrier::{BoxIo, client_carrier, server_carrier},
    config::{ClientPath, Listener},
};
use anyhow::{Context, Result, anyhow, ensure};
use snow::{Builder, TransportState};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

pub const NOISE_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
const PROLOGUE: &[u8] = b"dagger-rs/reverse-tunnel/v1";
const MAX_RECORD: usize = 65_535;

pub struct SecureConnection {
    stream: BoxIo,
    state: TransportState,
}

pub struct SecureReader {
    stream: tokio::io::ReadHalf<BoxIo>,
    state: Arc<Mutex<TransportState>>,
}

pub struct SecureWriter {
    stream: tokio::io::WriteHalf<BoxIo>,
    state: Arc<Mutex<TransportState>>,
}

impl SecureConnection {
    pub fn split(self) -> (SecureReader, SecureWriter) {
        let (read, write) = tokio::io::split(self.stream);
        let state = Arc::new(Mutex::new(self.state));
        (
            SecureReader {
                stream: read,
                state: state.clone(),
            },
            SecureWriter {
                stream: write,
                state,
            },
        )
    }
}

impl SecureReader {
    pub async fn recv(&mut self) -> Result<Frame> {
        let encrypted = read_record(&mut self.stream).await?;
        ensure!(encrypted.len() >= 16, "truncated encrypted record");
        let mut plaintext = vec![0; encrypted.len()];
        let size = self
            .state
            .lock()
            .map_err(|_| anyhow!("cipher state poisoned"))?
            .read_message(&encrypted, &mut plaintext)
            .context("record authentication failed")?;
        Frame::decode(&plaintext[..size])
    }
}

impl SecureWriter {
    pub async fn send(&mut self, frame: &Frame) -> Result<()> {
        let plaintext = frame.encode()?;
        let mut encrypted = vec![0; plaintext.len() + 16];
        let size = self
            .state
            .lock()
            .map_err(|_| anyhow!("cipher state poisoned"))?
            .write_message(&plaintext, &mut encrypted)
            .context("record encryption failed")?;
        write_record(&mut self.stream, &encrypted[..size]).await
    }
}

pub async fn connect(
    addr: &str,
    private_key: &[u8; 32],
    server_public_key: &[u8; 32],
    timeout: Duration,
) -> Result<SecureConnection> {
    tokio::time::timeout(timeout, async {
        let stream = TcpStream::connect(addr)
            .await
            .context("connect to configured peer")?;
        stream.set_nodelay(true)?;
        initiate(Box::new(stream), private_key, server_public_key).await
    })
    .await
    .context("peer connection/handshake timed out")?
}

pub async fn connect_path(path: &ClientPath, private_key: &[u8; 32]) -> Result<SecureConnection> {
    let server_key = crate::config::decode_key(&path.server_public_key)?;
    tokio::time::timeout(Duration::from_secs(path.dial_timeout), async {
        let stream = client_carrier(path).await?;
        initiate(stream, private_key, &server_key).await
    })
    .await
    .context("peer connection/handshake timed out")?
}

async fn initiate(
    mut stream: BoxIo,
    private_key: &[u8; 32],
    server_public_key: &[u8; 32],
) -> Result<SecureConnection> {
    let mut handshake = Builder::new(NOISE_PATTERN.parse()?)
        .local_private_key(private_key)?
        .remote_public_key(server_public_key)?
        .prologue(PROLOGUE)?
        .build_initiator()?;
    let mut output = vec![0; MAX_RECORD];
    let size = handshake.write_message(&[], &mut output)?;
    write_record(&mut stream, &output[..size]).await?;
    let response = read_record(&mut stream).await?;
    let size = handshake
        .read_message(&response, &mut output)
        .context("peer handshake authentication failed")?;
    ensure!(
        size == 0 && handshake.is_handshake_finished(),
        "unexpected handshake payload"
    );
    Ok(SecureConnection {
        stream,
        state: handshake.into_transport_mode()?,
    })
}

pub async fn accept(
    stream: TcpStream,
    private_key: &[u8; 32],
    allowed_peers: &[[u8; 32]],
    timeout: Duration,
) -> Result<SecureConnection> {
    stream.set_nodelay(true)?;
    tokio::time::timeout(
        timeout,
        respond(Box::new(stream), private_key, allowed_peers),
    )
    .await
    .context("peer handshake timed out")?
}

pub async fn accept_listener(
    stream: crate::kcp_carrier::Incoming,
    listener: &Listener,
    private_key: &[u8; 32],
    allowed_peers: &[[u8; 32]],
    timeout: Duration,
) -> Result<SecureConnection> {
    tokio::time::timeout(timeout, async {
        let stream: BoxIo = match stream {
            crate::kcp_carrier::Incoming::Tcp(stream) => server_carrier(stream, listener).await?,
            crate::kcp_carrier::Incoming::Kcp(stream) => Box::new(stream),
            crate::kcp_carrier::Incoming::Xhttp(stream) => stream,
            crate::kcp_carrier::Incoming::Quantum(stream) => stream,
        };
        respond(stream, private_key, allowed_peers).await
    })
    .await
    .context("peer handshake timed out")?
}

pub async fn connect_responder(
    listener: &Listener,
    private: &[u8; 32],
    peers: &[[u8; 32]],
) -> Result<SecureConnection> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let stream = client_carrier(&listener.dial_path()).await?;
        respond(stream, private, peers).await
    })
    .await
    .context("reverse peer handshake timed out")?
}

pub async fn accept_initiator(
    incoming: crate::kcp_carrier::Incoming,
    path: &ClientPath,
    private: &[u8; 32],
) -> Result<SecureConnection> {
    let server_key = crate::config::decode_key(&path.server_public_key)?;
    tokio::time::timeout(Duration::from_secs(path.dial_timeout), async {
        let stream: BoxIo = match incoming {
            crate::kcp_carrier::Incoming::Tcp(stream) => {
                server_carrier(stream, &path.listen_config()).await?
            }
            crate::kcp_carrier::Incoming::Kcp(stream) => Box::new(stream),
            crate::kcp_carrier::Incoming::Xhttp(stream) => stream,
            crate::kcp_carrier::Incoming::Quantum(stream) => stream,
        };
        initiate(stream, private, &server_key).await
    })
    .await
    .context("reverse peer handshake timed out")?
}

pub fn validate_tls_config(config: &crate::config::Config) -> Result<()> {
    crate::carrier::validate_tls_config(config)
}

async fn respond(
    mut stream: BoxIo,
    private_key: &[u8; 32],
    allowed_peers: &[[u8; 32]],
) -> Result<SecureConnection> {
    ensure!(!allowed_peers.is_empty(), "no authorized peers configured");
    let mut handshake = Builder::new(NOISE_PATTERN.parse()?)
        .local_private_key(private_key)?
        .prologue(PROLOGUE)?
        .build_responder()?;
    let request = read_record(&mut stream).await?;
    let mut output = vec![0; MAX_RECORD];
    let size = handshake
        .read_message(&request, &mut output)
        .context("peer handshake authentication failed")?;
    ensure!(size == 0, "unexpected handshake payload");
    let remote = handshake
        .get_remote_static()
        .context("missing peer identity")?;
    // These are public keys; equality is not a secret-dependent comparison.
    ensure!(
        allowed_peers
            .iter()
            .any(|allowed| allowed.as_slice() == remote),
        "peer key is not authorized"
    );
    let size = handshake.write_message(&[], &mut output)?;
    write_record(&mut stream, &output[..size]).await?;
    ensure!(handshake.is_handshake_finished(), "incomplete handshake");
    Ok(SecureConnection {
        stream,
        state: handshake.into_transport_mode()?,
    })
}

async fn read_record(reader: &mut (impl AsyncRead + Unpin)) -> Result<Vec<u8>> {
    let length = reader.read_u16().await.context("read record header")? as usize;
    ensure!(length > 0, "empty encrypted record");
    let mut data = vec![0; length];
    reader
        .read_exact(&mut data)
        .await
        .context("read encrypted record")?;
    Ok(data)
}

async fn write_record(writer: &mut (impl AsyncWrite + Unpin), data: &[u8]) -> Result<()> {
    ensure!(
        !data.is_empty() && data.len() <= MAX_RECORD,
        "invalid record length"
    );
    writer.write_u16(data.len() as u16).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn keys() -> Result<snow::Keypair> {
        Ok(Builder::new(NOISE_PATTERN.parse()?).generate_keypair()?)
    }

    #[tokio::test]
    async fn authenticated_bidirectional_records() -> Result<()> {
        let server = keys()?;
        let client = keys()?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?.to_string();
        let public: [u8; 32] = server.public.try_into().unwrap();
        let peer: [u8; 32] = client.public.try_into().unwrap();
        let private: [u8; 32] = server.private.try_into().unwrap();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            let connection = accept(stream, &private, &[peer], Duration::from_secs(3)).await?;
            let (mut read, mut write) = connection.split();
            let frame = read.recv().await?;
            write.send(&frame).await?;
            anyhow::Ok(())
        });
        let client_private = client.private.try_into().unwrap();
        let connection =
            connect(&address, &client_private, &public, Duration::from_secs(3)).await?;
        let (mut read, mut write) = connection.split();
        let data = Frame::Data {
            id: 1,
            data: vec![42; crate::wire::DATA_SIZE],
        };
        write.send(&data).await?;
        assert_eq!(read.recv().await?, data);
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn untrusted_client_is_rejected() -> Result<()> {
        let server = keys()?;
        let client = keys()?;
        let trusted = keys()?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?.to_string();
        let public = server.public.try_into().unwrap();
        let private = server.private.try_into().unwrap();
        let allowed = trusted.public.try_into().unwrap();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            assert!(
                accept(stream, &private, &[allowed], Duration::from_secs(3))
                    .await
                    .is_err()
            );
        });
        assert!(
            connect(
                &address,
                &client.private.try_into().unwrap(),
                &public,
                Duration::from_secs(3)
            )
            .await
            .is_err()
        );
        task.await?;
        Ok(())
    }

    #[test]
    fn modified_and_replayed_ciphertext_is_rejected() -> Result<()> {
        let server = keys()?;
        let client = keys()?;
        let mut a = Builder::new(NOISE_PATTERN.parse()?)
            .local_private_key(&client.private)?
            .remote_public_key(&server.public)?
            .prologue(PROLOGUE)?
            .build_initiator()?;
        let mut b = Builder::new(NOISE_PATTERN.parse()?)
            .local_private_key(&server.private)?
            .prologue(PROLOGUE)?
            .build_responder()?;
        let mut packet = [0; 1024];
        let mut plain = [0; 1024];
        let n = a.write_message(&[], &mut packet)?;
        b.read_message(&packet[..n], &mut plain)?;
        let n = b.write_message(&[], &mut packet)?;
        a.read_message(&packet[..n], &mut plain)?;
        let (mut a, mut b) = (a.into_transport_mode()?, b.into_transport_mode()?);
        let n = a.write_message(b"payload", &mut packet)?;
        let mut modified = packet;
        modified[0] ^= 1;
        assert!(b.read_message(&modified[..n], &mut plain).is_err());
        assert_eq!(b.read_message(&packet[..n], &mut plain)?, 7);
        assert!(b.read_message(&packet[..n], &mut plain).is_err());
        Ok(())
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
