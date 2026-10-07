//! Linux point-to-point layer-3 forwarding through authenticated tunnel carriers.
//! The nonpersistent interface belongs to its file descriptor and disappears on drop.
use crate::{
    config::{Config, Mode, TunConfig, decode_key},
    kcp_carrier::Acceptor,
    transport::{self, SecureConnection},
    wire::{DATA_SIZE, Frame},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    time::Duration,
};
use tokio::{io::unix::AsyncFd, process::Command, sync::mpsc, time};

pub struct TunDevice {
    fd: AsyncFd<File>,
    mtu: usize,
}

impl TunDevice {
    /// Creates a fresh interface. Never attaches to, or removes, an existing device.
    pub async fn create(config: &TunConfig) -> Result<Self> {
        ensure!(
            !config.name.is_empty()
                && config.name.len() < libc::IFNAMSIZ
                && config
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "invalid TUN interface name"
        );
        ensure!(
            usize::from(config.mtu) <= DATA_SIZE,
            "MTU exceeds frame size"
        );
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/net/tun")
            .context("open /dev/net/tun (Linux TUN support and CAP_NET_ADMIN are required)")?;
        {
            // SAFETY: ifreq is a C POD struct; zeroed fields are valid. The ioctl
            // receives its exact ABI layout and a live pointer for this call only.
            // Keep the pointer-containing union out of the future's suspended state.
            let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
            for (slot, byte) in request.ifr_name.iter_mut().zip(config.name.bytes()) {
                *slot = byte as libc::c_char;
            }
            request.ifr_ifru.ifru_flags =
                (libc::IFF_TUN | libc::IFF_NO_PI | libc::IFF_TUN_EXCL) as _;
            let result = unsafe { libc::ioctl(file.as_raw_fd(), libc::TUNSETIFF, &mut request) };
            if result < 0 {
                return Err(io::Error::last_os_error())
                    .context("create exclusive nonpersistent TUN interface");
            }
        }
        let device = Self {
            fd: AsyncFd::new(file)?,
            mtu: usize::from(config.mtu),
        };
        let local = config.address.to_string();
        let peer = config.peer_address.to_string();
        let mtu = config.mtu.to_string();
        if local.parse::<std::net::IpAddr>()?.is_ipv6() {
            // These explicit point-to-point /128 addresses need no neighbour discovery.
            // Disable DAD only on this owned address so the inner TCP bind is ready immediately.
            ip(&[
                "-6",
                "address",
                "add",
                &local,
                "peer",
                &peer,
                "dev",
                &config.name,
                "nodad",
            ])
            .await?;
        } else {
            ip(&["address", "add", &local, "peer", &peer, "dev", &config.name]).await?;
        }
        ip(&["link", "set", "dev", &config.name, "mtu", &mtu, "up"]).await?;
        // Linux installs the IPv4 peer route automatically, but the IPv6 address
        // operation installs a local-address route instead. Add only the explicit
        // peer host route; never replace an existing route or change defaults.
        if local.parse::<std::net::IpAddr>()?.is_ipv6() {
            let destination = format!("{peer}/128");
            ip(&["-6", "route", "add", &destination, "dev", &config.name]).await?;
        }
        tracing::info!(interface = %config.name, %local, %peer, mtu = config.mtu, "TUN interface ready");
        Ok(device)
    }

    pub async fn receive(&self, buffer: &mut [u8]) -> Result<usize> {
        loop {
            let mut ready = self.fd.readable().await?;
            match ready.try_io(|fd| {
                // SAFETY: buffer points to its writable length; fd stays owned by self.
                let count = unsafe {
                    libc::read(
                        fd.get_ref().as_raw_fd(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                    )
                };
                if count < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(count as usize)
                }
            }) {
                Ok(Ok(count)) => return Ok(count),
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => continue,
            }
        }
    }

    pub async fn inject(&self, packet: &[u8]) -> Result<()> {
        validate_packet(packet, self.mtu)?;
        loop {
            let mut ready = self.fd.writable().await?;
            match ready.try_io(|fd| {
                // SAFETY: packet points to readable bytes and fd remains live for write.
                let count = unsafe {
                    libc::write(
                        fd.get_ref().as_raw_fd(),
                        packet.as_ptr().cast(),
                        packet.len(),
                    )
                };
                if count < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(count as usize)
                }
            }) {
                Ok(Ok(count)) => {
                    ensure!(count == packet.len(), "partial TUN packet write");
                    return Ok(());
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => continue,
            }
        }
    }
}

async fn ip(arguments: &[&str]) -> Result<()> {
    let result = Command::new("ip")
        .args(arguments)
        .kill_on_drop(true)
        .output()
        .await
        .context("run iproute2 (the ip command is required)")?;
    ensure!(
        result.status.success(),
        "iproute2 failed: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(())
}

/// Rejects truncated, oversized, or non-IP datagrams before kernel injection.
pub fn validate_packet(packet: &[u8], mtu: usize) -> Result<()> {
    ensure!(
        !packet.is_empty() && packet.len() <= mtu && packet.len() <= DATA_SIZE,
        "IP packet outside MTU"
    );
    match packet[0] >> 4 {
        4 => {
            ensure!(packet.len() >= 20, "truncated IPv4 header");
            let header = usize::from(packet[0] & 15) * 4;
            ensure!(
                header >= 20 && header <= packet.len(),
                "invalid IPv4 header length"
            );
            ensure!(
                usize::from(u16::from_be_bytes([packet[2], packet[3]])) == packet.len(),
                "IPv4 packet length mismatch"
            );
        }
        6 => {
            ensure!(packet.len() >= 40, "truncated IPv6 header");
            ensure!(
                usize::from(u16::from_be_bytes([packet[4], packet[5]])) + 40 == packet.len(),
                "IPv6 packet length mismatch or unsupported jumbogram"
            );
        }
        _ => bail!("TUN accepts IPv4 or IPv6 packets only"),
    }
    Ok(())
}

pub async fn run(config: Config) -> Result<()> {
    config.validate()?;
    transport::validate_tls_config(&config)?;
    let tun = config.tun.as_ref().context("TUN configuration missing")?;
    let device = TunDevice::create(tun).await?;
    if let Some(inner) = config.tun_forwarding_config()? {
        tracing::info!(port = ?tun.forwarding_port, "authenticated forwarding over inner TUN TCP enabled");
        tokio::try_join!(
            run_packets(&device, &config),
            crate::engine::run_forwarding(inner)
        )?;
        return Ok(());
    }
    run_packets(&device, &config).await
}

async fn run_packets(device: &TunDevice, config: &Config) -> Result<()> {
    let carrier = match config.mode {
        Mode::Server => config.listeners[0].transport,
        Mode::Client => config.paths[0].transport,
    };
    if carrier == crate::config::Transport::Tun {
        return crate::raw_tun::run(device, config).await;
    }
    let private = config.private_key()?;
    match config.mode {
        Mode::Server => {
            let listener = &config.listeners[0];
            let peers = config
                .peer_public_keys
                .iter()
                .map(|key| decode_key(key))
                .collect::<Result<Vec<_>>>()?;
            let mut acceptor = if config.reverse {
                None
            } else {
                Some(Acceptor::bind(listener).await?)
            };
            loop {
                let connection = if let Some(acceptor) = &mut acceptor {
                    let incoming = acceptor.accept().await?;
                    transport::accept_listener(
                        incoming,
                        listener,
                        &private,
                        &peers,
                        Duration::from_secs(10),
                    )
                    .await
                } else {
                    transport::connect_responder(listener, &private, &peers).await
                };
                match connection {
                    Ok(connection) => {
                        if let Err(error) = pump(device, connection, config).await {
                            tracing::debug!(%error, "TUN session ended");
                        }
                    }
                    Err(error) => tracing::debug!(%error, "TUN peer rejected"),
                }
                if config.reverse {
                    time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
        Mode::Client => {
            let path = &config.paths[0];
            let mut acceptor = if config.reverse {
                Some(Acceptor::bind(&path.listen_config()).await?)
            } else {
                None
            };
            let base = path.retry_interval.clamp(1, 30);
            let mut retry = base;
            loop {
                let started = time::Instant::now();
                let connection = if let Some(acceptor) = &mut acceptor {
                    transport::accept_initiator(acceptor.accept().await?, path, &private).await
                } else {
                    transport::connect_path(path, &private).await
                };
                match connection {
                    Ok(connection) => {
                        if let Err(error) = pump(device, connection, config).await {
                            tracing::debug!(%error, "TUN session ended");
                        }
                    }
                    Err(error) => tracing::debug!(%error, "TUN connection failed"),
                }
                if started.elapsed() > Duration::from_secs(config.dead_timeout_sec) {
                    retry = base;
                }
                time::sleep(Duration::from_secs(retry)).await;
                retry = (retry * 2).min(30);
            }
        }
    }
}

async fn pump(device: &TunDevice, connection: SecureConnection, config: &Config) -> Result<()> {
    let (mut reader, mut writer) = connection.split();
    let (tx, mut rx) = mpsc::channel::<Frame>(32);
    let deadline = Duration::from_secs(config.dead_timeout_sec);
    let inbound = async {
        loop {
            // Timeout always ends the session: a partially received encrypted frame
            // must never be cancelled and retried on the same cipher stream.
            let frame = time::timeout(deadline, reader.recv())
                .await
                .context("TUN peer timed out")??;
            match frame {
                Frame::Data { id: 1, data } => time::timeout(deadline, device.inject(&data))
                    .await
                    .context("TUN device write timed out")??,
                Frame::Ping { nonce } => tx
                    .send(Frame::Pong { nonce })
                    .await
                    .context("TUN writer stopped")?,
                Frame::Pong { .. } => {}
                _ => bail!("unexpected frame in TUN session"),
            }
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    };
    let outbound = async {
        let mut heartbeat = time::interval(Duration::from_secs(config.heartbeat_sec));
        heartbeat.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut nonce = 0u64;
        loop {
            let frame = tokio::select! {
                frame = rx.recv() => frame.context("TUN producers stopped")?,
                _ = heartbeat.tick() => { nonce = nonce.wrapping_add(1); Frame::Ping { nonce } }
            };
            time::timeout(deadline, writer.send(&frame))
                .await
                .context("TUN send timed out")??;
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    };
    let packets = async {
        let mut buffer = vec![0; device.mtu + 1];
        loop {
            let count = device.receive(&mut buffer).await?;
            validate_packet(&buffer[..count], device.mtu)?;
            tx.send(Frame::Data {
                id: 1,
                data: buffer[..count].to_vec(),
            })
            .await
            .context("TUN writer stopped")?;
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    };
    tokio::try_join!(inbound, outbound, packets)?;
    Ok(())
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
