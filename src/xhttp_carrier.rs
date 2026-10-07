//! HTTP request/body transport with bounded, sequenced packet uploads.
//! This is dagger-rs's own framing; it is not the proprietary DaggerConnect wire format.
use crate::{
    carrier::{
        BoxIo, Headers, ManagedIo, accept_http, chunked_io, connect_http, http_authority,
        read_headers,
    },
    config::{ClientPath, Listener, XhttpMode, XhttpOptions},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::{Semaphore, mpsc, oneshot},
    task::{JoinHandle, JoinSet},
    time::{Instant, timeout},
};

const MAX_SESSIONS: usize = 64;
const MAX_HTTP_REQUESTS: usize = 256;
const UPLOAD_WINDOW: usize = 16;
#[cfg(test)]
const MAX_CHUNK: usize = 65_536;

struct Upload {
    sequence: u64,
    body: Vec<u8>,
    acknowledged: oneshot::Sender<u16>,
}

struct Session {
    uploads: mpsc::Sender<Upload>,
    mode: XhttpMode,
    stream_started: AtomicBool,
    request_slots: Arc<Semaphore>,
}

type Sessions = Arc<Mutex<HashMap<String, Arc<Session>>>>;

/// A guard removes a session even when the owning bridge task is cancelled.
struct SessionGuard {
    sessions: Sessions,
    id: String,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(&self.id);
        }
    }
}

/// Owns the HTTP listener and request tasks. The engine receives one stream
/// per GET session, regardless of how many upload requests that session uses.
pub struct Acceptor {
    incoming: mpsc::Receiver<BoxIo>,
    task: JoinHandle<()>,
}
impl Drop for Acceptor {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Acceptor {
    pub async fn bind(listener: &Listener) -> Result<Self> {
        let socket = TcpListener::bind(&listener.addr).await?;
        Self::from_listener(socket, listener.clone())
    }

    /// Allows tests and embedders to supply an already bound listener.
    pub fn from_listener(socket: TcpListener, listener: Listener) -> Result<Self> {
        let (incoming_tx, incoming) = mpsc::channel(MAX_SESSIONS);
        let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
        let task = tokio::spawn(async move {
            let slots = Arc::new(Semaphore::new(MAX_HTTP_REQUESTS));
            let mut requests = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = socket.accept() => {
                        let Ok((tcp, _)) = accepted else { break; };
                        let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                        let (listener, sessions, incoming_tx) = (listener.clone(), sessions.clone(), incoming_tx.clone());
                        requests.spawn(async move {
                            let _permit = permit;
                            let result = async {
                                let deadline = Duration::from_secs(listener.xhttp.session_timeout_sec);
                                let stream = timeout(deadline, accept_http(tcp, &listener)).await??;
                                handle_request(stream, &listener, sessions, incoming_tx).await
                            }.await;
                            if let Err(error) = result {
                                tracing::debug!(%error, "XHTTP request ended");
                            }
                        });
                    }
                    _ = requests.join_next(), if !requests.is_empty() => {}
                }
            }
        });
        Ok(Self { incoming, task })
    }

    pub async fn accept(&mut self) -> Result<BoxIo> {
        self.incoming.recv().await.context("XHTTP listener stopped")
    }
}

fn prefix(path: &str) -> &str {
    path.trim_end_matches('/')
}

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn host(path: &ClientPath) -> &str {
    path.xhttp.host.as_deref().unwrap_or(&path.addr)
}

fn chunked(headers: &Headers) -> bool {
    headers
        .value("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
}

fn random_bytes(bytes: &mut [u8]) -> Result<()> {
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(bytes)
        .map_err(|_| anyhow!("operating-system random generator failed"))
}

fn pad_url(base: &str, options: &XhttpOptions) -> Result<String> {
    ensure!(
        options.pad_min <= options.pad_max && options.pad_max <= 4096,
        "invalid HTTP padding bounds"
    );
    let mut random = [0; 4];
    random_bytes(&mut random)?;
    let length = options.pad_min
        + u32::from_le_bytes(random) as usize % (options.pad_max - options.pad_min + 1);
    if length == 0 {
        return Ok(base.to_string());
    }
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut bytes = vec![0; length];
    random_bytes(&mut bytes)?;
    for byte in &mut bytes {
        *byte = ALPHABET[*byte as usize % ALPHABET.len()];
    }
    Ok(format!("{base}?v={}", String::from_utf8(bytes)?))
}

fn request_path(url: &str) -> Result<&str> {
    ensure!(
        url.is_ascii() && !url.bytes().any(|b| b.is_ascii_control()) && !url.contains('#'),
        "invalid HTTP tunnel URL"
    );
    if let Some((path, query)) = url.split_once('?') {
        let padding = query
            .strip_prefix("v=")
            .context("unknown HTTP tunnel query parameter")?;
        ensure!(
            padding.len() <= 4096 && padding.bytes().all(|b| b.is_ascii_alphanumeric()),
            "invalid HTTP tunnel query padding"
        );
        Ok(path)
    } else {
        Ok(url)
    }
}

fn user_agent(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value.is_ascii()
            && !value.bytes().any(|b| b.is_ascii_control()),
        "invalid HTTP User-Agent"
    );
    Ok(())
}

async fn write_request(
    stream: &mut BoxIo,
    path: &ClientPath,
    method: &str,
    target: &str,
    fields: &str,
) -> Result<()> {
    http_authority(host(path))?;
    user_agent(&path.xhttp.user_agent)?;
    let url = pad_url(target, &path.xhttp)?;
    stream
        .write_all(
            format!(
                "{method} {url} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\n{fields}\r\n",
                host(path),
                path.xhttp.user_agent
            )
            .as_bytes(),
        )
        .await?;
    stream.flush().await?;
    Ok(())
}

async fn response(stream: &mut BoxIo, status: u16) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Bad Request",
    };
    stream.write_all(format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n").as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

async fn body_response(stream: &mut BoxIo) -> Result<()> {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nCache-Control: no-store, no-transform\r\nX-Accel-Buffering: no\r\nConnection: keep-alive\r\n\r\n").await?;
    stream.flush().await?;
    Ok(())
}

async fn handle_request(
    mut stream: BoxIo,
    listener: &Listener,
    sessions: Sessions,
    incoming: mpsc::Sender<BoxIo>,
) -> Result<()> {
    let deadline = Duration::from_secs(listener.xhttp.session_timeout_sec);
    loop {
        let header = timeout(deadline, read_headers(&mut stream)).await??;
        let parts: Vec<_> = header.first.split(' ').collect();
        if parts.len() != 3 || parts[2] != "HTTP/1.1" {
            response(&mut stream, 400).await?;
            return Ok(());
        }
        let method = parts[0];
        let url = match request_path(parts[1]) {
            Ok(url) => url,
            Err(_) => {
                response(&mut stream, 400).await?;
                return Ok(());
            }
        };
        if header
            .value("host")
            .is_none_or(|value| http_authority(value).is_err())
        {
            response(&mut stream, 400).await?;
            return Ok(());
        }
        // Retain the original dagger-rs stream-up endpoint for old configs.
        if method == "POST" && url == listener.http_path {
            if !chunked(&header) || header.value("content-length").is_some() {
                response(&mut stream, 400).await?;
                return Ok(());
            }
            body_response(&mut stream).await?;
            incoming
                .send(chunked_io(stream))
                .await
                .map_err(|_| anyhow!("XHTTP accept queue closed"))?;
            return Ok(());
        }
        let Some(suffix) = url.strip_prefix(&format!("{}/", prefix(&listener.http_path))) else {
            response(&mut stream, 404).await?;
            return Ok(());
        };
        let segments: Vec<_> = suffix.split('/').collect();
        if method == "POST"
            && segments.len() == 2
            && segments[0] == "probe"
            && valid_id(segments[1])
        {
            if !chunked(&header) || header.value("content-length").is_some() {
                response(&mut stream, 400).await?;
                return Ok(());
            }
            let first = timeout(deadline, read_chunk(&mut stream, 1)).await??;
            ensure!(
                first.as_deref() == Some(&[0][..]),
                "invalid XHTTP streaming probe"
            );
            response(&mut stream, 200).await?;
            ensure!(
                timeout(deadline, read_chunk(&mut stream, 1))
                    .await??
                    .is_none(),
                "probe has trailing data"
            );
            continue;
        }
        if segments.is_empty() || !valid_id(segments[0]) {
            response(&mut stream, 404).await?;
            return Ok(());
        }
        let id = segments[0].to_string();
        if method == "GET" && segments.len() == 1 {
            if header
                .value("content-length")
                .is_some_and(|value| value != "0")
                || header.value("transfer-encoding").is_some()
            {
                response(&mut stream, 400).await?;
                return Ok(());
            }
            let mode = match header.value("x-dagger-upload") {
                Some("packet-up") => XhttpMode::PacketUp,
                Some("stream-up") => XhttpMode::StreamUp,
                _ => {
                    response(&mut stream, 400).await?;
                    return Ok(());
                }
            };
            let (upload_tx, upload_rx) = mpsc::channel(UPLOAD_WINDOW);
            let session = Arc::new(Session {
                uploads: upload_tx,
                mode,
                stream_started: AtomicBool::new(false),
                request_slots: Arc::new(Semaphore::new(UPLOAD_WINDOW)),
            });
            let status = {
                let mut entries = sessions
                    .lock()
                    .map_err(|_| anyhow!("XHTTP session registry poisoned"))?;
                if entries.contains_key(&id) {
                    409
                } else if entries.len() >= MAX_SESSIONS {
                    503
                } else {
                    entries.insert(id.clone(), session);
                    200
                }
            };
            if status != 200 {
                response(&mut stream, status).await?;
                return Ok(());
            }
            let guard = SessionGuard { sessions, id };
            body_response(&mut stream).await?;
            let (io, bridge) = tokio::io::duplex(listener.xhttp.buffer_bytes);
            let task = tokio::spawn(async move {
                let _guard = guard;
                let (mut input, mut output) = tokio::io::split(bridge);
                let result = tokio::try_join!(
                    receive_uploads(upload_rx, &mut output, deadline),
                    send_chunks(&mut input, &mut stream)
                );
                if let Err(error) = result {
                    tracing::debug!(%error, "XHTTP session ended");
                }
            });
            incoming
                .send(Box::new(ManagedIo { io, task }))
                .await
                .map_err(|_| anyhow!("XHTTP accept queue closed"))?;
            return Ok(());
        }
        let session = {
            sessions
                .lock()
                .map_err(|_| anyhow!("XHTTP session registry poisoned"))?
                .get(&id)
                .cloned()
        };
        let Some(session) = session else {
            response(&mut stream, 410).await?;
            return Ok(());
        };
        if method != "POST" {
            response(&mut stream, 404).await?;
            return Ok(());
        }
        if segments.len() == 1 && session.mode == XhttpMode::StreamUp {
            if !chunked(&header) || header.value("content-length").is_some() {
                response(&mut stream, 400).await?;
                return Ok(());
            }
            if session.stream_started.swap(true, Ordering::AcqRel) {
                response(&mut stream, 409).await?;
                return Ok(());
            }
            body_response(&mut stream).await?;
            write_chunk(&mut stream, b"+").await?;
            let mut sequence = 0;
            loop {
                let chunk = timeout(
                    deadline,
                    read_chunk(&mut stream, listener.xhttp.up_max_bytes),
                )
                .await??;
                let end = chunk.is_none();
                let status =
                    submit(&session, sequence, chunk.unwrap_or_default(), deadline).await?;
                ensure!(status == 200, "XHTTP streaming upload rejected");
                if end {
                    write_chunk(&mut stream, &[]).await?;
                    return Ok(());
                }
                sequence = sequence
                    .checked_add(1)
                    .context("XHTTP upload sequence exhausted")?;
            }
        }
        if segments.len() != 2 || session.mode != XhttpMode::PacketUp {
            response(&mut stream, 409).await?;
            return Ok(());
        }
        if segments[1].is_empty()
            || segments[1].len() > 20
            || !segments[1].bytes().all(|b| b.is_ascii_digit())
        {
            response(&mut stream, 400).await?;
            return Ok(());
        }
        let Ok(sequence) = segments[1].parse::<u64>() else {
            response(&mut stream, 400).await?;
            return Ok(());
        };
        if header.value("transfer-encoding").is_some() {
            response(&mut stream, 400).await?;
            return Ok(());
        }
        let Some(length) = header
            .value("content-length")
            .filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<usize>().ok())
        else {
            response(&mut stream, 400).await?;
            return Ok(());
        };
        if length > listener.xhttp.up_max_bytes {
            response(&mut stream, 413).await?;
            return Ok(());
        }
        let Ok(_slot) = session.request_slots.clone().try_acquire_owned() else {
            response(&mut stream, 429).await?;
            return Ok(());
        };
        let mut body = vec![0; length];
        timeout(deadline, stream.read_exact(&mut body)).await??;
        let status = submit(&session, sequence, body, deadline).await?;
        response(&mut stream, status).await?;
        if length == 0 {
            return Ok(());
        }
    }
}

async fn submit(
    session: &Session,
    sequence: u64,
    body: Vec<u8>,
    deadline: Duration,
) -> Result<u16> {
    let (acknowledged, ack) = oneshot::channel();
    if timeout(
        deadline,
        session.uploads.send(Upload {
            sequence,
            body,
            acknowledged,
        }),
    )
    .await?
    .is_err()
    {
        return Ok(410);
    }
    Ok(timeout(deadline, ack).await?.unwrap_or(410))
}

async fn receive_uploads<W: AsyncWrite + Unpin>(
    mut uploads: mpsc::Receiver<Upload>,
    output: &mut W,
    deadline: Duration,
) -> Result<()> {
    let mut next = 0u64;
    let mut pending = BTreeMap::new();
    while let Some(upload) = timeout(deadline, uploads.recv()).await? {
        if upload.sequence < next
            || upload.sequence - next >= UPLOAD_WINDOW as u64
            || pending.contains_key(&upload.sequence)
        {
            let _ = upload.acknowledged.send(409);
            continue;
        }
        pending.insert(upload.sequence, upload);
        while let Some(upload) = pending.remove(&next) {
            if upload.body.is_empty() {
                output.shutdown().await?;
                let _ = upload.acknowledged.send(200);
                return Ok(());
            }
            timeout(deadline, output.write_all(&upload.body)).await??;
            let _ = upload.acknowledged.send(200);
            next = next
                .checked_add(1)
                .context("XHTTP upload sequence exhausted")?;
        }
    }
    bail!("XHTTP upload session closed before EOF")
}

async fn write_chunk<W: AsyncWrite + Unpin>(stream: &mut W, body: &[u8]) -> Result<()> {
    stream
        .write_all(format!("{:x}\r\n", body.len()).as_bytes())
        .await?;
    stream.write_all(body).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await?;
    Ok(())
}

async fn read_chunk<R: AsyncRead + Unpin>(stream: &mut R, max: usize) -> Result<Option<Vec<u8>>> {
    let length = chunk_length(stream, max).await?;
    if length == 0 {
        finish_chunks(stream).await?;
        return Ok(None);
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    chunk_terminator(stream).await?;
    Ok(Some(body))
}

async fn chunk_length<R: AsyncRead + Unpin>(stream: &mut R, max: usize) -> Result<usize> {
    let mut line = Vec::new();
    while !line.ends_with(b"\r\n") {
        ensure!(line.len() < 256, "XHTTP chunk header too long");
        line.push(stream.read_u8().await?);
    }
    let line = std::str::from_utf8(&line[..line.len() - 2])?;
    ensure!(
        line.bytes().all(|b| b.is_ascii_graphic() || b == b' '),
        "invalid HTTP chunk extension"
    );
    let digits = line.split(';').next().unwrap();
    ensure!(
        !digits.is_empty() && digits.len() <= 16 && digits.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid XHTTP chunk length"
    );
    let length = usize::from_str_radix(digits, 16)?;
    ensure!(length <= max, "XHTTP chunk exceeds configured limit");
    Ok(length)
}

async fn chunk_terminator<R: AsyncRead + Unpin>(stream: &mut R) -> Result<()> {
    let mut end = [0; 2];
    stream.read_exact(&mut end).await?;
    ensure!(&end == b"\r\n", "invalid XHTTP chunk terminator");
    Ok(())
}

async fn finish_chunks<R: AsyncRead + Unpin>(stream: &mut R) -> Result<()> {
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        while !line.ends_with(b"\r\n") {
            ensure!(total < 8192, "HTTP trailers too large");
            line.push(stream.read_u8().await?);
            total += 1;
        }
        if line == b"\r\n" {
            return Ok(());
        }
        let line = std::str::from_utf8(&line[..line.len() - 2])?;
        ensure!(
            line.contains(':') && !line.bytes().any(|b| b.is_ascii_control()),
            "invalid HTTP trailer"
        );
    }
}

async fn send_chunks<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    send_chunks_sized(reader, writer, 16_384).await
}

async fn send_chunks_sized<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    size: usize,
) -> Result<()> {
    let mut buffer = vec![0; size];
    loop {
        let count = reader.read(&mut buffer).await?;
        write_chunk(writer, &buffer[..count]).await?;
        if count == 0 {
            return Ok(());
        }
    }
}

async fn receive_chunks<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    // A proxy may combine or split origin chunks. Consume large proxy chunks
    // with a fixed buffer so they cannot control the tunnel's allocation size.
    let mut buffer = vec![0; 16_384];
    loop {
        let mut remaining = chunk_length(reader, 16 * 1024 * 1024).await?;
        if remaining == 0 {
            finish_chunks(reader).await?;
            break;
        }
        while remaining != 0 {
            let count = remaining.min(buffer.len());
            reader.read_exact(&mut buffer[..count]).await?;
            writer.write_all(&buffer[..count]).await?;
            remaining -= count;
        }
        chunk_terminator(reader).await?;
    }
    writer.shutdown().await?;
    Ok(())
}

fn session_id() -> Result<String> {
    let mut random = [0; 32];
    random_bytes(&mut random)?;
    Ok(hex::encode(random))
}

async fn probe_stream(path: &ClientPath, id: &str) -> Result<()> {
    let mut stream = connect_http(path).await?;
    write_request(&mut stream,path,"POST",&format!("{}/probe/{id}",prefix(&path.http_path)),"Transfer-Encoding: chunked\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n").await?;
    write_chunk(&mut stream, &[0]).await?;
    let headers = read_headers(&mut stream).await?;
    ensure!(
        headers.first == "HTTP/1.1 200 OK"
            && headers.value("content-length") == Some("0")
            && headers.value("transfer-encoding").is_none(),
        "streaming probe rejected"
    );
    write_chunk(&mut stream, &[]).await?;
    Ok(())
}

/// Uses only the configured peer address, Host/SNI and CA. It never discovers
/// edge addresses, contacts vendor services or downloads transport metadata.
pub async fn connect(path: &ClientPath) -> Result<BoxIo> {
    // Preserve the initial long-POST dagger-rs stream framing by default.
    if path.xhttp.mode == XhttpMode::StreamUp {
        let mut stream = connect_http(path).await?;
        write_request(&mut stream,path,"POST",&path.http_path,"Content-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n").await?;
        let header = read_headers(&mut stream).await?;
        ensure!(
            header.first == "HTTP/1.1 200 OK"
                && chunked(&header)
                && header.value("content-length").is_none(),
            "HTTP stream rejected"
        );
        return Ok(chunked_io(stream));
    }
    let id = session_id()?;
    let mode = if path.xhttp.mode == XhttpMode::Auto {
        let limit = path
            .xhttp
            .probe_ms
            .min(path.dial_timeout.saturating_mul(500))
            .max(1);
        if matches!(
            timeout(Duration::from_millis(limit), probe_stream(path, &id)).await,
            Ok(Ok(()))
        ) {
            XhttpMode::StreamUp
        } else {
            XhttpMode::PacketUp
        }
    } else {
        XhttpMode::PacketUp
    };
    let mut download = connect_http(path).await?;
    write_request(
        &mut download,
        path,
        "GET",
        &format!("{}/{id}", prefix(&path.http_path)),
        &format!(
            "X-Dagger-Upload: {}\r\nConnection: keep-alive\r\n",
            if mode == XhttpMode::PacketUp {
                "packet-up"
            } else {
                "stream-up"
            }
        ),
    )
    .await?;
    let headers = read_headers(&mut download).await?;
    ensure!(
        headers.first == "HTTP/1.1 200 OK"
            && chunked(&headers)
            && headers.value("content-length").is_none(),
        "XHTTP download rejected"
    );
    let upload = if mode == XhttpMode::StreamUp {
        let mut stream = connect_http(path).await?;
        write_request(&mut stream,path,"POST",&format!("{}/{id}",prefix(&path.http_path)),"Transfer-Encoding: chunked\r\nContent-Type: application/octet-stream\r\nConnection: keep-alive\r\n").await?;
        let headers = read_headers(&mut stream).await?;
        ensure!(
            headers.first == "HTTP/1.1 200 OK"
                && chunked(&headers)
                && headers.value("content-length").is_none(),
            "XHTTP upload rejected"
        );
        Some(stream)
    } else {
        None
    };
    let (io, bridge) = tokio::io::duplex(path.xhttp.buffer_bytes);
    let path = path.clone();
    let task = tokio::spawn(async move {
        let (mut input, mut output) = tokio::io::split(bridge);
        let upload = async {
            if let Some(stream) = upload {
                let (mut read, mut write) = tokio::io::split(stream);
                let mut discard = tokio::io::sink();
                tokio::try_join!(
                    send_chunks_sized(&mut input, &mut write, path.xhttp.up_max_bytes.min(16_384)),
                    receive_chunks(&mut read, &mut discard)
                )?;
            } else {
                packet_upload(&mut input, &path, &id).await?;
            }
            anyhow::Ok(())
        };
        if let Err(error) = tokio::try_join!(upload, receive_chunks(&mut download, &mut output)) {
            tracing::debug!(%error, "XHTTP carrier ended");
        }
    });
    Ok(Box::new(ManagedIo { io, task }))
}

struct Packet {
    sequence: u64,
    body: Vec<u8>,
}

async fn packet_upload<R: AsyncRead + Unpin>(
    reader: &mut R,
    path: &ClientPath,
    id: &str,
) -> Result<()> {
    let (sender, receiver) = mpsc::channel(path.xhttp.up_concurrency);
    let receiver = Arc::new(tokio::sync::Mutex::new(receiver));
    let mut workers = JoinSet::new();
    for _ in 0..path.xhttp.up_concurrency {
        let (receiver, path, id) = (receiver.clone(), path.clone(), id.to_string());
        workers.spawn(packet_worker(receiver, path, id));
    }
    let producer = async move {
        let mut sequence = 0;
        loop {
            let mut body = vec![0; path.xhttp.up_max_bytes];
            let count = reader.read(&mut body).await?;
            body.truncate(count);
            sender
                .send(Packet { sequence, body })
                .await
                .context("XHTTP upload workers stopped")?;
            if count == 0 {
                return anyhow::Ok(());
            }
            sequence = sequence
                .checked_add(1)
                .context("XHTTP upload sequence exhausted")?;
        }
    };
    let completion = async {
        while let Some(result) = workers.join_next().await {
            result??;
        }
        anyhow::Ok(())
    };
    tokio::try_join!(producer, completion)?;
    Ok(())
}

async fn packet_worker(
    receiver: Arc<tokio::sync::Mutex<mpsc::Receiver<Packet>>>,
    path: ClientPath,
    id: String,
) -> Result<()> {
    let mut connection: Option<BoxIo> = None;
    let mut used = Instant::now();
    let deadline = Duration::from_secs(path.xhttp.session_timeout_sec);
    loop {
        let packet = receiver.lock().await.recv().await;
        let Some(packet) = packet else {
            return Ok(());
        };
        if used.elapsed() >= deadline / 2 {
            connection = None;
        }
        if connection.is_none() {
            connection = Some(connect_http(&path).await?);
        }
        let stream = connection.as_mut().unwrap();
        let keep_alive = timeout(deadline, async {
            write_request(stream,&path,"POST",&format!("{}/{id}/{}",prefix(&path.http_path),packet.sequence),&format!("Content-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n",packet.body.len())).await?;
            stream.write_all(&packet.body).await?;
            stream.flush().await?;
            let headers = read_headers(stream).await?;
            ensure!(headers.first == "HTTP/1.1 200 OK", "XHTTP packet upload rejected: {}", headers.first);
            if chunked(&headers) && headers.value("content-length").is_none() {
                ensure!(read_chunk(stream,0).await?.is_none(), "XHTTP upload acknowledgement contained a body");
            } else {
                ensure!(headers.value("content-length") == Some("0") && headers.value("transfer-encoding").is_none(), "invalid XHTTP upload acknowledgement framing");
            }
            anyhow::Ok(!headers.value("connection").is_some_and(|value| value.split(',').any(|token| token.trim().eq_ignore_ascii_case("close"))))
        }).await??;
        if !keep_alive {
            connection = None;
        }
        used = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(sequence: u64, body: &[u8]) -> (Upload, oneshot::Receiver<u16>) {
        let (acknowledged, receiver) = oneshot::channel();
        (
            Upload {
                sequence,
                body: body.to_vec(),
                acknowledged,
            },
            receiver,
        )
    }

    #[tokio::test]
    async fn uploads_order_and_reject_replays_and_unbounded_sequences() -> Result<()> {
        let (sender, receiver) = mpsc::channel(16);
        let (mut read, mut write) = tokio::io::duplex(64);
        let task = tokio::spawn(async move {
            receive_uploads(receiver, &mut write, Duration::from_secs(2)).await
        });
        let (later, later_ack) = queued(1, b"second");
        sender.send(later).await?;
        let (first, first_ack) = queued(0, b"first");
        sender.send(first).await?;
        ensure!(
            first_ack.await? == 200 && later_ack.await? == 200,
            "ordered upload rejected"
        );
        let (duplicate, duplicate_ack) = queued(0, b"injected");
        sender.send(duplicate).await?;
        ensure!(duplicate_ack.await? == 409, "duplicate upload accepted");
        let (far, far_ack) = queued(u64::MAX, b"injected");
        sender.send(far).await?;
        ensure!(far_ack.await? == 409, "unbounded future upload accepted");
        let (end, end_ack) = queued(2, b"");
        sender.send(end).await?;
        ensure!(end_ack.await? == 200, "EOF rejected");
        let mut actual = Vec::new();
        read.read_to_end(&mut actual).await?;
        ensure!(
            actual == b"firstsecond",
            "ordering or duplicate protection failed"
        );
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn upload_acknowledgement_waits_for_bounded_stream_capacity() -> Result<()> {
        let (sender, receiver) = mpsc::channel(16);
        let (mut read, mut write) = tokio::io::duplex(16);
        let task = tokio::spawn(async move {
            receive_uploads(receiver, &mut write, Duration::from_secs(2)).await
        });
        let (packet, mut ack) = queued(0, &[42; 64]);
        sender.send(packet).await?;
        ensure!(
            timeout(Duration::from_millis(30), &mut ack).await.is_err(),
            "upload was acknowledged before bounded stream accepted its body"
        );
        let mut body = [0; 64];
        read.read_exact(&mut body).await?;
        ensure!(
            body == [42; 64] && ack.await? == 200,
            "backpressured upload changed"
        );
        let (end, end_ack) = queued(1, b"");
        sender.send(end).await?;
        ensure!(end_ack.await? == 200, "EOF rejected");
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn chunk_decoder_rejects_oversize_and_bad_terminator() -> Result<()> {
        for bytes in [b"10001\r\n".as_slice(), b"1\r\nxZZ".as_slice()] {
            let mut reader = bytes;
            ensure!(
                read_chunk(&mut reader, MAX_CHUNK).await.is_err(),
                "malformed chunk accepted"
            );
        }
        Ok(())
    }

    async fn request(address: std::net::SocketAddr, text: String) -> Result<String> {
        let mut stream: BoxIo = Box::new(tokio::net::TcpStream::connect(address).await?);
        stream.write_all(text.as_bytes()).await?;
        stream.flush().await?;
        Ok(read_headers(&mut stream).await?.first)
    }

    async fn listener() -> Result<(Acceptor, std::net::SocketAddr)> {
        let socket = TcpListener::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?;
        let listener: Listener = serde_json::from_value(
            serde_json::json!({"addr":address.to_string(),"transport":"xhttp"}),
        )?;
        Ok((Acceptor::from_listener(socket, listener)?, address))
    }

    async fn open_get(address: std::net::SocketAddr, id: &str) -> Result<BoxIo> {
        let mut stream: BoxIo = Box::new(tokio::net::TcpStream::connect(address).await?);
        stream.write_all(format!("GET /tunnel/{id} HTTP/1.1\r\nHost: localhost\r\nX-Dagger-Upload: packet-up\r\n\r\n").as_bytes()).await?;
        stream.flush().await?;
        ensure!(
            read_headers(&mut stream).await?.first == "HTTP/1.1 200 OK",
            "download setup failed"
        );
        Ok(stream)
    }

    #[tokio::test]
    async fn http_rejects_ambiguous_framing_bad_authority_and_oversize_before_body() -> Result<()> {
        let (mut acceptor, address) = listener().await?;
        let id = session_id()?;
        let _download = open_get(address, &id).await?;
        let _accepted = acceptor.accept().await?;
        let base = format!("POST /tunnel/{id}");
        for (text, status) in [
            (
                format!("GET /tunnel/{id} HTTP/1.1\r\nX-Dagger-Upload: packet-up\r\n\r\n"),
                400,
            ),
            (
                format!(
                    "GET /tunnel/{id} HTTP/1.1\r\nHost: user@localhost\r\nX-Dagger-Upload: packet-up\r\n\r\n"
                ),
                400,
            ),
            (
                format!(
                    "GET /tunnel/{id} HTTP/1.1\r\nHost: localhost\r\nX-Dagger-Upload: packet-up\r\n\r\n"
                ),
                409,
            ),
            (
                format!(
                    "GET /tunnel/{id} HTTP/1.1\r\nHost: localhost\r\nX-Dagger-Upload: packet-up\r\nContent-Length: 0\r\n\r\n"
                ),
                409,
            ),
            (
                format!(
                    "GET /tunnel/{id} HTTP/1.1\r\nHost: localhost\r\nX-Dagger-Upload: packet-up\r\nContent-Length: 1\r\n\r\nx"
                ),
                400,
            ),
            (
                format!("{base}/+0 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\n\r\nx"),
                400,
            ),
            (
                format!("{base}/0 HTTP/1.1\r\nHost: localhost\r\nContent-Length: +1\r\n\r\nx"),
                400,
            ),
            (
                format!(
                    "{base}/0 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\nx"
                ),
                400,
            ),
            (
                format!("{base}/0 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 65537\r\n\r\n"),
                413,
            ),
            (
                format!("{base}/16 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\n\r\nx"),
                409,
            ),
        ] {
            let first = timeout(Duration::from_secs(2), request(address, text)).await??;
            ensure!(
                first.starts_with(&format!("HTTP/1.1 {status} ")),
                "unexpected rejection: {first}"
            );
        }
        ensure!(request(address, format!("{base}/0 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx")).await.is_err(), "duplicate Content-Length accepted");
        for host in [
            "user@localhost",
            "localhost:0",
            "localhost:65536",
            "bad host",
            "host/path",
            "host?query",
        ] {
            ensure!(
                http_authority(host).is_err(),
                "invalid HTTP authority accepted: {host}"
            );
        }
        for host in ["localhost", "cdn.test:8443", "127.0.0.1:8080", "[::1]:8080"] {
            http_authority(host)?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_tunnel_removes_session_and_allows_new_download() -> Result<()> {
        let (mut acceptor, address) = listener().await?;
        let id = session_id()?;
        let mut download = open_get(address, &id).await?;
        let accepted = acceptor.accept().await?;
        drop(accepted);
        // Cancellation must close the original HTTP response and remove its
        // registry entry, so this ID can open a new download immediately.
        ensure!(
            timeout(Duration::from_secs(2), download.read_u8())
                .await?
                .is_err(),
            "cancelled bridge retained network task"
        );
        let _download = timeout(Duration::from_secs(2), open_get(address, &id)).await??;
        let _accepted = timeout(Duration::from_secs(2), acceptor.accept()).await??;
        Ok(())
    }

    #[tokio::test]
    async fn packet_worker_honors_proxy_connection_close_and_empty_chunked_ack() -> Result<()> {
        let socket = TcpListener::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?;
        let id = session_id()?;
        let expected_id = id.clone();
        let server = tokio::spawn(async move {
            for sequence in 0..3 {
                let mut stream: BoxIo = Box::new(socket.accept().await?.0);
                let header = read_headers(&mut stream).await?;
                let parts: Vec<_> = header.first.split(' ').collect();
                ensure!(
                    parts.len() == 3
                        && parts[0] == "POST"
                        && request_path(parts[1])? == format!("/tunnel/{expected_id}/{sequence}"),
                    "proxy saw wrong packet sequence"
                );
                let size = if sequence == 2 { 0 } else { 3 };
                ensure!(
                    header.value("content-length") == Some(size.to_string().as_str()),
                    "proxy saw wrong packet length"
                );
                let mut body = vec![0; size];
                stream.read_exact(&mut body).await?;
                stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked\r\nConnection: close\r\n\r\n0;complete=yes\r\nAck: empty\r\n\r\n").await?;
                stream.flush().await?;
            }
            anyhow::Ok(())
        });
        let path: ClientPath = serde_json::from_value(
            serde_json::json!({"addr":address.to_string(),"transport":"xhttp","server_public_key":"01".repeat(32),"xhttp":{"mode":"packet-up","host":"localhost"}}),
        )?;
        let (sender, receiver) = mpsc::channel(3);
        for (sequence, body) in [
            (0, b"one".as_slice()),
            (1, b"two".as_slice()),
            (2, b"".as_slice()),
        ] {
            sender
                .send(Packet {
                    sequence,
                    body: body.to_vec(),
                })
                .await?;
        }
        drop(sender);
        timeout(
            Duration::from_secs(3),
            packet_worker(Arc::new(tokio::sync::Mutex::new(receiver)), path, id),
        )
        .await??;
        timeout(Duration::from_secs(3), server).await???;
        Ok(())
    }

    #[test]
    fn padding_bounds_and_http_header_injection_are_rejected() -> Result<()> {
        let options = XhttpOptions {
            pad_min: 8,
            pad_max: 48,
            ..XhttpOptions::default()
        };
        for _ in 0..64 {
            let url = pad_url("/tunnel/session/0", &options)?;
            ensure!(
                request_path(&url)? == "/tunnel/session/0",
                "padding changed tunnel route"
            );
            let padding = url.split_once("?v=").context("padding missing")?.1;
            ensure!(
                (8..=48).contains(&padding.len())
                    && padding.bytes().all(|b| b.is_ascii_alphanumeric()),
                "padding outside bounds"
            );
        }
        let disabled = XhttpOptions {
            pad_min: 0,
            pad_max: 0,
            ..XhttpOptions::default()
        };
        ensure!(
            pad_url("/tunnel/session/0", &disabled)? == "/tunnel/session/0",
            "disabled padding added query"
        );
        for (min, max) in [(49, 48), (0, 4097)] {
            let bad = XhttpOptions {
                pad_min: min,
                pad_max: max,
                ..XhttpOptions::default()
            };
            ensure!(
                pad_url("/route", &bad).is_err(),
                "invalid padding bounds accepted"
            );
        }
        for url in [
            "/route?unknown=1",
            "/route?v=pad&other=2",
            "/route?v=bad%0d%0a",
            "/route#fragment",
        ] {
            ensure!(request_path(url).is_err(), "malformed query accepted");
        }
        ensure!(
            request_path(&format!("/route?v={}", "x".repeat(4097))).is_err(),
            "unbounded query accepted"
        );
        for value in ["", "agent\r\nInjected: yes", "agent\tother", "🙂"] {
            ensure!(user_agent(value).is_err(), "unsafe User-Agent accepted");
        }
        user_agent("dagger-rs")?;
        Ok(())
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
