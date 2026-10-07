//! Authenticated reverse forwarding. Every task is owned by a enclosing JoinSet;
//! dropping `run` closes tunnels, listeners, and forwarded sockets.
use crate::{
    config::{ClientPath, Config, Listener, MapKind, Mode, decode_key},
    transport::{self, SecureConnection},
    wire::{DATA_SIZE, Frame, StreamKind},
};
use anyhow::{Context, Result, anyhow, bail};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Semaphore, mpsc, watch},
    task::JoinSet,
    time::{self, Instant},
};

const STREAM_QUEUE: usize = 32;
const SESSION_QUEUE: usize = 128;
const UDP_IDLE: Duration = Duration::from_secs(120);
const CREDITS: usize = 16;

struct StreamEntry {
    tx: mpsc::Sender<Frame>,
    credits: Arc<Semaphore>,
}

struct Session {
    tx: mpsc::Sender<Frame>,
    control: mpsc::Sender<Frame>,
    closed: watch::Sender<bool>,
    streams: Mutex<HashMap<u32, StreamEntry>>,
    alive: AtomicBool,
    next_id: AtomicU32,
    max_streams: usize,
}

impl Session {
    fn register(&self, requested: Option<u32>) -> Result<(u32, mpsc::Receiver<Frame>)> {
        let mut streams = self.streams.lock().expect("stream registry poisoned");
        if !self.alive.load(Ordering::Acquire) {
            bail!("tunnel disconnected");
        }
        if streams.len() >= self.max_streams {
            bail!("stream limit reached");
        }
        let id = match requested {
            Some(id) if id != 0 && !streams.contains_key(&id) => id,
            Some(_) => bail!("invalid or duplicate stream id"),
            None => loop {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                if id != 0 && !streams.contains_key(&id) {
                    break id;
                }
            },
        };
        let (tx, rx) = mpsc::channel(STREAM_QUEUE);
        streams.insert(
            id,
            StreamEntry {
                tx,
                credits: Arc::new(Semaphore::new(CREDITS)),
            },
        );
        Ok((id, rx))
    }

    async fn send(&self, frame: Frame) -> Result<()> {
        // EOF/Close/Open stay ordered behind stream data. Only out-of-band
        // credit and heartbeat frames may overtake queued data.
        let sender = if matches!(
            frame,
            Frame::Credit { .. } | Frame::Ping { .. } | Frame::Pong { .. }
        ) {
            &self.control
        } else {
            &self.tx
        };
        sender
            .send(frame)
            .await
            .map_err(|_| anyhow!("tunnel disconnected"))
    }

    async fn send_data(&self, id: u32, data: Vec<u8>) -> Result<()> {
        let credits = self
            .streams
            .lock()
            .expect("stream registry poisoned")
            .get(&id)
            .map(|entry| entry.credits.clone())
            .context("stream closed")?;
        credits
            .acquire_owned()
            .await
            .context("stream closed")?
            .forget();
        self.send(Frame::Data { id, data }).await
    }

    fn grant(&self, id: u32, frames: u32) -> Result<()> {
        let entries = self.streams.lock().expect("stream registry poisoned");
        if let Some(entry) = entries.get(&id) {
            if frames == 0 || frames as usize + entry.credits.available_permits() > CREDITS {
                bail!("invalid stream credit");
            }
            entry.credits.add_permits(frames as usize);
        }
        Ok(())
    }

    fn remove(&self, id: u32) {
        if let Some(entry) = self
            .streams
            .lock()
            .expect("stream registry poisoned")
            .remove(&id)
        {
            entry.credits.close();
        }
    }

    fn deliver(&self, id: u32, frame: Frame) {
        let tx = self
            .streams
            .lock()
            .expect("stream registry poisoned")
            .get(&id)
            .map(|entry| entry.tx.clone());
        if let Some(tx) = tx {
            if tx.try_send(frame).is_err() {
                // A slow consumer must not block all other multiplexed streams.
                self.remove(id);
                let _ = self.tx.try_send(Frame::Close { id });
            }
        }
    }
}

struct SessionGuard(Arc<Session>);
impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.alive.store(false, Ordering::Release);
        self.0.closed.send_replace(true);
        let mut streams = self.0.streams.lock().expect("stream registry poisoned");
        for entry in streams.values() {
            entry.credits.close();
        }
        streams.clear();
    }
}

struct StreamGuard {
    session: Arc<Session>,
    id: u32,
}
impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.session.remove(self.id);
        let _ = self.session.tx.try_send(Frame::Close { id: self.id });
    }
}

#[derive(Default)]
struct Pool {
    sessions: Mutex<Vec<Weak<Session>>>,
    turn: AtomicUsize,
}
impl Pool {
    fn insert(&self, session: &Arc<Session>) {
        let mut entries = self.sessions.lock().expect("session pool poisoned");
        entries.retain(|entry| {
            entry
                .upgrade()
                .is_some_and(|s| s.alive.load(Ordering::Acquire))
        });
        entries.push(Arc::downgrade(session));
    }
    fn pick(&self) -> Option<Arc<Session>> {
        let mut entries = self.sessions.lock().expect("session pool poisoned");
        entries.retain(|entry| {
            entry
                .upgrade()
                .is_some_and(|s| s.alive.load(Ordering::Acquire))
        });
        if entries.is_empty() {
            return None;
        }
        let index = self.turn.fetch_add(1, Ordering::Relaxed) % entries.len();
        // Prefer a session with capacity, while retaining round-robin ordering.
        (0..entries.len()).find_map(|offset| {
            let session = entries[(index + offset) % entries.len()].upgrade()?;
            let available = session
                .streams
                .lock()
                .expect("stream registry poisoned")
                .len()
                < session.max_streams;
            available.then_some(session)
        })
    }
}

pub async fn run(config: Config) -> Result<()> {
    config.validate()?;
    if config.tun.is_some() {
        #[cfg(target_os = "linux")]
        return crate::tun_device::run(config).await;
        #[cfg(not(target_os = "linux"))]
        bail!("TUN mode requires Linux");
    }
    run_forwarding(config).await
}

/// Runs normal authenticated forwarding, including the TCP overlay on a TUN.
pub(crate) async fn run_forwarding(config: Config) -> Result<()> {
    config.validate()?;
    if config.tun.is_some() {
        bail!("forwarding overlay must not contain TUN configuration");
    }
    let private = config.private_key()?;
    let config = Arc::new(config);
    let mut tasks = JoinSet::new();
    match config.mode {
        Mode::Server => {
            let all = Arc::new(Pool::default());
            let slots = Arc::new(Semaphore::new(config.max_connections));
            for listener in config.listeners.clone() {
                tasks.spawn(server_listener(
                    listener,
                    config.clone(),
                    private,
                    all.clone(),
                    slots.clone(),
                ));
            }
            if let Some(addr) = &config.socks5 {
                tasks.spawn(socks_listener(addr.clone(), all, config.max_streams));
            }
        }
        Mode::Client => {
            let slots = Arc::new(Semaphore::new(config.max_connections));
            for path in &config.paths {
                if config.reverse {
                    tasks.spawn(client_reverse_listener(
                        path.clone(),
                        config.clone(),
                        private,
                        slots.clone(),
                    ));
                    continue;
                }
                for _ in 0..path.connection_pool {
                    tasks.spawn(client_path(path.clone(), config.clone(), private));
                }
            }
        }
    }
    let result = tasks
        .join_next()
        .await
        .context("no tunnel tasks configured")?;
    result.context("tunnel task panicked")??;
    bail!("tunnel task exited unexpectedly")
}

async fn server_listener(
    listener: Listener,
    config: Arc<Config>,
    private: [u8; 32],
    all: Arc<Pool>,
    slots: Arc<Semaphore>,
) -> Result<()> {
    let mut socket = if config.reverse {
        None
    } else {
        Some(
            crate::kcp_carrier::Acceptor::bind(&listener)
                .await
                .with_context(|| format!("bind tunnel listener {}", listener.addr))?,
        )
    };
    let allowed: Vec<[u8; 32]> = config
        .peer_public_keys
        .iter()
        .map(|key| decode_key(key))
        .collect::<Result<_>>()?;
    let allowed = Arc::new(allowed);
    let local = Arc::new(Pool::default());
    let mut maps = JoinSet::new();
    for map in listener.maps.clone() {
        match map.kind {
            MapKind::Tcp => {
                maps.spawn(tcp_map(
                    map.bind,
                    map.target,
                    local.clone(),
                    config.max_streams,
                ));
            }
            MapKind::Udp => {
                maps.spawn(udp_map(
                    map.bind,
                    map.target,
                    local.clone(),
                    config.max_streams,
                ));
            }
        }
    }
    let mut connections = JoinSet::new();
    if config.reverse {
        for _ in 0..listener.connection_pool {
            connections.spawn(server_reverse_path(
                listener.clone(),
                config.clone(),
                private,
                allowed.clone(),
                local.clone(),
                all.clone(),
                slots.clone(),
            ));
        }
    }
    tracing::info!(address = %listener.addr, "tunnel listener ready");
    loop {
        tokio::select! {
            accepted = async { match &mut socket { Some(socket) => socket.accept().await, None => std::future::pending().await } } => {
                let stream = accepted?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let (config, allowed, local, all) = (config.clone(), allowed.clone(), local.clone(), all.clone());
                let listener = listener.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let secure = transport::accept_listener(stream, &listener, &private, &allowed, Duration::from_secs(10)).await?;
                    session_loop(secure, config, Some((local, all))).await
                });
            }
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(result) = result { match result {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => tracing::debug!(%error, "tunnel session closed"),
                    Err(error) => tracing::warn!(%error, "tunnel session task failed"),
                }}
            }
            result = maps.join_next(), if !maps.is_empty() => {
                result.context("map task missing")?.context("map task panicked")??;
                bail!("map listener stopped");
            }
        }
    }
}

async fn server_reverse_path(
    listener: Listener,
    config: Arc<Config>,
    private: [u8; 32],
    peers: Arc<Vec<[u8; 32]>>,
    local: Arc<Pool>,
    all: Arc<Pool>,
    slots: Arc<Semaphore>,
) -> Result<()> {
    let _permit = slots
        .acquire_owned()
        .await
        .context("connection slots closed")?;
    let mut retry = 1u64;
    loop {
        let started = Instant::now();
        match transport::connect_responder(&listener, &private, &peers).await {
            Ok(secure) => {
                tracing::info!(address = %listener.addr, "reverse tunnel authenticated");
                if let Err(error) =
                    session_loop(secure, config.clone(), Some((local.clone(), all.clone()))).await
                {
                    tracing::debug!(%error, "reverse session closed");
                }
            }
            Err(error) => tracing::debug!(%error, "reverse dial failed"),
        }
        if started.elapsed() >= Duration::from_secs(config.dead_timeout_sec) {
            retry = 1;
        }
        time::sleep(Duration::from_secs(retry)).await;
        retry = (retry * 2).min(30);
    }
}

async fn client_reverse_listener(
    path: ClientPath,
    config: Arc<Config>,
    private: [u8; 32],
    slots: Arc<Semaphore>,
) -> Result<()> {
    let mut acceptor = crate::kcp_carrier::Acceptor::bind(&path.listen_config()).await?;
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            incoming = acceptor.accept() => {
                let incoming = incoming?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let (path, config) = (path.clone(), config.clone());
                sessions.spawn(async move {
                    let _permit = permit;
                    let secure = transport::accept_initiator(incoming, &path, &private).await?;
                    session_loop(secure, config, None).await
                });
            }
            result = sessions.join_next(), if !sessions.is_empty() => {
                if let Some(result) = result { match result {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => tracing::debug!(%error, "reverse client session closed"),
                    Err(error) => tracing::warn!(%error, "reverse client session task failed"),
                }}
            }
        }
    }
}

async fn client_path(path: ClientPath, config: Arc<Config>, private: [u8; 32]) -> Result<()> {
    let base = path.retry_interval.clamp(1, 30);
    let mut retry = base;
    loop {
        let started = Instant::now();
        match transport::connect_path(&path, &private).await {
            Ok(connection) => {
                tracing::info!(address = %path.addr, "authenticated tunnel connected");
                if let Err(error) = session_loop(connection, config.clone(), None).await {
                    tracing::debug!(%error, "tunnel connection ended");
                }
            }
            Err(error) => tracing::debug!(address = %path.addr, %error, "tunnel connection failed"),
        }
        if started.elapsed() >= Duration::from_secs(config.dead_timeout_sec) {
            retry = base;
        }
        time::sleep(Duration::from_secs(retry)).await;
        retry = retry.saturating_mul(2).min(30);
    }
}

async fn session_loop(
    connection: SecureConnection,
    config: Arc<Config>,
    pools: Option<(Arc<Pool>, Arc<Pool>)>,
) -> Result<()> {
    let (mut reader, mut writer) = connection.split();
    let (tx, mut outgoing) = mpsc::channel(SESSION_QUEUE);
    let (control, mut controls) = mpsc::channel(SESSION_QUEUE);
    let (closed, _) = watch::channel(false);
    let session = Arc::new(Session {
        tx,
        control,
        closed,
        streams: Mutex::new(HashMap::new()),
        alive: AtomicBool::new(true),
        next_id: AtomicU32::new(1),
        max_streams: config.max_streams,
    });
    let _guard = SessionGuard(session.clone());
    let server = pools.is_some();
    if let Some((local, all)) = pools {
        local.insert(&session);
        all.insert(&session);
    }
    let (received, mut incoming) = mpsc::channel(SESSION_QUEUE);
    let dead = Duration::from_secs(config.dead_timeout_sec);
    let mut io = JoinSet::new();
    io.spawn(async move {
        loop {
            // Dedicated reader: never cancel a partially read encrypted frame.
            let frame = reader.recv().await?;
            if received.send(frame).await.is_err() {
                return Ok::<(), anyhow::Error>(());
            }
        }
    });
    io.spawn(async move {
        loop {
            let frame = tokio::select! {
                biased;
                frame = controls.recv() => frame,
                frame = outgoing.recv() => frame,
            };
            let Some(frame) = frame else {
                return Ok::<(), anyhow::Error>(());
            };
            time::timeout(dead, writer.send(&frame))
                .await
                .context("tunnel write timeout")??;
        }
    });
    let mut streams = JoinSet::new();
    let mut heartbeat = time::interval(Duration::from_secs(config.heartbeat_sec));
    heartbeat.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let mut last_received = Instant::now();
    let mut nonce = 0u64;
    loop {
        tokio::select! {
            frame = incoming.recv() => {
                let frame = frame.context("tunnel reader closed")?;
                last_received = Instant::now();
                match frame {
                    Frame::Ping { nonce } => { let _ = session.control.try_send(Frame::Pong { nonce }); }
                    Frame::Pong { .. } => {},
                    Frame::Credit { id, frames } => session.grant(id, frames)?,
                    Frame::Open { id, kind, target } => {
                        if server { bail!("client attempted to open a reverse stream"); }
                        if session.streams.lock().expect("stream registry poisoned").contains_key(&id) { bail!("duplicate stream id"); }
                        if streams.len() >= config.max_streams {
                            let _ = session.tx.try_send(Frame::Error { id, message: "stream capacity exhausted".into() });
                            continue;
                        }
                        if !config.allowed_targets.iter().any(|allowed| allowed == "*" || allowed == &target) {
                            let _ = session.tx.try_send(Frame::Error { id, message: "target not allowed".into() });
                            continue;
                        }
                        match session.register(Some(id)) {
                            Ok((_, rx)) => {
                                let session = session.clone();
                                streams.spawn(async move {
                                    let _guard = StreamGuard { session: session.clone(), id };
                                    if let Err(error) = close_stream(&session, id, target_stream(&session, id, kind, &target, rx)).await {
                                        tracing::debug!(%error, "target stream ended");
                                        let _ = session.tx.try_send(Frame::Error { id, message: "target unavailable".into() });
                                    }
                                });
                            }
                            Err(_) => { let _ = session.tx.try_send(Frame::Error { id, message: "invalid stream or capacity exhausted".into() }); }
                        }
                    }
                    Frame::Opened { id } => session.deliver(id, Frame::Opened { id }),
                    Frame::Data { id, data } => session.deliver(id, Frame::Data { id, data }),
                    Frame::Eof { id } => session.deliver(id, Frame::Eof { id }),
                    Frame::Close { id } => session.deliver(id, Frame::Close { id }),
                    Frame::Error { id, message } => session.deliver(id, Frame::Error { id, message }),
                }
            }
            _ = heartbeat.tick() => {
                if last_received.elapsed() >= dead { bail!("tunnel heartbeat timeout"); }
                nonce = nonce.wrapping_add(1);
                let _ = session.control.try_send(Frame::Ping { nonce });
            }
            result = io.join_next() => {
                result.context("tunnel I/O tasks exited")?.context("tunnel I/O task panicked")??;
                return Ok(());
            }
            result = streams.join_next(), if !streams.is_empty() => {
                if let Some(Err(error)) = result { tracing::warn!(%error, "target stream task failed"); }
            }
        }
    }
}

async fn await_open(rx: &mut mpsc::Receiver<Frame>) -> Result<()> {
    match time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .context("target open timeout")?
    {
        Some(Frame::Opened { .. }) => Ok(()),
        Some(Frame::Error { message, .. }) => bail!("target rejected: {message}"),
        _ => bail!("target closed before opening"),
    }
}

async fn relay_tcp(
    socket: TcpStream,
    session: &Session,
    id: u32,
    mut rx: mpsc::Receiver<Frame>,
) -> Result<()> {
    let (mut read, mut write) = socket.into_split();
    let upload = async {
        let mut buffer = vec![0; DATA_SIZE];
        loop {
            let count = read.read(&mut buffer).await?;
            if count == 0 {
                session.send(Frame::Eof { id }).await?;
                return Ok::<(), anyhow::Error>(());
            }
            session.send_data(id, buffer[..count].to_vec()).await?;
        }
    };
    let download = async {
        loop {
            match rx.recv().await {
                Some(Frame::Data { data, .. }) => {
                    write.write_all(&data).await?;
                    session.send(Frame::Credit { id, frames: 1 }).await?;
                }
                Some(Frame::Eof { .. }) => {
                    write.shutdown().await?;
                    return Ok::<(), anyhow::Error>(());
                }
                Some(Frame::Close { .. }) | None => bail!("remote stream closed"),
                Some(Frame::Error { message, .. }) => bail!("remote stream error: {message}"),
                _ => bail!("invalid frame for TCP stream state"),
            }
        }
    };
    // Keep receiving/crediting while upload waits for its own credit. Both
    // directions must reach EOF before returning (TCP half-close semantics).
    tokio::try_join!(upload, download)?;
    Ok(())
}

async fn session_scope<F>(session: &Session, future: F) -> Result<()>
where
    F: std::future::Future<Output = Result<()>>,
{
    let mut closed = session.closed.subscribe();
    if *closed.borrow() {
        bail!("tunnel disconnected");
    }
    tokio::select! {
        result = future => result,
        _ = closed.changed() => bail!("tunnel disconnected"),
    }
}

async fn close_stream<F>(session: &Session, id: u32, future: F) -> Result<()>
where
    F: std::future::Future<Output = Result<()>>,
{
    let result = future.await;
    // Queue Close after preceding data/EOF even when the output queue is full.
    // The Drop guard is only a best-effort fallback for cancellation.
    let _ = session.send(Frame::Close { id }).await;
    result
}

async fn target_stream(
    session: &Session,
    id: u32,
    kind: StreamKind,
    target: &str,
    rx: mpsc::Receiver<Frame>,
) -> Result<()> {
    match kind {
        StreamKind::Tcp => {
            let socket = time::timeout(Duration::from_secs(10), TcpStream::connect(target))
                .await
                .context("target connect timeout")??;
            socket.set_nodelay(true)?;
            session.send(Frame::Opened { id }).await?;
            relay_tcp(socket, session, id, rx).await
        }
        StreamKind::Udp => {
            let addr = time::timeout(Duration::from_secs(10), tokio::net::lookup_host(target))
                .await
                .context("UDP target lookup timeout")??
                .next()
                .context("UDP target has no address")?;
            let socket = UdpSocket::bind(if addr.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            })
            .await?;
            socket.connect(addr).await?;
            session.send(Frame::Opened { id }).await?;
            target_udp(socket, session, id, rx).await
        }
    }
}

async fn target_udp(
    socket: UdpSocket,
    session: &Session,
    id: u32,
    mut rx: mpsc::Receiver<Frame>,
) -> Result<()> {
    let activity = Mutex::new(Instant::now());
    let upload = async {
        let mut buffer = vec![0; 65536];
        loop {
            let count = socket.recv(&mut buffer).await?;
            *activity.lock().expect("UDP activity poisoned") = Instant::now();
            if count <= DATA_SIZE {
                session.send_data(id, buffer[..count].to_vec()).await?;
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let download = async {
        loop {
            match rx.recv().await {
                Some(Frame::Data { data, .. }) => {
                    socket.send(&data).await?;
                    *activity.lock().expect("UDP activity poisoned") = Instant::now();
                    session.send(Frame::Credit { id, frames: 1 }).await?;
                }
                Some(Frame::Close { .. }) | Some(Frame::Eof { .. }) | None => {
                    return Ok::<(), anyhow::Error>(());
                }
                Some(Frame::Error { message, .. }) => bail!("remote UDP error: {message}"),
                _ => bail!("invalid frame for UDP stream"),
            }
        }
    };
    tokio::select! {
        result = upload => result,
        result = download => result,
        _ = udp_idle(&activity) => Ok(()),
    }
}

async fn udp_idle(activity: &Mutex<Instant>) {
    loop {
        let deadline = *activity.lock().expect("UDP activity poisoned") + UDP_IDLE;
        time::sleep_until(deadline).await;
        if activity.lock().expect("UDP activity poisoned").elapsed() >= UDP_IDLE {
            return;
        }
    }
}

async fn tcp_map(bind: String, target: String, pool: Arc<Pool>, limit: usize) -> Result<()> {
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind TCP map {bind}"))?;
    let permits = Arc::new(Semaphore::new(limit));
    let mut tasks = JoinSet::new();
    tracing::info!(address = %bind, "TCP map ready");
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                let Some(session) = pool.pick() else { continue; };
                let Ok((id, mut rx)) = session.register(None) else { continue; };
                let guard = StreamGuard { session: session.clone(), id };
                let target = target.clone();
                tasks.spawn(async move {
                    let (_permit, _guard) = (permit, guard);
                    session_scope(&session, close_stream(&session, id, async {
                        socket.set_nodelay(true)?;
                        session.send(Frame::Open { id, kind: StreamKind::Tcp, target }).await?;
                        await_open(&mut rx).await?;
                        relay_tcp(socket, &session, id, rx).await
                    })).await
                });
            }
            result = tasks.join_next(), if !tasks.is_empty() => { log_forward_result(result); }
        }
    }
}

fn log_forward_result(result: Option<std::result::Result<Result<()>, tokio::task::JoinError>>) {
    match result {
        Some(Ok(Err(error))) => tracing::debug!(%error, "forwarded stream ended"),
        Some(Err(error)) => tracing::warn!(%error, "forwarding task failed"),
        _ => {}
    }
}

async fn udp_map(bind: String, target: String, pool: Arc<Pool>, limit: usize) -> Result<()> {
    let socket = Arc::new(
        UdpSocket::bind(&bind)
            .await
            .with_context(|| format!("bind UDP map {bind}"))?,
    );
    let mut peers: HashMap<SocketAddr, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut tasks = JoinSet::new();
    let mut buffer = vec![0; 65536];
    tracing::info!(address = %bind, "UDP map ready");
    loop {
        tokio::select! {
            packet = socket.recv_from(&mut buffer) => {
                let (count, peer) = packet?;
                if count > DATA_SIZE { continue; }
                peers.retain(|_, sender| !sender.is_closed());
                if let Some(sender) = peers.get(&peer) {
                    let _ = sender.try_send(buffer[..count].to_vec());
                    continue;
                }
                if peers.len() >= limit { continue; }
                let Some(session) = pool.pick() else { continue; };
                let Ok((id, mut rx)) = session.register(None) else { continue; };
                let guard = StreamGuard { session: session.clone(), id };
                let (tx, mut packets) = mpsc::channel(STREAM_QUEUE);
                tx.try_send(buffer[..count].to_vec()).expect("new UDP queue has capacity");
                peers.insert(peer, tx);
                let (socket, target) = (socket.clone(), target.clone());
                tasks.spawn(async move {
                    let _guard = guard;
                    session_scope(&session, close_stream(&session, id, async {
                        session.send(Frame::Open { id, kind: StreamKind::Udp, target }).await?;
                        await_open(&mut rx).await?;
                        let activity = Mutex::new(Instant::now());
                        let upload = async {
                            while let Some(data) = packets.recv().await {
                                session.send_data(id, data).await?;
                                *activity.lock().expect("UDP activity poisoned") = Instant::now();
                            }
                            Ok::<(), anyhow::Error>(())
                        };
                        let download = async {
                            loop {
                                match rx.recv().await {
                                    Some(Frame::Data { data, .. }) => {
                                        socket.send_to(&data, peer).await?;
                                        *activity.lock().expect("UDP activity poisoned") = Instant::now();
                                        session.send(Frame::Credit { id, frames: 1 }).await?;
                                    }
                                    Some(Frame::Close { .. }) | Some(Frame::Eof { .. }) | None => return Ok::<(), anyhow::Error>(()),
                                    Some(Frame::Error { message, .. }) => bail!("remote UDP error: {message}"),
                                    _ => bail!("invalid UDP stream frame"),
                                }
                            }
                        };
                        tokio::select! {
                            result = upload => result,
                            result = download => result,
                            _ = udp_idle(&activity) => Ok(()),
                        }
                    })).await
                });
            }
            result = tasks.join_next(), if !tasks.is_empty() => { log_forward_result(result); }
        }
    }
}

async fn socks_listener(bind: String, pool: Arc<Pool>, limit: usize) -> Result<()> {
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind SOCKS5 listener {bind}"))?;
    let permits = Arc::new(Semaphore::new(limit));
    let mut tasks = JoinSet::new();
    tracing::info!(address = %bind, "SOCKS5 listener ready");
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                let pool = pool.clone();
                tasks.spawn(async move { let _permit = permit; socks_stream(socket, pool).await });
            }
            result = tasks.join_next(), if !tasks.is_empty() => { log_forward_result(result); }
        }
    }
}

async fn socks_request(socket: &mut TcpStream) -> Result<String> {
    let mut hello = [0; 2];
    socket.read_exact(&mut hello).await?;
    if hello[0] != 5 || hello[1] == 0 {
        bail!("invalid SOCKS5 greeting");
    }
    let mut methods = vec![0; hello[1] as usize];
    socket.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        socket.write_all(&[5, 255]).await?;
        bail!("SOCKS5 requires no-auth method");
    }
    socket.write_all(&[5, 0]).await?;
    let mut request = [0; 4];
    socket.read_exact(&mut request).await?;
    if request[0] != 5 || request[2] != 0 {
        bail!("invalid SOCKS5 request");
    }
    if request[1] != 1 {
        socks_reply(socket, 7).await?;
        bail!("only SOCKS5 CONNECT is supported");
    }
    let host = match request[3] {
        1 => {
            let mut octets = [0; 4];
            socket.read_exact(&mut octets).await?;
            std::net::Ipv4Addr::from(octets).to_string()
        }
        4 => {
            let mut octets = [0; 16];
            socket.read_exact(&mut octets).await?;
            format!("[{}]", std::net::Ipv6Addr::from(octets))
        }
        3 => {
            let size = socket.read_u8().await? as usize;
            if size == 0 {
                bail!("empty SOCKS5 hostname");
            }
            let mut bytes = vec![0; size];
            socket.read_exact(&mut bytes).await?;
            let host = String::from_utf8(bytes).context("SOCKS5 hostname must be UTF-8")?;
            if host.contains([':', '/', '\\', '\0']) {
                bail!("invalid SOCKS5 hostname");
            }
            host
        }
        _ => {
            socks_reply(socket, 8).await?;
            bail!("unsupported SOCKS5 address type");
        }
    };
    let port = socket.read_u16().await?;
    if port == 0 {
        bail!("SOCKS5 port must be nonzero");
    }
    Ok(format!("{host}:{port}"))
}

async fn socks_reply(socket: &mut TcpStream, status: u8) -> Result<()> {
    socket
        .write_all(&[5, status, 0, 1, 0, 0, 0, 0, 0, 0])
        .await?;
    Ok(())
}

async fn socks_stream(mut socket: TcpStream, pool: Arc<Pool>) -> Result<()> {
    socket.set_nodelay(true)?;
    let target = time::timeout(Duration::from_secs(10), socks_request(&mut socket))
        .await
        .context("SOCKS5 handshake timeout")??;
    let Some(session) = pool.pick() else {
        socks_reply(&mut socket, 3).await?;
        bail!("no authenticated tunnel available");
    };
    let (id, mut rx) = match session.register(None) {
        Ok(stream) => stream,
        Err(error) => {
            socks_reply(&mut socket, 1).await?;
            return Err(error);
        }
    };
    let _guard = StreamGuard {
        session: session.clone(),
        id,
    };
    session_scope(
        &session,
        close_stream(&session, id, async {
            session
                .send(Frame::Open {
                    id,
                    kind: StreamKind::Tcp,
                    target,
                })
                .await?;
            if let Err(error) = await_open(&mut rx).await {
                socks_reply(&mut socket, 2).await?;
                return Err(error);
            }
            socks_reply(&mut socket, 0).await?;
            relay_tcp(socket, &session, id, rx).await
        }),
    )
    .await
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
