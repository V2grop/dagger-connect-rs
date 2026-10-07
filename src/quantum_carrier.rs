//! Quantum family: KCP with the recovered 10-data/1-parity FEC envelope.
//! Quantum+ uses UDP and an optional P/A knock. Identity and payload encryption
//! are provided by Noise; the upstream PSK header cipher/token is omitted.
use crate::{
    carrier::BoxIo,
    config::{ClientPath, Listener, Transport},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::UdpSocket,
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, timeout},
};
use tokio_kcp::{KcpConfig, KcpListener, KcpNoDelayConfig, KcpStream};

#[path = "quantum_tuning.rs"]
mod tuning;
pub use tuning::{QuantumTunerMode, QuantumTunerOptions};

const DATA: usize = 10;
const SHARDS: u32 = 11;
const TYPE_DATA: u16 = 0xf1;
const TYPE_PARITY: u16 = 0xf2;
const PAWS: u32 = u32::MAX / SHARDS * SHARDS;
// Row ten of V(11,10) * inverse(V(10,10)), GF(256), polynomial 0x11d.
const PARITY_COEFFICIENTS: [u8; DATA] = [129, 150, 175, 184, 210, 196, 254, 232, 3, 2];
const MAX_GROUPS: usize = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuantumProfile {
    #[default]
    Default,
    Gaming,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuantumOptions {
    pub mtu: usize,
    pub peer_mtu: Option<usize>,
    pub profile: QuantumProfile,
    pub knock: bool,
    pub knock_timeout_ms: u64,
    pub max_peers: usize,
    pub sndwnd: u16,
    pub rcvwnd: u16,
    pub socket_buf_bytes: usize,
    pub tuner: QuantumTunerOptions,
    pub nodelay: Option<bool>,
    pub interval_ms: Option<u32>,
    pub resend: Option<u16>,
    pub congestion_control: Option<bool>,
    pub write_delay: Option<bool>,
    pub ack_no_delay: Option<bool>,
}
impl Default for QuantumOptions {
    fn default() -> Self {
        Self {
            mtu: 1350,
            peer_mtu: None,
            profile: QuantumProfile::Default,
            knock: true,
            knock_timeout_ms: 3000,
            max_peers: 256,
            sndwnd: 1024,
            rcvwnd: 1024,
            socket_buf_bytes: 4 * 1024 * 1024,
            tuner: QuantumTunerOptions::default(),
            nodelay: None,
            interval_ms: None,
            resend: None,
            congestion_control: None,
            write_delay: None,
            ack_no_delay: None,
        }
    }
}
impl QuantumOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (512..=9000).contains(&self.mtu),
            "quantum mtu must be 512..9000"
        );
        ensure!(
            self.peer_mtu.is_none_or(|mtu| (512..=9000).contains(&mtu)),
            "quantum peer_mtu must be 512..9000 when set"
        );
        ensure!(
            (500..=30_000).contains(&self.knock_timeout_ms),
            "quantum knock_timeout_ms must be 500..30000"
        );
        ensure!(
            (1..=4096).contains(&self.max_peers),
            "quantum max_peers must be 1..4096"
        );
        ensure!(
            (128..=8192).contains(&self.sndwnd) && (128..=8192).contains(&self.rcvwnd),
            "quantum sndwnd/rcvwnd must be 128..8192"
        );
        ensure!(
            (256 * 1024..=64 * 1024 * 1024).contains(&self.socket_buf_bytes),
            "quantum socket_buf_bytes must be 256 KiB..64 MiB"
        );
        self.tuner.validate()?;
        ensure!(
            self.interval_ms
                .is_none_or(|interval| (10..=5000).contains(&interval)),
            "quantum interval_ms must be 10..5000 when set"
        );
        ensure!(
            self.resend.is_none_or(|resend| resend <= 255),
            "quantum resend must be 0..255 when set"
        );
        if self.tuner.mode == QuantumTunerMode::Auto {
            ensure!(
                self.tuner.memory_budget_mb * 1024 * 1024 >= 4 * self.minimum_allocation(),
                "quantum tuner memory budget must cover one conversation's minimum allocation"
            );
            ensure!(
                self.tuner.max_window_bytes >= 256 * (self.envelope_mtu(false) - 8).max(512),
                "quantum tuner max_window_bytes must cover the 256-segment minimum at the configured MTU"
            );
        }
        Ok(())
    }
    fn envelope_mtu(&self, raw: bool) -> usize {
        if raw {
            (self.mtu.min(self.peer_mtu.unwrap_or(self.mtu)) - 100).clamp(512, 1500)
        } else {
            self.mtu.min(1500)
        }
    }
    fn kcp(&self, raw: bool) -> KcpConfig {
        let gaming = self.profile == QuantumProfile::Gaming;
        let cap = if gaming { 1024 } else { 8192 };
        let wnd_size = if self.tuner.mode == QuantumTunerMode::Auto {
            let budget = tuning::window_budget(
                self.tuner.min_window_bytes,
                self.envelope_mtu(raw) - 8,
                gaming,
            );
            (budget, budget)
        } else {
            (self.sndwnd.min(cap), self.rcvwnd.min(cap))
        };
        KcpConfig {
            mtu: self.envelope_mtu(raw) - 8,
            stream: true,
            wnd_size,
            nodelay: KcpNoDelayConfig {
                nodelay: self.nodelay.unwrap_or(!raw || gaming),
                interval: self.interval_ms.unwrap_or(10) as i32,
                resend: self.resend.unwrap_or(2) as i32,
                nc: !self.congestion_control.unwrap_or(false),
            },
            session_expire: Duration::from_secs(60),
            flush_write: !self.write_delay.unwrap_or(raw && !gaming),
            flush_acks_input: self.ack_no_delay.unwrap_or(!raw),
            ..KcpConfig::default()
        }
    }
    fn minimum_allocation(&self) -> usize {
        // Reserve enough for the 256-segment floor at either raw/UDP MTU.
        self.tuner
            .min_buffer_bytes
            .max(self.tuner.min_window_bytes)
            .max(256 * (self.envelope_mtu(false) - 8).max(512))
    }
    fn peer_limit(&self) -> usize {
        if self.tuner.mode == QuantumTunerMode::Auto {
            self.max_peers
                .min(self.tuner.memory_budget_mb * 1024 * 1024 / (4 * self.minimum_allocation()))
        } else {
            self.max_peers
        }
    }
    fn initial_buffer(&self) -> usize {
        if self.tuner.mode == QuantumTunerMode::Auto {
            self.socket_buf_bytes
                .clamp(self.tuner.min_buffer_bytes, self.tuner.max_buffer_bytes)
                .min(self.tuner.memory_budget_mb * 1024 * 1024 / 4)
        } else {
            self.socket_buf_bytes
        }
    }
}

enum NetworkSocket {
    Udp(UdpSocket),
    #[cfg(target_os = "linux")]
    Raw(crate::raw_socket::RawSocket),
}
impl NetworkSocket {
    async fn recv_from(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        match self {
            Self::Udp(socket) => socket.recv_from(buffer).await,
            #[cfg(target_os = "linux")]
            Self::Raw(socket) => socket.recv_from(buffer).await,
        }
    }
    async fn send_to(&self, data: &[u8], peer: SocketAddr) -> io::Result<usize> {
        match self {
            Self::Udp(socket) => socket.send_to(data, peer).await,
            #[cfg(target_os = "linux")]
            Self::Raw(socket) => socket.send_to(data, peer).await,
        }
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Self::Udp(socket) => socket.local_addr(),
            #[cfg(target_os = "linux")]
            Self::Raw(socket) => Ok(socket.local_addr()),
        }
    }
    fn set_socket_buffer_bytes(&self, bytes: usize) -> io::Result<()> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Raw(socket) => socket.set_socket_buffer_bytes(bytes),
            Self::Udp(socket) => {
                #[cfg(target_os = "linux")]
                {
                    use std::os::fd::AsRawFd;
                    let value = i32::try_from(bytes).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "socket buffer too large")
                    })?;
                    for option in [libc::SO_RCVBUF, libc::SO_SNDBUF] {
                        // The pointer addresses a live i32 for the duration of this syscall.
                        let result = unsafe {
                            libc::setsockopt(
                                socket.as_raw_fd(),
                                libc::SOL_SOCKET,
                                option,
                                (&value as *const i32).cast(),
                                std::mem::size_of::<i32>() as libc::socklen_t,
                            )
                        };
                        if result < 0 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                }
                #[cfg(not(target_os = "linux"))]
                let _ = (socket, bytes);
                Ok(())
            }
        }
    }
}

async fn raw_network(
    raw: Option<&crate::raw_packet::RawOptions>,
    server: bool,
    mtu: usize,
    socket_buf: usize,
) -> Result<NetworkSocket> {
    #[cfg(target_os = "linux")]
    {
        let mut options = raw.context("raw carrier options required")?.clone();
        options.sock_buf = socket_buf;
        let socket = crate::raw_socket::RawSocket::bind_quantum(&options, server).await?;
        ensure!(
            mtu <= socket.max_payload(),
            "quantum mtu exceeds raw interface capacity"
        );
        Ok(NetworkSocket::Raw(socket))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (raw, server, mtu, socket_buf);
        anyhow::bail!("quantum raw carrier requires Linux")
    }
}

fn gf_multiply(mut a: u8, mut b: u8) -> u8 {
    let mut out = 0;
    while b != 0 {
        if b & 1 != 0 {
            out ^= a;
        }
        let high = a & 0x80 != 0;
        a <<= 1;
        if high {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    out
}
fn gf_inverse(a: u8) -> u8 {
    let mut out = 1;
    for _ in 0..254 {
        out = gf_multiply(out, a);
    }
    out
}

#[derive(Default)]
struct FecEncoder {
    next: u32,
    shards: Vec<Vec<u8>>,
}
impl FecEncoder {
    fn encode(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        let size = (data.len() + 2) as u16;
        let mut shard = Vec::with_capacity(data.len() + 2);
        shard.extend(size.to_le_bytes());
        shard.extend_from_slice(data);
        let mut output = vec![self.envelope(TYPE_DATA, &shard)];
        self.shards.push(shard);
        if self.shards.len() == DATA {
            let mut parity = vec![0; self.shards.iter().map(Vec::len).max().unwrap_or(0)];
            for (index, shard) in self.shards.iter().enumerate() {
                for (position, &byte) in shard.iter().enumerate() {
                    parity[position] ^= gf_multiply(PARITY_COEFFICIENTS[index], byte);
                }
            }
            output.push(self.envelope(TYPE_PARITY, &parity));
            self.shards.clear();
        }
        output
    }
    fn envelope(&mut self, kind: u16, shard: &[u8]) -> Vec<u8> {
        let mut packet = Vec::with_capacity(shard.len() + 6);
        packet.extend(self.next.to_le_bytes());
        packet.extend(kind.to_le_bytes());
        packet.extend_from_slice(shard);
        self.next += 1;
        if self.next == PAWS {
            self.next = 0;
        }
        packet
    }
}
struct FecGroup {
    id: u32,
    shards: [Option<Vec<u8>>; 11],
    delivered: [bool; 10],
}
impl FecGroup {
    fn new(id: u32) -> Self {
        Self {
            id,
            shards: std::array::from_fn(|_| None),
            delivered: [false; 10],
        }
    }
}
#[derive(Default)]
struct FecDecoder {
    groups: VecDeque<FecGroup>,
}
impl FecDecoder {
    fn decode(&mut self, packet: &[u8], mtu: usize) -> Vec<Vec<u8>> {
        if packet.len() < 8 || packet.len() > mtu {
            return vec![];
        }
        let seq = u32::from_le_bytes(packet[0..4].try_into().unwrap());
        let kind = u16::from_le_bytes(packet[4..6].try_into().unwrap());
        let slot = (seq % SHARDS) as usize;
        if seq >= PAWS || !(slot < DATA && kind == TYPE_DATA || slot == DATA && kind == TYPE_PARITY)
        {
            return vec![];
        }
        if kind == TYPE_DATA && shard_payload(&packet[6..]).is_none() {
            return vec![];
        }
        let id = seq / SHARDS;
        let index = if let Some(index) = self.groups.iter().position(|g| g.id == id) {
            index
        } else {
            if self.groups.len() == MAX_GROUPS {
                self.groups.pop_front();
            }
            self.groups.push_back(FecGroup::new(id));
            self.groups.len() - 1
        };
        let group = &mut self.groups[index];
        if group.shards[slot].is_some() {
            return vec![];
        }
        group.shards[slot] = Some(packet[6..].to_vec());
        let mut output = Vec::new();
        if slot < DATA {
            output.push(
                shard_payload(group.shards[slot].as_ref().unwrap())
                    .unwrap()
                    .to_vec(),
            );
            group.delivered[slot] = true;
        }
        let missing: Vec<_> = (0..DATA).filter(|&i| group.shards[i].is_none()).collect();
        if missing.len() == 1 && group.shards[DATA].is_some() {
            let parity = group.shards[DATA].as_ref().unwrap();
            let lost = missing[0];
            let mut recovered = parity.clone();
            for (index, shard) in group.shards[..DATA].iter().enumerate() {
                if let Some(shard) = shard {
                    if shard.len() > recovered.len() {
                        return output;
                    }
                    for (position, &byte) in shard.iter().enumerate() {
                        recovered[position] ^= gf_multiply(PARITY_COEFFICIENTS[index], byte);
                    }
                }
            }
            let inverse = gf_inverse(PARITY_COEFFICIENTS[lost]);
            for byte in &mut recovered {
                *byte = gf_multiply(*byte, inverse);
            }
            if let Some(payload) = shard_payload(&recovered) {
                if !group.delivered[lost] {
                    output.push(payload.to_vec());
                    group.delivered[lost] = true;
                }
                group.shards[lost] = Some(recovered);
            }
        }
        output
    }
}
fn shard_payload(shard: &[u8]) -> Option<&[u8]> {
    if shard.len() < 2 {
        return None;
    }
    let size = u16::from_le_bytes(shard[..2].try_into().ok()?) as usize;
    (size >= 2 && size <= shard.len()).then(|| &shard[2..size])
}

fn fec_conversation(packet: &[u8], mtu: usize) -> Option<u32> {
    if packet.len() < 32 || packet.len() > mtu {
        return None;
    }
    let seq = u32::from_le_bytes(packet[..4].try_into().ok()?);
    let kind = u16::from_le_bytes(packet[4..6].try_into().ok()?);
    let slot = (seq % SHARDS) as usize;
    if seq >= PAWS || !(slot < DATA && kind == TYPE_DATA || slot == DATA && kind == TYPE_PARITY) {
        return None;
    }
    if kind == TYPE_DATA && shard_payload(&packet[6..])?.len() < 24 {
        return None;
    }
    // All KCP packets in a FEC group share a conversation. The parity row
    // sums to one in GF(256), so these four bytes are preserved in parity.
    let conversation = u32::from_le_bytes(packet[8..12].try_into().ok()?);
    (conversation != 0).then_some(conversation)
}

fn knock_address(address: SocketAddr) -> SocketAddr {
    let port = if address.port() <= 55_535 {
        address.port() + 10_000
    } else {
        address.port() - 1000
    };
    SocketAddr::new(address.ip(), port)
}
async fn resolve(addr: &str) -> Result<SocketAddr> {
    tokio::net::lookup_host(addr)
        .await
        .context("resolve quantum peer")?
        .next()
        .context("quantum peer has no address")
}
async fn knock(peer: SocketAddr, total: Duration) -> Result<()> {
    let socket = UdpSocket::bind(if peer.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })
    .await?;
    socket.connect(knock_address(peer)).await?;
    let attempt = (total / 3).clamp(Duration::from_millis(500), Duration::from_secs(3));
    for index in 0..3 {
        socket.send(b"P").await?;
        let mut response = [0; 32];
        if let Ok(Ok(size)) = timeout(attempt, socket.recv(&mut response)).await {
            if size > 0 && response[0] == b'A' {
                return Ok(());
            }
        }
        if index < 2 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    anyhow::bail!("quantum+ knock timed out")
}

struct TaskGuard(JoinHandle<()>);
impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct GuardedStream {
    stream: KcpStream,
    _bridge: Option<TaskGuard>,
    _tuner: Option<TaskGuard>,
    measurements: Option<tuning::SharedMeasurements>,
}
impl AsyncRead for GuardedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = out.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, out);
        if let Some(measurements) = &self.measurements {
            measurements
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .read(out.filled().len() - before);
        }
        result
    }
}
impl AsyncWrite for GuardedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        // KCP limits the number of fragments accepted by one send call.
        // AsyncWrite may accept a prefix; write_all handles the remainder.
        let size = bytes.len().min(16 * 1024);
        let result = Pin::new(&mut self.stream).poll_write(cx, &bytes[..size]);
        if let Poll::Ready(Ok(size)) = &result {
            if let Some(measurements) = &self.measurements {
                measurements
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .written(*size);
            }
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

pub async fn connect(path: &ClientPath) -> Result<BoxIo> {
    path.quantum.validate()?;
    let raw = matches!(
        path.transport,
        Transport::Quantum | Transport::QuantumGaming
    );
    let mut options = path.quantum.clone();
    if path.transport == Transport::QuantumGaming {
        options.profile = QuantumProfile::Gaming;
    }
    let peer = if raw {
        let settings = path.raw.as_ref().context("raw carrier options required")?;
        SocketAddr::new(IpAddr::V4(settings.peer_ip), settings.l4_port)
    } else {
        resolve(&path.addr).await?
    };
    if !raw && options.knock {
        knock(peer, Duration::from_millis(options.knock_timeout_ms)).await?;
    }
    let mtu = options.envelope_mtu(raw);
    let outside = Arc::new(if raw {
        raw_network(path.raw.as_ref(), false, mtu, options.initial_buffer()).await?
    } else {
        let socket = UdpSocket::bind(if peer.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        })
        .await?;
        socket.connect(peer).await?;
        NetworkSocket::Udp(socket)
    });
    outside.set_socket_buffer_bytes(options.initial_buffer())?;
    let bridge = UdpSocket::bind("127.0.0.1:0").await?;
    let kcp = UdpSocket::bind("127.0.0.1:0").await?;
    let kcp_address = kcp.local_addr()?;
    bridge.connect(kcp_address).await?;
    let bridge_address = bridge.local_addr()?;
    let stream = KcpStream::connect_with_socket(&options.kcp(raw), kcp, bridge_address)
        .await
        .context("open quantum KCP session")?;
    let conversation = stream.session().conv().await;
    let measurements = tuning::Measurements::shared(&options);
    let registry = TuningRegistry::default();
    if let Some(stats) = &measurements {
        registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(bridge_address, stats.clone());
    }
    let buffers = tuning::SocketBuffers::shared(registry, options.initial_buffer());
    let controller = measurements.as_ref().map(|measurements| {
        TaskGuard(tuning::Control::spawn(
            measurements.clone(),
            options.clone(),
            outside.clone(),
            Arc::new(AtomicUsize::new(1)),
            buffers,
            raw,
            stream.shared_session(),
        ))
    });
    let bridge_measurements = measurements.clone();
    let task = tokio::spawn(async move {
        let mut inside_buffer = vec![0; mtu];
        let mut outside_buffer = vec![0; mtu + 1];
        let mut encoder = FecEncoder::default();
        let mut decoder = FecDecoder::default();
        loop {
            let result: io::Result<()> = tokio::select! {
                read=bridge.recv(&mut inside_buffer)=>async {
                    let size=read?;
                    if let Some(stats)=&bridge_measurements {stats.lock().unwrap_or_else(|e|e.into_inner()).observe(&inside_buffer[..size],true,Instant::now());}
                    for packet in encoder.encode(&inside_buffer[..size]) { outside.send_to(&packet,peer).await?; }
                    Ok(())
                }.await,
                read=outside.recv_from(&mut outside_buffer)=>async {
                    let (size,_)=read?;
                    if fec_conversation(&outside_buffer[..size],mtu)!=Some(conversation) {return Ok(());}
                    for packet in decoder.decode(&outside_buffer[..size],mtu) {
                        if let Some(stats)=&bridge_measurements {stats.lock().unwrap_or_else(|e|e.into_inner()).observe(&packet,false,Instant::now());}
                        bridge.send(&packet).await?;
                    }
                    Ok(())
                }.await,
            };
            if let Err(error) = result {
                tracing::debug!(%error,"quantum bridge stopped");
                break;
            }
        }
    });
    let guard = TaskGuard(task);
    Ok(Box::new(GuardedStream {
        stream,
        _bridge: Some(guard),
        _tuner: controller,
        measurements,
    }))
}

struct Peer {
    socket: Arc<UdpSocket>,
    encoder: FecEncoder,
    decoder: FecDecoder,
    last: Instant,
    _relay: TaskGuard,
    measurements: Option<tuning::SharedMeasurements>,
    relay_address: SocketAddr,
    registry: TuningRegistry,
    population: Arc<AtomicUsize>,
}
type TuningRegistry = Arc<Mutex<HashMap<SocketAddr, tuning::SharedMeasurements>>>;
impl Drop for Peer {
    fn drop(&mut self) {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.relay_address);
        self.population.fetch_sub(1, Ordering::Relaxed);
    }
}
pub struct QuantumListener {
    listener: KcpListener,
    _bridge: TaskGuard,
    address: SocketAddr,
    registry: TuningRegistry,
    options: QuantumOptions,
    outside: Arc<NetworkSocket>,
    population: Arc<AtomicUsize>,
    raw: bool,
    buffers: Arc<tuning::SocketBuffers>,
}
impl QuantumListener {
    pub async fn bind(settings: &Listener) -> Result<Self> {
        settings.quantum.validate()?;
        let raw = matches!(
            settings.transport,
            Transport::Quantum | Transport::QuantumGaming
        );
        let mut options = settings.quantum.clone();
        if settings.transport == Transport::QuantumGaming {
            options.profile = QuantumProfile::Gaming;
        }
        if raw {
            options.knock = false;
        }
        let mtu = options.envelope_mtu(raw);
        let outside = Arc::new(if raw {
            raw_network(settings.raw.as_ref(), true, mtu, options.initial_buffer()).await?
        } else {
            NetworkSocket::Udp(
                UdpSocket::bind(&settings.addr)
                    .await
                    .context("bind quantum+ UDP listener")?,
            )
        });
        outside.set_socket_buffer_bytes(options.initial_buffer())?;
        let address = outside.local_addr()?;
        let knock_socket = if options.knock {
            Some(
                UdpSocket::bind(knock_address(address))
                    .await
                    .context("bind quantum+ knock listener")?,
            )
        } else {
            None
        };
        let listener = KcpListener::bind(options.kcp(raw), "127.0.0.1:0").await?;
        let inside_address = listener.local_addr()?;
        let registry = TuningRegistry::default();
        let task_registry = registry.clone();
        let buffers = tuning::SocketBuffers::shared(registry.clone(), options.initial_buffer());
        let population = Arc::new(AtomicUsize::new(0));
        let task_population = population.clone();
        let task_outside = outside.clone();
        let task_options = options.clone();
        let task = tokio::spawn(async move {
            let outside = task_outside;
            let options = task_options;
            let (return_tx, mut return_rx) = mpsc::channel::<((SocketAddr, u32), Vec<u8>)>(256);
            let mut peers: HashMap<(SocketAddr, u32), Peer> = HashMap::new();
            let mut knocks: HashMap<IpAddr, Instant> = HashMap::new();
            let mut outside_buffer = vec![0; mtu + 1];
            let mut knock_buffer = [0; 64];
            let mut cleanup = tokio::time::interval(Duration::from_secs(5));
            loop {
                let result: io::Result<()> = tokio::select! {
                    read=outside.recv_from(&mut outside_buffer)=>async {
                        let (size,remote)=read?;
                        let Some(conversation)=fec_conversation(&outside_buffer[..size],mtu) else {return Ok(());};
                        let key=(remote,conversation);
                        if !peers.contains_key(&key) {
                            if options.knock && !knocks.contains_key(&remote.ip()) { return Ok(()); }
                            if peers.len()>=options.peer_limit() { return Ok(()); }
                            let mut decoder=FecDecoder::default();
                            let packets=decoder.decode(&outside_buffer[..size],mtu);
                            if packets.iter().all(|p|p.len()<24) { return Ok(()); }
                            let socket=Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
                            let relay_address=socket.local_addr()?;
                            socket.connect(inside_address).await?;
                            let relay_socket=socket.clone(); let sender=return_tx.clone();
                            let relay=tokio::spawn(async move {
                                let mut buffer=vec![0;mtu];
                                while let Ok(size)=relay_socket.recv(&mut buffer).await {
                                    if sender.send((key,buffer[..size].to_vec())).await.is_err() { break; }
                                }
                            });
                            let measurements=tuning::Measurements::shared(&options);
                            if let Some(stats)=&measurements {
                                task_registry.lock().unwrap_or_else(|e|e.into_inner()).insert(relay_address,stats.clone());
                            }
                            task_population.fetch_add(1,Ordering::Relaxed);
                            // Install ownership before forwarding: an I/O error still drops the
                            // registry entry and population count with the peer map.
                            peers.insert(key,Peer {socket,encoder:FecEncoder::default(),decoder,last:Instant::now(),_relay:TaskGuard(relay),
                                measurements,relay_address,registry:task_registry.clone(),population:task_population.clone()});
                            let peer=peers.get_mut(&key).unwrap();
                            for packet in packets { if packet.len()>=24 {
                                if let Some(stats)=&peer.measurements {stats.lock().unwrap_or_else(|e|e.into_inner()).observe(&packet,false,Instant::now());}
                                peer.socket.send(&packet).await?;
                            } }
                        } else if let Some(peer)=peers.get_mut(&key) {
                            for packet in peer.decoder.decode(&outside_buffer[..size],mtu) {
                                if packet.len()>=24 {
                                    if let Some(stats)=&peer.measurements {stats.lock().unwrap_or_else(|e|e.into_inner()).observe(&packet,false,Instant::now());}
                                    peer.socket.send(&packet).await?; peer.last=Instant::now();
                                }
                            }
                        }
                        Ok(())
                    }.await,
                    Some((key,packet))=return_rx.recv()=>async {
                        if let Some(peer)=peers.get_mut(&key) {
                            if let Some(stats)=&peer.measurements {stats.lock().unwrap_or_else(|e|e.into_inner()).observe(&packet,true,Instant::now());}
                            for packet in peer.encoder.encode(&packet) { outside.send_to(&packet,key.0).await?; }
                            peer.last=Instant::now();
                        }
                        Ok(())
                    }.await,
                    read=async { match &knock_socket { Some(socket)=>socket.recv_from(&mut knock_buffer).await,
                        None=>std::future::pending().await } }=>async {
                        let (size,remote)=read?;
                        if size==1 && knock_buffer[0]==b'P' && (knocks.len()<options.max_peers || knocks.contains_key(&remote.ip())) {
                            knocks.insert(remote.ip(),Instant::now());
                            if let Some(socket)=&knock_socket { socket.send_to(b"A",remote).await?; }
                        }
                        Ok(())
                    }.await,
                    _=cleanup.tick()=>{ peers.retain(|_,p|p.last.elapsed()<Duration::from_secs(60));
                        knocks.retain(|_,time|time.elapsed()<Duration::from_secs(30)); Ok(()) },
                };
                if let Err(error) = result {
                    tracing::debug!(%error,"quantum listener bridge stopped");
                    break;
                }
            }
        });
        Ok(Self {
            listener,
            _bridge: TaskGuard(task),
            address,
            registry,
            options,
            outside,
            population,
            raw,
            buffers,
        })
    }
    pub async fn accept(&mut self) -> Result<BoxIo> {
        let (stream, relay_address) = self.listener.accept().await?;
        let measurements = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&relay_address)
            .cloned();
        let controller = measurements.as_ref().map(|stats| {
            TaskGuard(tuning::Control::spawn(
                stats.clone(),
                self.options.clone(),
                self.outside.clone(),
                self.population.clone(),
                self.buffers.clone(),
                self.raw,
                stream.shared_session(),
            ))
        });
        Ok(Box::new(GuardedStream {
            stream,
            _bridge: None,
            _tuner: controller,
            measurements,
        }))
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parity_matches_independent_vandermonde_reference_and_recovers_each_loss() {
        let mut encoder = FecEncoder::default();
        let mut packets = Vec::new();
        for index in 0..10 {
            packets.extend(encoder.encode(&(0..=index).collect::<Vec<u8>>()));
        }
        assert_eq!(
            &packets[10][6..],
            &hex::decode("8e0000802cd604b872a50812").unwrap()
        );
        for lost in 0..10 {
            let mut decoder = FecDecoder::default();
            let mut recovered = Vec::new();
            for (index, packet) in packets.iter().enumerate() {
                if index != lost {
                    recovered.extend(decoder.decode(packet, 1350));
                }
            }
            assert_eq!(recovered.len(), 10, "loss at shard {lost}");
            assert!(recovered.contains(&(0..=lost as u8).collect::<Vec<_>>()));
        }
    }
    #[test]
    fn fec_data_and_parity_preserve_conversation_for_demultiplexing() {
        let conversation = 0x12345678_u32;
        let mut encoder = FecEncoder::default();
        let mut packets = Vec::new();
        for index in 0..10 {
            let mut payload = vec![index as u8; 24 + index];
            payload[..4].copy_from_slice(&conversation.to_le_bytes());
            packets.extend(encoder.encode(&payload));
        }
        assert_eq!(packets.len(), 11);
        for packet in packets {
            assert_eq!(fec_conversation(&packet, 1350), Some(conversation));
        }
    }
    #[test]
    fn malformed_fec_and_replays_do_not_escape_or_grow_storage() {
        let mut decoder = FecDecoder::default();
        let mut encoder = FecEncoder::default();
        let packet = encoder.encode(b"valid").remove(0);
        assert_eq!(decoder.decode(&packet, 1350), vec![b"valid".to_vec()]);
        assert!(decoder.decode(&packet, 1350).is_empty());
        for size in 0..8 {
            assert!(decoder.decode(&packet[..size], 1350).is_empty());
        }
        let mut invalid = packet.clone();
        invalid[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(decoder.decode(&invalid, 1350).is_empty());
        invalid = packet.clone();
        invalid[..4].copy_from_slice(&10_u32.to_le_bytes());
        assert!(decoder.decode(&invalid, 1350).is_empty());
        for seq in (0..1000).step_by(11) {
            let mut next = packet.clone();
            next[..4].copy_from_slice(&(seq as u32).to_le_bytes());
            decoder.decode(&next, 1350);
            assert!(decoder.groups.len() <= MAX_GROUPS);
        }
    }
    #[test]
    fn knock_port_wrap_and_profiles_match_recovered_parameters() {
        assert_eq!(
            knock_address("127.0.0.1:1234".parse().unwrap()).port(),
            11_234
        );
        assert_eq!(
            knock_address("127.0.0.1:60000".parse().unwrap()).port(),
            59_000
        );
        let mut options = QuantumOptions::default();
        assert!(!options.kcp(true).nodelay.nodelay);
        assert!(options.kcp(false).nodelay.nodelay);
        options.profile = QuantumProfile::Gaming;
        assert!(options.kcp(true).nodelay.nodelay);
        assert_eq!(options.kcp(true).nodelay.interval, 10);
        assert_eq!(options.kcp(true).nodelay.resend, 2);
        assert!(options.kcp(true).nodelay.nc);
        assert_eq!(options.kcp(false).mtu, 1342);
        assert!(!options.kcp(true).flush_acks_input);
        assert!(options.kcp(false).flush_acks_input);
        assert_eq!(options.kcp(true).mtu, 1242);
        options.mtu = 512;
        assert_eq!(options.kcp(true).mtu, 504);
        options.mtu = 9000;
        assert_eq!(options.kcp(true).mtu, 1492);
    }
    #[test]
    fn configured_windows_and_shared_auto_budget_are_bounded() {
        let mut options = QuantumOptions {
            sndwnd: 2048,
            rcvwnd: 4096,
            ..QuantumOptions::default()
        };
        assert_eq!(options.kcp(false).wnd_size, (2048, 4096));
        options.profile = QuantumProfile::Gaming;
        assert_eq!(options.kcp(true).wnd_size, (1024, 1024));
        options.tuner.mode = QuantumTunerMode::Auto;
        options.tuner.memory_budget_mb = 8;
        options.max_peers = 4096;
        assert!(options.peer_limit() < 4096);
        assert!(options.peer_limit() * 4 * options.minimum_allocation() <= 8 * 1024 * 1024);
        assert!(options.kcp(false).wnd_size.0 <= 1024);
        options.tuner.min_buffer_bytes = 64 * 1024 * 1024;
        options.tuner.max_buffer_bytes = 64 * 1024 * 1024;
        assert!(options.validate().is_err());
    }
    #[test]
    fn timing_overrides_map_to_kcp_flags_and_preserve_peer_mtu_minimum() {
        let mut options = QuantumOptions {
            nodelay: Some(false),
            interval_ms: Some(50),
            resend: Some(0),
            congestion_control: Some(true),
            write_delay: Some(true),
            ack_no_delay: Some(false),
            peer_mtu: Some(1000),
            ..QuantumOptions::default()
        };
        options.validate().unwrap();
        for raw in [false, true] {
            let profile = options.kcp(raw);
            assert!(!profile.nodelay.nodelay);
            assert_eq!(profile.nodelay.interval, 50);
            assert_eq!(profile.nodelay.resend, 0);
            assert!(!profile.nodelay.nc);
            assert!(!profile.flush_write);
            assert!(!profile.flush_acks_input);
        }
        assert_eq!(options.kcp(true).mtu, 892);
        assert_eq!(options.kcp(false).mtu, 1342);
        options.peer_mtu = Some(9000);
        assert_eq!(options.kcp(true).mtu, 1242);
        options.peer_mtu = Some(512);
        assert_eq!(options.kcp(true).mtu, 504);
        options.interval_ms = Some(9);
        assert!(options.validate().is_err());
        options.interval_ms = Some(5001);
        assert!(options.validate().is_err());
        options.interval_ms = Some(10);
        options.resend = Some(256);
        assert!(options.validate().is_err());
        options.resend = Some(255);
        options.tuner.mode = QuantumTunerMode::Auto;
        options.tuner.max_window_bytes = 256 * 1024;
        assert!(options.validate().is_err());
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
