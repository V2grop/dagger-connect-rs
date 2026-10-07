//! Network carriers. Noise authenticates and encrypts every carrier's payload.
use crate::config::{ClientPath, Config, Listener, Transport};
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    io,
    path::Path,
    pin::Pin,
    sync::{Arc, LazyLock, Mutex},
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf},
    net::TcpStream,
    task::JoinHandle,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};

pub trait TunnelIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> TunnelIo for T {}
pub type BoxIo = Box<dyn TunnelIo>;

fn certs(path: &Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read certificate {}", path.display()))?;
    let result =
        rustls_pemfile::certs(&mut bytes.as_slice()).collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        !result.is_empty(),
        "certificate file contains no certificates"
    );
    Ok(result)
}

fn server_tls(listener: &Listener) -> Result<Arc<rustls::ServerConfig>> {
    let certificates = certs(
        listener
            .cert_file
            .as_deref()
            .context("cert_file required")?,
    )?;
    let bytes = std::fs::read(listener.key_file.as_deref().context("key_file required")?)?;
    let key = rustls_pemfile::private_key(&mut bytes.as_slice())?
        .context("TLS key file contains no key")?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(certificates, key)?;
    Ok(Arc::new(config))
}

fn client_tls(path: &ClientPath) -> Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    if let Some(file) = &path.ca_file {
        for cert in certs(file)? {
            roots.add(cert)?;
        }
    } else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

pub fn validate_tls_config(config: &Config) -> Result<()> {
    for listener in &config.listeners {
        if listener.transport.uses_tls() {
            if config.reverse {
                let path = listener.dial_path();
                client_tls(&path)?;
                tls_server_name(&path)?;
            } else {
                server_tls(listener)?;
            }
        }
    }
    for path in &config.paths {
        if path.transport.uses_tls() {
            if config.reverse {
                server_tls(&path.listen_config())?;
            } else {
                client_tls(path)?;
                tls_server_name(path)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn http_authority(
    value: &str,
) -> Result<tokio_tungstenite::tungstenite::http::uri::Authority> {
    ensure!(
        !value.is_empty() && value.len() <= 253 && value.is_ascii() && !value.contains('@'),
        "invalid HTTP Host authority"
    );
    let authority: tokio_tungstenite::tungstenite::http::uri::Authority =
        value.parse().context("invalid HTTP Host authority")?;
    let host = authority.host();
    ensure!(!host.is_empty(), "HTTP Host must contain a hostname or IP");
    if host.starts_with('[') {
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::Ipv6Addr>()
            .context("invalid HTTP Host IPv6 address")?;
    } else {
        ensure!(
            host.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')),
            "invalid HTTP Host hostname"
        );
    }
    let suffix = &value[host.len()..];
    if !suffix.is_empty() {
        let digits = suffix
            .strip_prefix(':')
            .context("invalid HTTP Host port separator")?;
        ensure!(
            !digits.is_empty()
                && digits.len() <= 5
                && digits.bytes().all(|b| b.is_ascii_digit())
                && digits.parse::<u16>().is_ok_and(|port| port != 0),
            "invalid HTTP Host port"
        );
    }
    Ok(authority)
}

fn tls_server_name(path: &ClientPath) -> Result<rustls::pki_types::ServerName<'static>> {
    let name = if let Some(host) = &path.xhttp.host {
        let authority = http_authority(host)?;
        authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string()
    } else {
        path.server_name.clone().context("server_name required")?
    };
    Ok(rustls::pki_types::ServerName::try_from(name)?)
}

static FAILED_EDGES: LazyLock<Mutex<HashMap<String, tokio::time::Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) async fn connect_http(path: &ClientPath) -> Result<BoxIo> {
    if !matches!(path.transport, Transport::Xhttp | Transport::Xhttps)
        || path.xhttp.edge_addrs.is_empty()
    {
        return connect_to(&path.addr, path).await;
    }
    let key = |addr: &str| {
        format!(
            "{addr}|{}|{:?}",
            path.xhttp.host.as_deref().unwrap_or(&path.addr),
            path.transport
        )
    };
    let now = tokio::time::Instant::now();
    let mut candidates: Vec<_> = {
        let mut failed = FAILED_EDGES
            .lock()
            .map_err(|_| anyhow::anyhow!("edge retry state poisoned"))?;
        failed.retain(|_, retry| *retry > now);
        path.xhttp
            .edge_addrs
            .iter()
            .filter(|addr| !failed.contains_key(&key(addr)))
            .collect()
    };
    // When every edge recently failed, retry the configured list immediately
    // instead of turning a temporary blacklist into a total outage.
    if candidates.is_empty() {
        candidates = path.xhttp.edge_addrs.iter().collect();
    }
    let per_edge = Duration::from_millis(
        (path.dial_timeout.saturating_mul(1000) / candidates.len() as u64).clamp(100, 3000),
    );
    let mut error = None;
    for address in candidates {
        let result = tokio::time::timeout(per_edge, connect_to(address, path))
            .await
            .map_err(|_| anyhow::anyhow!("configured edge connection timed out"))
            .and_then(|result| result);
        match result {
            Ok(stream) => {
                if let Ok(mut failed) = FAILED_EDGES.lock() {
                    failed.remove(&key(address));
                }
                return Ok(stream);
            }
            Err(reason) => {
                if let Ok(mut failed) = FAILED_EDGES.lock() {
                    // The cache remains bounded even across many configs.
                    if failed.len() >= 1024 {
                        failed.clear();
                    }
                    failed.insert(
                        key(address),
                        tokio::time::Instant::now() + Duration::from_secs(60),
                    );
                }
                error = Some(reason.context(format!("configured edge {address}")));
            }
        }
    }
    Err(error.context("no configured XHTTP edge address")?)
}

async fn connect_to(address: &str, path: &ClientPath) -> Result<BoxIo> {
    let tcp = TcpStream::connect(address)
        .await
        .context("connect to configured peer")?;
    tcp.set_nodelay(true)?;
    if matches!(path.transport, Transport::Xhttp | Transport::Xhttps) {
        set_http_socket_buffers(&tcp, path.xhttp.socket_buf_bytes)?;
    }
    let mut stream: BoxIo = Box::new(tcp);
    if path.transport.uses_tls() {
        let name = tls_server_name(path)?;
        stream = Box::new(
            TlsConnector::from(client_tls(path)?)
                .connect(name, stream)
                .await
                .context("TLS peer verification failed")?,
        );
    }
    Ok(stream)
}

pub(crate) async fn accept_http(tcp: TcpStream, listener: &Listener) -> Result<BoxIo> {
    tcp.set_nodelay(true)?;
    set_http_socket_buffers(&tcp, listener.xhttp.socket_buf_bytes)?;
    let mut stream: BoxIo = Box::new(tcp);
    if listener.transport.uses_tls() {
        stream = Box::new(
            TlsAcceptor::from(server_tls(listener)?)
                .accept(stream)
                .await?,
        );
    }
    Ok(stream)
}

fn set_http_socket_buffers(socket: &TcpStream, bytes: usize) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let value: libc::c_int = bytes
            .try_into()
            .context("socket buffer size exceeds kernel integer range")?;
        for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
            // The descriptor remains owned by TcpStream, the integer is valid
            // for the entire call, and its exact size is supplied to Linux.
            let result = unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    option,
                    (&value as *const libc::c_int).cast(),
                    std::mem::size_of_val(&value) as libc::socklen_t,
                )
            };
            if result != 0 {
                return Err(io::Error::last_os_error())
                    .context("set configured HTTP socket buffer");
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (socket, bytes);
    Ok(())
}

pub async fn client_carrier(path: &ClientPath) -> Result<BoxIo> {
    if path.transport == Transport::Tun {
        bail!("raw TUN carrier runs through the dedicated TUN device engine");
    }
    if matches!(
        path.transport,
        Transport::Quantum | Transport::QuantumPlus | Transport::QuantumGaming
    ) {
        return crate::quantum_carrier::connect(path).await;
    }
    if path.transport == Transport::Kcp {
        return Ok(Box::new(crate::kcp_carrier::connect(&path.addr).await?));
    }
    if matches!(path.transport, Transport::Xhttp | Transport::Xhttps) {
        return crate::xhttp_carrier::connect(path).await;
    }
    let mut stream = connect_http(path).await?;
    match path.transport {
        Transport::Tcp | Transport::Dc6 | Transport::Kcp => Ok(stream),
        Transport::Ws | Transport::Wss => {
            let url = format!(
                "{}://{}{}",
                if path.transport.uses_tls() {
                    "wss"
                } else {
                    "ws"
                },
                path.addr,
                path.http_path
            );
            let (socket, _) =
                tokio_tungstenite::client_async_with_config(url, stream, Some(ws_config())).await?;
            Ok(websocket_io(socket))
        }
        Transport::Http | Transport::Https => {
            stream.write_all(format!("GET {} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: dagger-rs-v1\r\n\r\n", path.http_path, path.addr).as_bytes()).await?;
            stream.flush().await?;
            let header = read_headers(&mut stream).await?;
            ensure!(
                header.first == "HTTP/1.1 101 Switching Protocols"
                    && header.value("upgrade") == Some("dagger-rs-v1"),
                "HTTP upgrade rejected"
            );
            Ok(stream)
        }
        Transport::Xhttp
        | Transport::Xhttps
        | Transport::Quantum
        | Transport::QuantumPlus
        | Transport::QuantumGaming
        | Transport::Tun => unreachable!("handled before TCP connect"),
    }
}

pub async fn server_carrier(tcp: TcpStream, listener: &Listener) -> Result<BoxIo> {
    tcp.set_nodelay(true)?;
    let mut stream: BoxIo = Box::new(tcp);
    if listener.transport.uses_tls() {
        stream = Box::new(
            TlsAcceptor::from(server_tls(listener)?)
                .accept(stream)
                .await?,
        );
    }
    match listener.transport {
        Transport::Tcp | Transport::Dc6 | Transport::Kcp => Ok(stream),
        Transport::Quantum | Transport::QuantumPlus | Transport::QuantumGaming | Transport::Tun => {
            bail!("raw/Quantum sessions require their dedicated acceptor")
        }
        Transport::Ws | Transport::Wss => {
            let path = listener.http_path.clone();
            // Tungstenite requires its HTTP response as the callback error type.
            #[allow(clippy::result_large_err)]
            let callback =
                move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                      response| {
                    if request.uri().path_and_query().map(|v| v.as_str()) == Some(path.as_str()) {
                        Ok(response)
                    } else {
                        Err(tokio_tungstenite::tungstenite::http::Response::builder()
                            .status(404)
                            .body(Some("unknown tunnel path".to_string()))
                            .unwrap())
                    }
                };
            let socket = tokio_tungstenite::accept_hdr_async_with_config(
                stream,
                callback,
                Some(ws_config()),
            )
            .await?;
            Ok(websocket_io(socket))
        }
        Transport::Http | Transport::Https => {
            let header = read_headers(&mut stream).await?;
            ensure!(
                header.first == format!("GET {} HTTP/1.1", listener.http_path)
                    && header.value("upgrade") == Some("dagger-rs-v1")
                    && header.value("content-length").is_none()
                    && header.value("transfer-encoding").is_none(),
                "invalid HTTP upgrade"
            );
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: dagger-rs-v1\r\n\r\n").await?;
            stream.flush().await?;
            Ok(stream)
        }
        Transport::Xhttp | Transport::Xhttps => {
            let header = read_headers(&mut stream).await?;
            ensure!(
                header.first == format!("POST {} HTTP/1.1", listener.http_path)
                    && header.value("transfer-encoding") == Some("chunked")
                    && header.value("content-length").is_none(),
                "invalid HTTP streaming request"
            );
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nCache-Control: no-store\r\n\r\n").await?;
            stream.flush().await?;
            Ok(chunked_io(stream))
        }
    }
}

pub(crate) struct Headers {
    pub(crate) first: String,
    pub(crate) fields: Vec<(String, String)>,
}
impl Headers {
    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.as_str())
    }
}

pub(crate) async fn read_headers(reader: &mut BoxIo) -> Result<Headers> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        ensure!(bytes.len() < 8192, "HTTP headers too large");
        bytes.push(reader.read_u8().await?);
    }
    let text = std::str::from_utf8(&bytes)?;
    let mut lines = text[..text.len() - 4].split("\r\n");
    let first = lines.next().context("empty HTTP header")?.to_string();
    let mut fields = Vec::new();
    for line in lines {
        let (key, value) = line.split_once(':').context("invalid HTTP header")?;
        let key = key.to_ascii_lowercase();
        ensure!(
            !fields.iter().any(|(prior, _)| prior == &key),
            "duplicate HTTP header"
        );
        fields.push((key, value.trim().to_string()));
    }
    Ok(Headers { first, fields })
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(65_536))
        .max_frame_size(Some(65_536))
}

/// Owns the bridge task so dropping a tunnel cannot leave network tasks behind.
pub(crate) struct ManagedIo {
    pub(crate) io: DuplexStream,
    pub(crate) task: JoinHandle<()>,
}
impl Drop for ManagedIo {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl AsyncRead for ManagedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}
impl AsyncWrite for ManagedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

fn websocket_io(socket: WebSocketStream<BoxIo>) -> BoxIo {
    let (io, bridge) = tokio::io::duplex(65_536);
    let task = tokio::spawn(async move {
        let (mut sink, mut source) = socket.split();
        let (mut input, mut output) = tokio::io::split(bridge);
        let upload = async {
            let mut buffer = vec![0; 16_384];
            loop {
                let count = input.read(&mut buffer).await?;
                if count == 0 {
                    sink.close().await?;
                    return anyhow::Ok(());
                }
                sink.send(Message::Binary(buffer[..count].to_vec().into()))
                    .await?;
            }
        };
        let download = async {
            while let Some(message) = source.next().await {
                match message? {
                    Message::Binary(bytes) => output.write_all(&bytes).await?,
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => {}
                    _ => bail!("expected binary WebSocket message"),
                }
            }
            output.shutdown().await?;
            anyhow::Ok(())
        };
        if let Err(error) = tokio::try_join!(upload, download) {
            tracing::debug!(%error,"WebSocket carrier ended");
        }
    });
    Box::new(ManagedIo { io, task })
}

pub(crate) fn chunked_io(stream: BoxIo) -> BoxIo {
    let (io, bridge) = tokio::io::duplex(65_536);
    let task = tokio::spawn(async move {
        let (mut network_read, mut network_write) = tokio::io::split(stream);
        let (mut input, mut output) = tokio::io::split(bridge);
        let upload = async {
            let mut buffer = vec![0; 16_384];
            loop {
                let count = input.read(&mut buffer).await?;
                network_write
                    .write_all(format!("{count:x}\r\n").as_bytes())
                    .await?;
                network_write.write_all(&buffer[..count]).await?;
                network_write.write_all(b"\r\n").await?;
                network_write.flush().await?;
                if count == 0 {
                    return anyhow::Ok(());
                }
            }
        };
        let download = async {
            loop {
                let mut line = Vec::new();
                while !line.ends_with(b"\r\n") {
                    ensure!(line.len() < 32, "HTTP chunk header too long");
                    line.push(network_read.read_u8().await?);
                }
                let count =
                    usize::from_str_radix(std::str::from_utf8(&line[..line.len() - 2])?, 16)?;
                ensure!(count <= 65_536, "HTTP chunk too large");
                let mut data = vec![0; count];
                network_read.read_exact(&mut data).await?;
                let mut end = [0; 2];
                network_read.read_exact(&mut end).await?;
                ensure!(&end == b"\r\n", "invalid HTTP chunk terminator");
                if count == 0 {
                    output.shutdown().await?;
                    return anyhow::Ok(());
                }
                output.write_all(&data).await?;
            }
        };
        if let Err(error) = tokio::try_join!(upload, download) {
            tracing::debug!(%error,"HTTP body carrier ended");
        }
    });
    Box::new(ManagedIo { io, task })
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
