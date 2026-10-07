//! Linux Ethernet packet transport for the recovered raw IPv4 profiles.
//! No route, sysctl, firewall or neighbor entry is changed by this module.
pub use crate::raw_packet::RawOptions;
use crate::raw_packet::{self, PacketOptions};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::CString,
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};
use tokio::{io::unix::AsyncFd, process::Command, time};

pub struct RawSocket {
    fd: AsyncFd<OwnedFd>,
    interface_index: i32,
    packet: PacketOptions,
    peer: SocketAddr,
    mtu: usize,
    sequence: AtomicU32,
}

impl RawSocket {
    /// Opens one packet socket on a physical/veth Ethernet interface.
    /// A supplied peer_mac is a next-hop MAC, including when spoof_dst_ip is used.
    pub async fn bind(options: &RawOptions, server: bool) -> Result<Self> {
        Self::bind_inner(options, server, false).await
    }
    pub async fn bind_quantum(options: &RawOptions, server: bool) -> Result<Self> {
        Self::bind_inner(options, server, true).await
    }
    async fn bind_inner(options: &RawOptions, server: bool, quantum: bool) -> Result<Self> {
        options.validate()?;
        let mut route_args = vec![
            "-j".to_string(),
            "route".into(),
            "get".into(),
            options.peer_ip.to_string(),
        ];
        if let Some(interface) = &options.interface {
            route_args.extend(["oif".into(), interface.clone()]);
        }
        let routes = ip_json(&route_args).await?;
        let route = routes
            .as_array()
            .and_then(|v| v.first())
            .context("ip route get returned no route")?;
        let interface = options
            .interface
            .as_deref()
            .or_else(|| route["dev"].as_str())
            .context("raw interface cannot be determined")?;
        let local_ip = options
            .local_ip
            .or_else(|| route["prefsrc"].as_str().and_then(|s| s.parse().ok()))
            .context("raw local_ip cannot be determined")?;
        let next_hop = options
            .gateway_ip
            .or_else(|| route["gateway"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(options.peer_ip);
        let links = ip_json(&[
            "-j".into(),
            "link".into(),
            "show".into(),
            "dev".into(),
            interface.into(),
        ])
        .await?;
        let link = links
            .as_array()
            .and_then(|v| v.first())
            .context("raw interface does not exist")?;
        ensure!(
            link["link_type"].as_str() == Some("ether"),
            "raw profiles require an Ethernet/veth interface"
        );
        let source_mac = raw_packet::parse_mac(
            link["address"]
                .as_str()
                .context("interface MAC unavailable")?,
        )?;
        let mtu = link["mtu"].as_u64().context("interface MTU unavailable")? as usize;
        let name = CString::new(interface)?;
        // SAFETY: CString is terminated and remains live during if_nametoindex.
        let interface_index = unsafe { libc::if_nametoindex(name.as_ptr()) } as i32;
        ensure!(interface_index > 0, "interface index unavailable");
        // SAFETY: socket has no borrowed pointers; successful fd is owned below.
        let fd = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                (libc::ETH_P_ALL as u16).to_be() as i32,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error())
                .context("open raw socket (CAP_NET_RAW is required)");
        }
        // SAFETY: successful socket returns a fresh descriptor owned exclusively.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut address = link_address(interface_index, [0; 6]);
        address.sll_protocol = (libc::ETH_P_ALL as u16).to_be();
        // SAFETY: pointer references a fully initialized sockaddr_ll with its size.
        let result = unsafe {
            libc::bind(
                fd,
                (&address as *const libc::sockaddr_ll).cast(),
                std::mem::size_of_val(&address) as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error()).context("bind raw interface");
        }
        set_socket_buffers(fd, options.sock_buf).context("set raw socket buffers")?;
        let fd = AsyncFd::new(owned)?;
        let destination_mac = if let Some(mac) = &options.peer_mac {
            raw_packet::parse_mac(mac)?
        } else {
            resolve_arp(&fd, interface_index, source_mac, local_ip, next_hop).await?
        };
        let client_port = options.source_port.unwrap_or_else(|| {
            if options.l4_port == u16::MAX {
                options.l4_port - 1
            } else {
                options.l4_port + 1
            }
        });
        let (source_port, destination_port) = if server {
            (options.l4_port, client_port)
        } else {
            (client_port, options.l4_port)
        };
        let tcp_flags = options.tcp_flags.bytes().fold(0u8, |flags, v| {
            flags
                | match v {
                    b'f' => 1,
                    b's' => 2,
                    b'r' => 4,
                    b'p' => 8,
                    b'a' => 16,
                    b'u' => 32,
                    _ => 0,
                }
        });
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let packet = PacketOptions {
            source_mac,
            destination_mac,
            source_ip: options.spoof_src_ip.unwrap_or(local_ip),
            destination_ip: options.spoof_dst_ip.unwrap_or(options.peer_ip),
            source_port,
            destination_port,
            profile: options.profile,
            dcpi: options.dcpi_enabled(),
            server,
            tcp_camouflage: quantum,
            tcp_flags,
            timestamp_seed: seed,
        };
        tracing::info!(%interface,%local_ip,peer=%options.peer_ip,profile=?options.profile,dcpi=options.dcpi_enabled(),"raw packet interface ready");
        Ok(Self {
            fd,
            interface_index,
            packet,
            peer: SocketAddr::V4(SocketAddrV4::new(options.peer_ip, options.l4_port)),
            mtu,
            sequence: AtomicU32::new(1),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(
            self.packet.source_ip,
            self.packet.source_port,
        ))
    }
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }
    pub fn max_payload(&self) -> usize {
        self.mtu.saturating_sub(20 + self.packet.header_len())
    }

    /// Adjusts this socket's buffers without changing host-wide limits.
    pub fn set_socket_buffer_bytes(&self, bytes: usize) -> io::Result<()> {
        set_socket_buffers(self.fd.get_ref().as_raw_fd(), bytes)
    }

    pub async fn send(&self, payload: &[u8]) -> io::Result<usize> {
        if payload.len() > self.max_payload() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "raw payload exceeds interface MTU",
            ));
        }
        let frame = raw_packet::encode(
            &self.packet,
            payload,
            self.sequence.fetch_add(1, Ordering::Relaxed),
        )
        .map_err(io::Error::other)?;
        send_frame(
            &self.fd,
            self.interface_index,
            self.packet.destination_mac,
            &frame,
        )
        .await?;
        Ok(payload.len())
    }

    pub async fn recv(&self, payload: &mut [u8]) -> io::Result<usize> {
        let mut frame = vec![0; 65_549];
        loop {
            let (length, outgoing) = receive_frame(&self.fd, &mut frame).await?;
            if outgoing || length < 14 || frame[..6] != self.packet.source_mac {
                continue;
            }
            if let Ok(data) = raw_packet::decode(&self.packet, &frame[..length]) {
                // Ignore an oversized datagram instead of terminating the carrier.
                if data.len() > payload.len() {
                    continue;
                }
                payload[..data.len()].copy_from_slice(data);
                return Ok(data.len());
            }
        }
    }
    pub async fn recv_from(&self, payload: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        Ok((self.recv(payload).await?, self.peer))
    }
    pub async fn send_to(&self, payload: &[u8], destination: SocketAddr) -> io::Result<usize> {
        if destination.ip() != self.peer.ip() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "raw socket is pinned to its configured peer",
            ));
        }
        self.send(payload).await
    }
}

fn set_socket_buffers(fd: i32, bytes: usize) -> io::Result<()> {
    if !(256 * 1024..=64 * 1024 * 1024).contains(&bytes) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "raw socket buffers must be 256 KiB..64 MiB",
        ));
    }
    let buffer = bytes as i32;
    for option in [libc::SO_RCVBUF, libc::SO_SNDBUF] {
        // SAFETY: value is initialized and remains live for the system call.
        let result = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&buffer as *const i32).cast(),
                std::mem::size_of_val(&buffer) as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn link_address(index: i32, mac: [u8; 6]) -> libc::sockaddr_ll {
    // SAFETY: sockaddr_ll is a C POD structure where all-zero fields are valid.
    let mut address: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
    address.sll_family = libc::AF_PACKET as u16;
    address.sll_protocol = (libc::ETH_P_IP as u16).to_be();
    address.sll_ifindex = index;
    address.sll_halen = 6;
    address.sll_addr[..6].copy_from_slice(&mac);
    address
}

async fn send_frame(
    fd: &AsyncFd<OwnedFd>,
    index: i32,
    mac: [u8; 6],
    frame: &[u8],
) -> io::Result<()> {
    let address = link_address(index, mac);
    loop {
        let mut guard = fd.writable().await?;
        match guard.try_io(|inner| {
            // SAFETY: frame and sockaddr buffers are initialized and live until return.
            let result = unsafe {
                libc::sendto(
                    inner.get_ref().as_raw_fd(),
                    frame.as_ptr().cast(),
                    frame.len(),
                    0,
                    (&address as *const libc::sockaddr_ll).cast(),
                    std::mem::size_of_val(&address) as libc::socklen_t,
                )
            };
            if result < 0 {
                Err(io::Error::last_os_error())
            } else if result as usize != frame.len() {
                Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "short Ethernet write",
                ))
            } else {
                Ok(())
            }
        }) {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}

async fn receive_frame(fd: &AsyncFd<OwnedFd>, frame: &mut [u8]) -> io::Result<(usize, bool)> {
    loop {
        let mut guard = fd.readable().await?;
        match guard.try_io(|inner| {
            // SAFETY: POD sockaddr is initialized and mutable receive buffers are valid.
            let mut address: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
            let mut size = std::mem::size_of_val(&address) as libc::socklen_t;
            let result = unsafe {
                libc::recvfrom(
                    inner.get_ref().as_raw_fd(),
                    frame.as_mut_ptr().cast(),
                    frame.len(),
                    0,
                    (&mut address as *mut libc::sockaddr_ll).cast(),
                    &mut size,
                )
            };
            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok((
                    result as usize,
                    address.sll_pkttype == libc::PACKET_OUTGOING,
                ))
            }
        }) {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}

async fn resolve_arp(
    fd: &AsyncFd<OwnedFd>,
    index: i32,
    mac: [u8; 6],
    local: Ipv4Addr,
    peer: Ipv4Addr,
) -> Result<[u8; 6]> {
    let mut request = vec![0; 42];
    request[..6].fill(0xff);
    request[6..12].copy_from_slice(&mac);
    request[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
    request[22..28].copy_from_slice(&mac);
    request[28..32].copy_from_slice(&local.octets());
    request[38..42].copy_from_slice(&peer.octets());
    for _ in 0..3 {
        send_frame(fd, index, [0xff; 6], &request).await?;
        let attempt = async {
            let mut frame = vec![0; 2048];
            loop {
                let (n, outgoing) = receive_frame(fd, &mut frame).await?;
                if !outgoing
                    && n >= 42
                    && frame[12..22] == [8, 6, 0, 1, 8, 0, 6, 4, 0, 2]
                    && frame[28..32] == peer.octets()
                    && frame[38..42] == local.octets()
                    && frame[32..38] == mac
                {
                    let mut found = [0; 6];
                    found.copy_from_slice(&frame[22..28]);
                    if found[0] & 1 == 0 && found != [0; 6] {
                        return Ok::<_, io::Error>(found);
                    }
                }
            }
        };
        if let Ok(result) = time::timeout(Duration::from_secs(1), attempt).await {
            return Ok(result?);
        }
    }
    anyhow::bail!("ARP next-hop resolution failed; configure peer_mac explicitly")
}

async fn ip_json(arguments: &[String]) -> Result<serde_json::Value> {
    let output = Command::new("ip")
        .args(arguments)
        .output()
        .await
        .context("execute iproute2 for interface lookup")?;
    ensure!(
        output.status.success(),
        "iproute2 interface lookup failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    serde_json::from_slice(&output.stdout).context("parse iproute2 interface information")
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
