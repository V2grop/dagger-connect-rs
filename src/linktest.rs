//! Explicit-peer link diagnostics. Only operator-selected endpoints are contacted.
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket, TcpStream, UdpSocket},
    sync::{Notify, Semaphore},
    task::JoinSet,
    time::{self, Instant},
};

const MAGIC: &[u8; 5] = b"DRLT\x01";
const SESSION: u8 = 0;
const ECHO: u8 = 1;
const UPLOAD: u8 = 2;
const DOWNLOAD: u8 = 3;
const REVERSE_TCP: u8 = 4;
const REVERSE_UDP: u8 = 5;
const FINISH: u8 = 6;
const BLOCK: usize = 32_768;
const MAX_FRAME: usize = 65_536;
const MAX_TRANSFER_FRAMES: u64 = 8192; // 256 MiB per direction
const UDP_HEADER: usize = 41; // magic(5), token(32), counter(4)

#[derive(Clone, Debug)]
pub struct ListenOptions {
    pub bind: SocketAddr,
    pub expected_peer: IpAddr,
    pub extra_ports: Vec<u16>,
    pub wait: Duration,
    pub keep: bool,
}
#[derive(Clone, Debug)]
pub struct ProbeOptions {
    pub peer: SocketAddr,
    pub bind: IpAddr,
    pub local_port: u16,
    pub extra_ports: Vec<u16>,
    pub seconds: Duration,
    pub timeout: Duration,
    pub quick: bool,
}
impl ListenOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.expected_peer.is_unspecified() && !self.expected_peer.is_multicast(),
            "linktest requires one explicit unicast peer IP"
        );
        ensure!(
            self.bind.is_ipv4() == self.expected_peer.is_ipv4(),
            "linktest bind and peer IP families differ"
        );
        ensure!(
            (Duration::from_secs(1)..=Duration::from_secs(3600)).contains(&self.wait),
            "linktest wait must be 1..3600 seconds"
        );
        check_ports(&self.extra_ports)
    }
}
impl ProbeOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.peer.port() != 0
                && !self.peer.ip().is_unspecified()
                && !self.peer.ip().is_multicast(),
            "linktest requires an explicit unicast peer and nonzero port"
        );
        ensure!(
            self.bind.is_ipv4() == self.peer.is_ipv4(),
            "linktest bind and peer IP families differ"
        );
        ensure!(
            (Duration::from_millis(50)..=Duration::from_secs(30)).contains(&self.seconds),
            "linktest throughput duration must be 0.05..30 seconds"
        );
        ensure!(
            (Duration::from_millis(50)..=Duration::from_secs(60)).contains(&self.timeout),
            "linktest timeout must be 0.05..60 seconds"
        );
        check_ports(&self.extra_ports)
    }
}
fn check_ports(ports: &[u16]) -> Result<()> {
    ensure!(
        ports.len() <= 16 && ports.iter().all(|p| *p != 0),
        "linktest permits at most 16 explicit nonzero extra ports"
    );
    ensure!(
        ports.iter().copied().collect::<HashSet<_>>().len() == ports.len(),
        "linktest extra ports must be unique"
    );
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub ok: bool,
    pub round_trips: usize,
    pub rtt_ms: f64,
    pub error: Option<String>,
}
impl CheckResult {
    fn failure(error: impl std::fmt::Display) -> Self {
        Self {
            ok: false,
            round_trips: 0,
            rtt_ms: 0.,
            error: Some(error.to_string()),
        }
    }
}
#[derive(Debug, Serialize)]
pub struct DatagramResult {
    pub sent: usize,
    pub received: usize,
    pub loss_percent: f64,
    pub integrity: bool,
    pub rtt_ms: f64,
    pub error: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct ThroughputResult {
    pub bytes: u64,
    pub seconds: f64,
    pub mbps: f64,
    pub integrity: bool,
    pub error: Option<String>,
}
impl ThroughputResult {
    fn new(bytes: u64, seconds: f64, integrity: bool) -> Self {
        Self {
            bytes,
            seconds,
            mbps: bytes as f64 * 8. / seconds.max(0.000001) / 1_000_000.,
            integrity,
            error: None,
        }
    }
    fn failure(error: impl std::fmt::Display) -> Self {
        Self {
            bytes: 0,
            seconds: 0.,
            mbps: 0.,
            integrity: false,
            error: Some(error.to_string()),
        }
    }
}
#[derive(Debug, Serialize)]
pub struct PortResult {
    pub port: u16,
    pub tcp: CheckResult,
    pub udp: DatagramResult,
}
#[derive(Debug, Serialize)]
pub struct LinkReport {
    pub peer: SocketAddr,
    pub reverse_listener: SocketAddr,
    pub tcp: CheckResult,
    pub udp: DatagramResult,
    pub reverse_tcp: CheckResult,
    pub reverse_udp: DatagramResult,
    pub upload: ThroughputResult,
    pub download: ThroughputResult,
    pub ports: Vec<PortResult>,
}
impl LinkReport {
    pub fn passed(&self) -> bool {
        self.tcp.ok
            && self.reverse_tcp.ok
            && datagrams_passed(&self.udp)
            && datagrams_passed(&self.reverse_udp)
            && self.upload.integrity
            && self.download.integrity
            && self.upload.bytes > 0
            && self.download.bytes > 0
            && self.upload.error.is_none()
            && self.download.error.is_none()
            && self
                .ports
                .iter()
                .all(|p| p.tcp.ok && datagrams_passed(&p.udp))
    }
}
fn datagrams_passed(result: &DatagramResult) -> bool {
    result.sent > 0 && result.received == result.sent && result.integrity && result.error.is_none()
}

struct Session {
    peer: IpAddr,
    expires: Instant,
    udp_left: usize,
    tcp_left: usize,
}
struct State {
    sessions: Mutex<HashMap<[u8; 32], Session>>,
    peer: IpAddr,
    finished: Notify,
    keep: bool,
}
impl State {
    fn register(&self, token: [u8; 32]) -> Result<()> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("linktest session lock poisoned"))?;
        sessions.retain(|_, v| v.expires > Instant::now());
        ensure!(sessions.len() < 64, "linktest session capacity reached");
        sessions.insert(
            token,
            Session {
                peer: self.peer,
                expires: Instant::now() + Duration::from_secs(180),
                udp_left: 4096,
                tcp_left: 128,
            },
        );
        Ok(())
    }
    fn valid(&self, token: &[u8; 32], peer: IpAddr, udp: bool) -> bool {
        if peer != self.peer {
            return false;
        }
        let Ok(mut sessions) = self.sessions.lock() else {
            return false;
        };
        if let Some(session) = sessions.get_mut(token) {
            if session.peer != peer || session.expires <= Instant::now() {
                return false;
            }
            if udp {
                if session.udp_left == 0 {
                    return false;
                }
                session.udp_left -= 1;
            } else {
                if session.tcp_left == 0 {
                    return false;
                }
                session.tcp_left -= 1;
            }
            true
        } else {
            false
        }
    }
}
fn random_token() -> Result<[u8; 32]> {
    let mut token = [0; 32];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut token)
        .map_err(|_| anyhow::anyhow!("linktest random token generation failed"))?;
    Ok(token)
}

/// Listen until the time limit or an authorized probe finishes (unless keep).
pub async fn listen(options: ListenOptions) -> Result<()> {
    options.validate()?;
    let state = Arc::new(State {
        sessions: Mutex::new(HashMap::new()),
        peer: options.expected_peer,
        finished: Notify::new(),
        keep: options.keep,
    });
    let mut tasks = JoinSet::new();
    let base = spawn_pair(options.bind, state.clone(), &mut tasks).await?;
    for port in options.extra_ports {
        if port != base.port() {
            spawn_pair(SocketAddr::new(base.ip(), port), state.clone(), &mut tasks).await?;
        }
    }
    tracing::info!(%base,peer=%options.expected_peer,"local linktest listeners ready");
    tokio::select! {
        _=time::sleep(options.wait)=>{},
        _=state.finished.notified()=>{},
        result=tasks.join_next()=> {if let Some(result)=result {result??;}}
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}
async fn spawn_pair(
    bind: SocketAddr,
    state: Arc<State>,
    tasks: &mut JoinSet<Result<()>>,
) -> Result<SocketAddr> {
    let tcp = TcpListener::bind(bind)
        .await
        .context("bind linktest TCP listener")?;
    let address = tcp.local_addr()?;
    let udp = UdpSocket::bind(address)
        .await
        .context("bind linktest UDP listener")?;
    tasks.spawn(tcp_server(tcp, state.clone()));
    tasks.spawn(udp_server(udp, state));
    Ok(address)
}
async fn tcp_server(listener: TcpListener, state: Arc<State>) -> Result<()> {
    let limit = Arc::new(Semaphore::new(16));
    let mut handlers = JoinSet::new();
    loop {
        tokio::select! {
            result=listener.accept()=> {
                let (stream,peer)=result?;
                if peer.ip()!=state.peer {continue;}
                let Ok(permit)=limit.clone().try_acquire_owned()else {continue};
                let state=state.clone();handlers.spawn(async move {
                    let _permit=permit;
                    let _=time::timeout(Duration::from_secs(90),handle_tcp(stream,peer,state)).await;
                });
            },
            _=handlers.join_next(),if !handlers.is_empty()=>{}
        }
    }
}
async fn udp_server(socket: UdpSocket, state: Arc<State>) -> Result<()> {
    let mut bytes = vec![0; 8193];
    loop {
        let (size, peer) = socket.recv_from(&mut bytes).await?;
        if !(UDP_HEADER..=8192).contains(&size) || bytes[..5] != *MAGIC {
            continue;
        }
        let token: [u8; 32] = bytes[5..37].try_into()?;
        if state.valid(&token, peer.ip(), true) {
            socket.send_to(&bytes[..size], peer).await?;
        }
    }
}
async fn handle_tcp(mut stream: TcpStream, peer: SocketAddr, state: Arc<State>) -> Result<()> {
    let mut header = [0; 38];
    stream.read_exact(&mut header).await?;
    ensure!(header[..5] == *MAGIC, "invalid linktest request");
    let token: [u8; 32] = header[6..].try_into()?;
    if header[5] == SESSION {
        let token = random_token()?;
        state.register(token)?;
        stream.write_all(&token).await?;
        return Ok(());
    }
    ensure!(
        state.valid(&token, peer.ip(), false),
        "unknown/expired linktest session"
    );
    match header[5] {
        ECHO => {
            let frame = read_frame(&mut stream).await?;
            write_frame(&mut stream, &frame).await?;
        }
        UPLOAD => {
            let (bytes, valid) = receive_transfer(&mut stream).await?;
            stream.write_u64(bytes).await?;
            stream.write_u8(u8::from(valid)).await?;
        }
        DOWNLOAD => {
            let millis = stream.read_u32().await?;
            ensure!((50..=30_000).contains(&millis), "invalid linktest duration");
            send_transfer(&mut stream, Duration::from_millis(u64::from(millis))).await?;
        }
        REVERSE_TCP => {
            let port = stream.read_u16().await?;
            ensure!(port != 0, "reverse port cannot be zero");
            let start = Instant::now();
            let result = tcp_check(
                SocketAddr::new(peer.ip(), port),
                stream.local_addr()?.ip(),
                token,
                Duration::from_secs(5),
            )
            .await;
            stream.write_u8(u8::from(result.ok)).await?;
            stream.write_u64(start.elapsed().as_micros() as u64).await?;
        }
        REVERSE_UDP => {
            let port = stream.read_u16().await?;
            ensure!(port != 0, "reverse port cannot be zero");
            let count = usize::from(stream.read_u8().await?);
            ensure!((1..=32).contains(&count), "invalid reverse datagram count");
            let socket = UdpSocket::bind(SocketAddr::new(stream.local_addr()?.ip(), 0)).await?;
            let result = udp_check(
                &socket,
                SocketAddr::new(peer.ip(), port),
                token,
                count,
                Duration::from_secs(5),
            )
            .await;
            stream.write_u32(result.received as u32).await?;
            stream.write_u8(u8::from(result.integrity)).await?;
            stream.write_u64((result.rtt_ms * 1000.) as u64).await?;
        }
        FINISH => {
            stream.write_u8(1).await?;
            if !state.keep {
                state.finished.notify_one();
            }
        }
        _ => anyhow::bail!("unknown linktest operation"),
    }
    Ok(())
}
async fn open(peer: SocketAddr, bind: IpAddr, token: [u8; 32], operation: u8) -> Result<TcpStream> {
    let socket = if peer.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.bind(SocketAddr::new(bind, 0))?;
    let mut stream = socket.connect(peer).await?;
    stream.set_nodelay(true)?;
    stream.write_all(MAGIC).await?;
    stream.write_u8(operation).await?;
    stream.write_all(&token).await?;
    Ok(stream)
}
async fn tcp_check(
    peer: SocketAddr,
    bind: IpAddr,
    token: [u8; 32],
    deadline: Duration,
) -> CheckResult {
    let start = Instant::now();
    let work = async {
        let mut stream = open(peer, bind, token, ECHO).await?;
        let bytes: Vec<_> = (0..4096).map(|i| (i % 251) as u8).collect();
        write_frame(&mut stream, &bytes).await?;
        ensure!(
            read_frame(&mut stream).await? == bytes,
            "TCP probe integrity mismatch"
        );
        Ok::<_, anyhow::Error>(())
    };
    match time::timeout(deadline, work).await {
        Ok(Ok(())) => CheckResult {
            ok: true,
            round_trips: 1,
            rtt_ms: start.elapsed().as_secs_f64() * 1000.,
            error: None,
        },
        Ok(Err(e)) => CheckResult::failure(e),
        Err(e) => CheckResult::failure(e),
    }
}
async fn udp_check(
    socket: &UdpSocket,
    peer: SocketAddr,
    token: [u8; 32],
    count: usize,
    deadline: Duration,
) -> DatagramResult {
    let start = Instant::now();
    let mut received = HashSet::new();
    let mut integrity = true;
    let mut sum = 0.;
    let mut sent_at = Vec::new();
    let mut error = None;
    for sequence in 0..count {
        let mut packet = vec![0; UDP_HEADER + 512];
        packet[..5].copy_from_slice(MAGIC);
        packet[5..37].copy_from_slice(&token);
        packet[37..41].copy_from_slice(&(sequence as u32).to_be_bytes());
        for (index, byte) in packet[41..].iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        let now = Instant::now();
        if let Err(e) = socket.send_to(&packet, peer).await {
            error = Some(e.to_string());
            break;
        }
        sent_at.push(now);
    }
    let end = start + deadline;
    let mut packet = [0; 8193];
    let sent = sent_at.len();
    while received.len() < sent {
        match time::timeout_at(end, socket.recv_from(&mut packet)).await {
            Ok(Ok((size, from))) => {
                if from != peer
                    || size != UDP_HEADER + 512
                    || packet[..5] != *MAGIC
                    || packet[5..37] != token
                {
                    continue;
                }
                let sequence = u32::from_be_bytes(packet[37..41].try_into().unwrap()) as usize;
                if sequence >= sent {
                    continue;
                }
                integrity &= packet[41..size]
                    .iter()
                    .enumerate()
                    .all(|(i, b)| *b == (i % 251) as u8);
                if received.insert(sequence) {
                    sum += sent_at[sequence].elapsed().as_secs_f64() * 1000.;
                }
            }
            Ok(Err(e)) => {
                error = Some(e.to_string());
                break;
            }
            Err(_) => {
                break;
            }
        }
    }
    DatagramResult {
        sent,
        received: received.len(),
        loss_percent: (sent - received.len()) as f64 / sent.max(1) as f64 * 100.,
        integrity,
        rtt_ms: sum / received.len().max(1) as f64,
        error,
    }
}
async fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_FRAME, "linktest frame exceeds limit");
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(bytes).await?;
    Ok(())
}
async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let size = stream.read_u32().await? as usize;
    ensure!(size <= MAX_FRAME, "linktest frame exceeds limit");
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}
async fn send_transfer(stream: &mut TcpStream, duration: Duration) -> Result<u64> {
    let mut block: Vec<_> = (0..BLOCK).map(|i| (i % 251) as u8).collect();
    let end = Instant::now() + duration;
    let mut sequence = 0u64;
    let mut bytes = 0;
    while Instant::now() < end && sequence < MAX_TRANSFER_FRAMES {
        block[..8].copy_from_slice(&sequence.to_be_bytes());
        write_frame(stream, &block).await?;
        sequence += 1;
        bytes += BLOCK as u64;
    }
    write_frame(stream, &[]).await?;
    Ok(bytes)
}
async fn receive_transfer(stream: &mut TcpStream) -> Result<(u64, bool)> {
    let mut sequence = 0u64;
    let mut bytes = 0;
    let mut valid = true;
    loop {
        let block = read_frame(stream).await?;
        if block.is_empty() {
            break;
        }
        ensure!(
            sequence < MAX_TRANSFER_FRAMES,
            "linktest transfer exceeds 256 MiB limit"
        );
        ensure!(block.len() == BLOCK, "invalid linktest transfer block size");
        valid &= u64::from_be_bytes(block[..8].try_into()?) == sequence
            && block[8..]
                .iter()
                .enumerate()
                .all(|(i, b)| *b == ((i + 8) % 251) as u8);
        bytes += block.len() as u64;
        sequence += 1;
    }
    Ok((bytes, valid))
}

/// Executes actual two-way probes, reverse inbound connectivity and TCP transfers.
pub async fn probe(options: ProbeOptions) -> Result<LinkReport> {
    options.validate()?;
    let mut hello = time::timeout(
        options.timeout,
        open(options.peer, options.bind, [0; 32], SESSION),
    )
    .await??;
    let mut token = [0; 32];
    time::timeout(options.timeout, hello.read_exact(&mut token)).await??;
    let state = Arc::new(State {
        sessions: Mutex::new(HashMap::new()),
        peer: options.peer.ip(),
        finished: Notify::new(),
        keep: true,
    });
    state.register(token)?;
    let mut reverse = JoinSet::new();
    let reverse_listener = spawn_pair(
        SocketAddr::new(options.bind, options.local_port),
        state,
        &mut reverse,
    )
    .await?;
    let socket = UdpSocket::bind(SocketAddr::new(options.bind, 0)).await?;
    let count = if options.quick { 4 } else { 16 };
    let tcp = tcp_check(options.peer, options.bind, token, options.timeout).await;
    let udp = udp_check(&socket, options.peer, token, count, options.timeout).await;
    let reverse_tcp_work = async {
        let mut s = open(options.peer, options.bind, token, REVERSE_TCP).await?;
        s.write_u16(reverse_listener.port()).await?;
        let ok = s.read_u8().await? != 0;
        let rtt = s.read_u64().await?;
        Ok::<_, anyhow::Error>(CheckResult {
            ok,
            round_trips: usize::from(ok),
            rtt_ms: rtt as f64 / 1000.,
            error: (!ok).then(|| "reverse TCP connection/integrity check failed".into()),
        })
    };
    let reverse_tcp =
        match time::timeout(options.timeout + Duration::from_secs(5), reverse_tcp_work).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => CheckResult::failure(e),
            Err(e) => CheckResult::failure(e),
        };
    let reverse_udp_work = async {
        let mut s = open(options.peer, options.bind, token, REVERSE_UDP).await?;
        s.write_u16(reverse_listener.port()).await?;
        s.write_u8(count as u8).await?;
        let received = s.read_u32().await? as usize;
        ensure!(received <= count, "invalid reverse UDP result");
        let integrity = s.read_u8().await? != 0;
        let rtt = s.read_u64().await?;
        Ok::<_, anyhow::Error>(DatagramResult {
            sent: count,
            received,
            loss_percent: (count - received) as f64 / count as f64 * 100.,
            integrity,
            rtt_ms: rtt as f64 / 1000.,
            error: None,
        })
    };
    let reverse_udp =
        match time::timeout(options.timeout + Duration::from_secs(5), reverse_udp_work).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => DatagramResult {
                sent: count,
                received: 0,
                loss_percent: 100.,
                integrity: false,
                rtt_ms: 0.,
                error: Some(e.to_string()),
            },
            Err(e) => DatagramResult {
                sent: count,
                received: 0,
                loss_percent: 100.,
                integrity: false,
                rtt_ms: 0.,
                error: Some(e.to_string()),
            },
        };
    let duration = if options.quick {
        options.seconds.min(Duration::from_millis(250))
    } else {
        options.seconds
    };
    let upload_work = async {
        let mut s = open(options.peer, options.bind, token, UPLOAD).await?;
        let start = Instant::now();
        let sent = send_transfer(&mut s, duration).await?;
        let got = s.read_u64().await?;
        let valid = s.read_u8().await? != 0;
        Ok::<_, anyhow::Error>(ThroughputResult::new(
            got,
            start.elapsed().as_secs_f64(),
            valid && got == sent,
        ))
    };
    let upload = match time::timeout(duration + options.timeout, upload_work).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => ThroughputResult::failure(e),
        Err(e) => ThroughputResult::failure(e),
    };
    let download_work = async {
        let mut s = open(options.peer, options.bind, token, DOWNLOAD).await?;
        s.write_u32(duration.as_millis() as u32).await?;
        let start = Instant::now();
        let (bytes, valid) = receive_transfer(&mut s).await?;
        Ok::<_, anyhow::Error>(ThroughputResult::new(
            bytes,
            start.elapsed().as_secs_f64(),
            valid,
        ))
    };
    let download = match time::timeout(duration + options.timeout, download_work).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => ThroughputResult::failure(e),
        Err(e) => ThroughputResult::failure(e),
    };
    let mut ports = Vec::new();
    for port in options.extra_ports {
        let address = SocketAddr::new(options.peer.ip(), port);
        ports.push(PortResult {
            port,
            tcp: tcp_check(address, options.bind, token, options.timeout).await,
            udp: udp_check(&socket, address, token, count, options.timeout).await,
        });
    }
    let _ = time::timeout(options.timeout, async {
        let mut s = open(options.peer, options.bind, token, FINISH).await?;
        s.read_u8().await?;
        Ok::<_, anyhow::Error>(())
    })
    .await;
    reverse.abort_all();
    while reverse.join_next().await.is_some() {}
    Ok(LinkReport {
        peer: options.peer,
        reverse_listener,
        tcp,
        udp,
        reverse_tcp,
        reverse_udp,
        upload,
        download,
        ports,
    })
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
