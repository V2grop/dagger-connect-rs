//! Unreliable IP datagrams over the recovered raw Ethernet/IPv4 envelopes.
//! Noise IK replaces vendor/PSK authentication. Independent nonces allow loss
//! and reordering; an authenticated sliding replay window rejects duplicates.
use crate::{
    config::{Config, Mode, decode_key},
    raw_socket::RawSocket,
    transport::NOISE_PATTERN,
    tun_device::{TunDevice, validate_packet},
};
use anyhow::{Context, Result, ensure};
use snow::{Builder, HandshakeState, StatelessTransportState};
use std::time::Duration;
use tokio::time::{self, Instant};

const MAGIC: &[u8; 5] = b"DRTU\x01";
const H1: u8 = 1;
const H2: u8 = 2;
const DATA: u8 = 3;
const IP: u8 = 1;
const PING: u8 = 2;
const PONG: u8 = 3;
const HEADER: usize = 30; // magic(5), type(1), session(16), counter(8)
pub const DATAGRAM_OVERHEAD: usize = HEADER + 16 + 1;
const PROLOGUE: &[u8] = b"dagger-rs/raw-tun/v1";

#[derive(Default)]
struct ReplayWindow {
    highest: Option<u64>,
    bits: [u64; 4],
}
impl ReplayWindow {
    fn seen(&self, nonce: u64) -> bool {
        if let Some(highest) = self.highest {
            if nonce > highest {
                return false;
            }
            let age = highest - nonce;
            age >= 256 || self.bits[age as usize / 64] & (1_u64 << (age % 64)) != 0
        } else {
            false
        }
    }
    fn mark(&mut self, nonce: u64) {
        match self.highest {
            None => {
                self.highest = Some(nonce);
                self.bits = [1, 0, 0, 0];
            }
            Some(highest) if nonce > highest => {
                let shift = nonce - highest;
                if shift >= 256 {
                    self.bits = [0; 4];
                } else {
                    let old = self.bits;
                    let words = shift as usize / 64;
                    let remainder = shift % 64;
                    for index in 0..4 {
                        self.bits[index] = if index >= words {
                            old[index - words] << remainder
                        } else {
                            0
                        };
                        if remainder > 0 && index > words {
                            self.bits[index] |= old[index - words - 1] >> (64 - remainder);
                        }
                    }
                }
                self.bits[0] |= 1;
                self.highest = Some(nonce);
            }
            Some(highest) => {
                let age = highest - nonce;
                if age < 256 {
                    self.bits[age as usize / 64] |= 1_u64 << (age % 64);
                }
            }
        }
    }
}

struct Session {
    cipher: StatelessTransportState,
    id: [u8; 16],
    counter: u64,
    replay: ReplayWindow,
    last_authenticated: Instant,
    last_heartbeat: Instant,
}
impl Session {
    fn finish(handshake: HandshakeState) -> Result<Self> {
        let id = handshake.get_handshake_hash()[..16].try_into()?;
        Ok(Self {
            cipher: handshake.into_stateless_transport_mode()?,
            id,
            counter: 0,
            replay: ReplayWindow::default(),
            last_authenticated: Instant::now(),
            last_heartbeat: Instant::now(),
        })
    }
    fn encode(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            self.counter < u64::MAX,
            "raw TUN send nonce exhausted; reconnect required"
        );
        let mut packet = Vec::with_capacity(HEADER + payload.len() + 16);
        packet.extend_from_slice(MAGIC);
        packet.push(DATA);
        packet.extend_from_slice(&self.id);
        packet.extend(self.counter.to_be_bytes());
        packet.resize(HEADER + payload.len() + 16, 0);
        let size = self
            .cipher
            .write_message(self.counter, payload, &mut packet[HEADER..])?;
        packet.truncate(HEADER + size);
        self.counter += 1;
        Ok(packet)
    }
    fn decode(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>> {
        if packet.len() < HEADER + 17
            || &packet[..5] != MAGIC
            || packet[5] != DATA
            || packet[6..22] != self.id
        {
            return Ok(None);
        }
        let nonce = u64::from_be_bytes(packet[22..30].try_into()?);
        if nonce == u64::MAX || self.replay.seen(nonce) {
            return Ok(None);
        }
        let mut payload = vec![0; packet.len() - HEADER];
        let size = self
            .cipher
            .read_message(nonce, &packet[HEADER..], &mut payload)?;
        // Authentication MUST precede any replay-window advancement.
        self.replay.mark(nonce);
        payload.truncate(size);
        ensure!(
            !payload.is_empty() && matches!(payload[0], IP | PING | PONG),
            "unknown authenticated raw TUN payload"
        );
        ensure!(
            payload[0] == IP || payload.len() == 1,
            "invalid raw TUN heartbeat payload"
        );
        self.last_authenticated = Instant::now();
        Ok(Some(payload))
    }
}

struct Initiating {
    handshake: HandshakeState,
    request: Vec<u8>,
    id: [u8; 16],
    started: Instant,
    last_sent: Instant,
}
impl Initiating {
    fn new(private: &[u8; 32], server: &[u8; 32]) -> Result<Self> {
        let mut handshake = Builder::new(NOISE_PATTERN.parse()?)
            .local_private_key(private)?
            .remote_public_key(server)?
            .prologue(PROLOGUE)?
            .build_initiator()?;
        let mut message = vec![0; 256];
        let size = handshake.write_message(&[], &mut message)?;
        message.truncate(size);
        let id = message[..16].try_into()?;
        let mut request = MAGIC.to_vec();
        request.push(H1);
        request.extend(message);
        Ok(Self {
            handshake,
            request,
            id,
            started: Instant::now(),
            last_sent: Instant::now() - Duration::from_secs(1),
        })
    }
    fn finish(mut self, response: &[u8]) -> Result<Session> {
        ensure!(
            response.len() >= 22 && response[6..22] == self.id,
            "raw TUN handshake response identifier changed"
        );
        let mut payload = [0; 256];
        let size = self.handshake.read_message(&response[22..], &mut payload)?;
        ensure!(
            size == 0 && self.handshake.is_handshake_finished(),
            "unexpected raw TUN handshake payload"
        );
        Session::finish(self.handshake)
    }
}
struct Candidate {
    session: Session,
    created: Instant,
}
struct CachedHandshake {
    request: Vec<u8>,
    response: Vec<u8>,
    created: Instant,
}
fn respond(request: &[u8], private: &[u8; 32], allowed: &[[u8; 32]]) -> Result<(Session, Vec<u8>)> {
    ensure!(
        request.len() >= 38 && request.len() <= 262,
        "invalid raw TUN handshake request size"
    );
    let mut handshake = Builder::new(NOISE_PATTERN.parse()?)
        .local_private_key(private)?
        .prologue(PROLOGUE)?
        .build_responder()?;
    let mut payload = [0; 256];
    let size = handshake.read_message(&request[6..], &mut payload)?;
    ensure!(size == 0, "unexpected raw TUN handshake request payload");
    let public = handshake
        .get_remote_static()
        .context("raw TUN peer did not supply an identity")?;
    ensure!(
        allowed.iter().any(|key| key.as_slice() == public),
        "raw TUN peer identity is not allowed"
    );
    let mut message = [0; 256];
    let size = handshake.write_message(&[], &mut message)?;
    let mut response = MAGIC.to_vec();
    response.push(H2);
    response.extend_from_slice(&request[6..22]);
    response.extend_from_slice(&message[..size]);
    Ok((Session::finish(handshake)?, response))
}

/// Runs one pinned raw peer. Lost IP packets stay lost; no KCP or byte stream
/// sits under this carrier. Heartbeats and fresh handshakes recover peer restarts.
pub async fn run(device: &TunDevice, config: &Config) -> Result<()> {
    let tun = config
        .tun
        .as_ref()
        .context("raw TUN interface settings missing")?;
    let server = config.mode == Mode::Server;
    let options = if server {
        config.listeners[0].raw.as_ref()
    } else {
        config.paths[0].raw.as_ref()
    }
    .context("raw TUN transport settings missing")?;
    let socket = RawSocket::bind(options, server).await?;
    let maximum = socket.max_payload();
    ensure!(
        maximum > DATAGRAM_OVERHEAD && usize::from(tun.mtu) <= maximum - DATAGRAM_OVERHEAD,
        "raw TUN MTU {} exceeds authenticated raw packet limit {}; lower tun.mtu",
        tun.mtu,
        maximum.saturating_sub(DATAGRAM_OVERHEAD)
    );
    let private = config.private_key()?;
    let allowed = config
        .peer_public_keys
        .iter()
        .map(|key| decode_key(key))
        .collect::<Result<Vec<_>>>()?;
    let server_key = if server {
        None
    } else {
        Some(decode_key(&config.paths[0].server_public_key)?)
    };
    let deadline = Duration::from_secs(config.dead_timeout_sec);
    let heartbeat = Duration::from_secs(config.heartbeat_sec);
    let handshake_deadline = if server {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(config.paths[0].dial_timeout)
    };
    let mut active: Option<Session> = None;
    let mut initiating: Option<Initiating> = None;
    let mut candidate: Option<Candidate> = None;
    let mut cached: Option<CachedHandshake> = None;
    let mut network_buffer = vec![0; maximum + 1];
    let mut ip_buffer = vec![0; usize::from(tun.mtu) + 1];
    let mut tick = time::interval(Duration::from_millis(250));
    tracing::info!(peer=%options.peer_ip,"raw TUN datagram carrier ready");
    loop {
        tokio::select! {
            read=socket.recv(&mut network_buffer)=> {
                let size=read.context("receive raw TUN datagram")?;
                let packet=&network_buffer[..size];
                if packet.len()<6 || &packet[..5]!=MAGIC {continue;}
                match packet[5] {
                    H1 if server=> {
                        if let Some(cache)=&cached {
                            if cache.request==packet && cache.created.elapsed()<deadline {
                                socket.send(&cache.response).await?;continue;
                            }
                        }
                        // Keep one candidate and throttle replacement; a replayed
                        // initiator message must not evict an active session.
                        if candidate.as_ref().is_some_and(|c|c.created.elapsed()<Duration::from_secs(3)) {continue;}
                        match respond(packet,&private,&allowed) {
                            Ok((session,response))=> {
                                socket.send(&response).await?;
                                candidate=Some(Candidate {session,created:Instant::now()});
                                cached=Some(CachedHandshake {request:packet.to_vec(),response,created:Instant::now()});
                            }
                            Err(error)=>tracing::debug!(%error,"raw TUN handshake rejected"),
                        }
                    }
                    H2 if !server=> {
                        if packet.len()>278 || packet.len()<22 || initiating.as_ref().is_none_or(|p|packet[6..22]!=p.id) {continue;}
                        if let Some(pending)=initiating.take() {
                            match pending.finish(packet) {
                                Ok(mut session)=> {
                                    socket.send(&session.encode(&[PING])?).await?;
                                    active=Some(session);tracing::info!("raw TUN peer authenticated");
                                }
                                Err(error)=>tracing::debug!(%error,"raw TUN handshake response rejected"),
                            }
                        }
                    }
                    DATA=> {
                        if packet.len()<HEADER+17 {continue;}
                        let is_candidate=candidate.as_ref().is_some_and(|c|packet[6..22]==c.session.id);
                        let decoded=if is_candidate {
                            candidate.as_mut().unwrap().session.decode(packet)
                        } else if let Some(session)=&mut active {session.decode(packet)}else {continue};
                        match decoded {
                            Ok(Some(payload))=> {
                                if is_candidate {active=Some(candidate.take().unwrap().session);tracing::info!("raw TUN peer authenticated");}
                                match payload[0] {
                                    IP=> {
                                        if let Err(error)=validate_packet(&payload[1..],usize::from(tun.mtu)) {
                                            tracing::debug!(%error,"invalid authenticated raw TUN IP packet dropped");
                                        } else {device.inject(&payload[1..]).await?;}
                                    }
                                    PING=> {if let Some(session)=&mut active {socket.send(&session.encode(&[PONG])?).await?;}},
                                    PONG=>{},_=>unreachable!(),
                                }
                            }
                            Ok(None)=>{},Err(error)=>tracing::debug!(%error,"raw TUN datagram authentication rejected"),
                        }
                    }
                    _=>{},
                }
            }
            read=device.receive(&mut ip_buffer), if active.is_some()=> {
                let size=read?;
                if validate_packet(&ip_buffer[..size],usize::from(tun.mtu)).is_err() {continue;}
                let mut payload=Vec::with_capacity(size+1);payload.push(IP);payload.extend_from_slice(&ip_buffer[..size]);
                if let Some(session)=&mut active {socket.send(&session.encode(&payload)?).await?;}
            }
            _=tick.tick()=> {
                if active.as_ref().is_some_and(|s|s.last_authenticated.elapsed()>=deadline) {
                    active=None;tracing::info!("raw TUN peer silent; reconnecting");
                }
                if candidate.as_ref().is_some_and(|c|c.created.elapsed()>=deadline) {
                    candidate=None;
                    // Its response contains the now-discarded responder key.
                    // Never acknowledge a retry using an orphaned response.
                    cached=None;
                }
                if !server && active.is_none() {
                    if initiating.as_ref().is_some_and(|p|p.started.elapsed()>=handshake_deadline) {initiating=None;}
                    if initiating.is_none() {initiating=Some(Initiating::new(&private,server_key.as_ref().unwrap())?);}
                    if let Some(pending)=&mut initiating {
                        if pending.last_sent.elapsed()>=Duration::from_secs(1) {
                            socket.send(&pending.request).await?;pending.last_sent=Instant::now();
                        }
                    }
                }
                if let Some(session)=&mut active {
                    if session.last_heartbeat.elapsed()>=heartbeat {
                        socket.send(&session.encode(&[PING])?).await?;session.last_heartbeat=Instant::now();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sessions() -> Result<(Session, Session)> {
        let client = Builder::new(NOISE_PATTERN.parse()?).generate_keypair()?;
        let server = Builder::new(NOISE_PATTERN.parse()?).generate_keypair()?;
        let pending = Initiating::new(
            &client.private.try_into().unwrap(),
            &server.public.try_into().unwrap(),
        )?;
        let (responder, response) = respond(
            &pending.request,
            &server.private.try_into().unwrap(),
            &[client.public.try_into().unwrap()],
        )?;
        Ok((pending.finish(&response)?, responder))
    }
    #[test]
    fn datagrams_survive_loss_and_reordering_and_reject_replay() -> Result<()> {
        let (mut sender, mut receiver) = sessions()?;
        let mut packets = Vec::new();
        for _ in 0..300 {
            packets.push(sender.encode(&[PING])?);
        }
        assert_eq!(receiver.decode(&packets[40])?, Some(vec![PING]));
        assert_eq!(receiver.decode(&packets[3])?, Some(vec![PING]));
        assert!(receiver.decode(&packets[40])?.is_none());
        assert_eq!(receiver.decode(&packets[299])?, Some(vec![PING]));
        assert!(receiver.decode(&packets[3])?.is_none());
        assert_eq!(receiver.decode(&packets[298])?, Some(vec![PING]));
        assert!(receiver.decode(&packets[298])?.is_none());
        Ok(())
    }
    #[test]
    fn forged_large_nonce_does_not_advance_replay_window() -> Result<()> {
        let (mut sender, mut receiver) = sessions()?;
        let valid = sender.encode(&[PING])?;
        let mut forged = valid.clone();
        forged[22..30].copy_from_slice(&1_000_000_u64.to_be_bytes());
        assert!(receiver.decode(&forged).is_err());
        assert!(receiver.replay.highest.is_none());
        assert_eq!(receiver.decode(&valid)?, Some(vec![PING]));
        let mut wrong_session = sender.encode(&[PONG])?;
        wrong_session[6] ^= 1;
        assert!(receiver.decode(&wrong_session)?.is_none());
        sender.counter = u64::MAX;
        assert!(sender.encode(&[PING]).is_err());
        Ok(())
    }
    #[test]
    fn replay_bitmap_matches_reference_for_word_boundaries() {
        let mut window = ReplayWindow::default();
        let mut accepted = std::collections::HashSet::new();
        let mut highest = 0;
        for nonce in [
            0, 63, 64, 65, 128, 255, 256, 511, 512, 513, 300, 350, 600, 601, 599,
        ] {
            assert_eq!(
                window.seen(nonce),
                accepted.contains(&nonce) || (nonce <= highest && highest - nonce >= 256)
            );
            window.mark(nonce);
            accepted.insert(nonce);
            highest = highest.max(nonce);
            for probe in 0..=highest + 1 {
                assert_eq!(
                    window.seen(probe),
                    accepted.contains(&probe) || (probe <= highest && highest - probe >= 256),
                    "highest={highest},probe={probe}"
                );
            }
        }
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
