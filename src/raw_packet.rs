//! Recovered DaggerConnect IPv4 outer packet layouts. Payload protection belongs
//! to the caller; these routines never consult a vendor or use a shared secret.
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const DCPI_MARKER: [u8; 4] = [0xda, 0x66, 0xe7, 0x01];
pub const ETHERNET_HEADER: usize = 14;
pub const IPV4_HEADER: usize = 20;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RawProfile {
    #[default]
    Tcp,
    Udp,
    Icmp,
    Gre,
    Ipip,
    Bip,
    Raw,
}

impl RawProfile {
    pub fn protocol(self) -> u8 {
        match self {
            Self::Tcp => 6,
            Self::Udp => 17,
            Self::Icmp | Self::Bip => 1,
            Self::Gre => 47,
            Self::Ipip => 4,
            Self::Raw => 253,
        }
    }
    pub fn header_len(self) -> usize {
        match self {
            Self::Tcp => 20,
            Self::Udp | Self::Icmp | Self::Bip => 8,
            Self::Gre => 4,
            Self::Ipip | Self::Raw => 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RawOptions {
    pub profile: RawProfile,
    pub interface: Option<String>,
    pub local_ip: Option<Ipv4Addr>,
    pub peer_ip: Ipv4Addr,
    pub l4_port: u16,
    pub source_port: Option<u16>,
    pub dcpi_mode: bool,
    /// Original configuration alias for the IPv4 DCPI carrier.
    pub proto58: bool,
    /// Legacy sentinels: the supplied core rejects these in enabled proto58 mode.
    pub proto58_src_ipv6: String,
    pub proto58_dst_ipv6: String,
    pub spoof_src_ip: Option<Ipv4Addr>,
    pub spoof_dst_ip: Option<Ipv4Addr>,
    pub peer_mac: Option<String>,
    pub gateway_ip: Option<Ipv4Addr>,
    pub sock_buf: usize,
    pub tcp_flags: String,
}

impl Default for RawOptions {
    fn default() -> Self {
        Self {
            profile: RawProfile::Tcp,
            interface: None,
            local_ip: None,
            peer_ip: Ipv4Addr::UNSPECIFIED,
            l4_port: 443,
            source_port: None,
            dcpi_mode: false,
            proto58: false,
            proto58_src_ipv6: String::new(),
            proto58_dst_ipv6: String::new(),
            spoof_src_ip: None,
            spoof_dst_ip: None,
            peer_mac: None,
            gateway_ip: None,
            sock_buf: 4_194_304,
            tcp_flags: "pa".into(),
        }
    }
}

impl RawOptions {
    pub fn dcpi_enabled(&self) -> bool {
        self.dcpi_mode || self.proto58
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.proto58_src_ipv6.is_empty() && self.proto58_dst_ipv6.is_empty(),
            "proto58_src_ipv6/proto58_dst_ipv6 are unsupported: recovered proto58/DCPI uses IPv4 outer packets"
        );
        ensure!(
            !self.peer_ip.is_unspecified()
                && !self.peer_ip.is_multicast()
                && !self.peer_ip.is_broadcast(),
            "raw peer_ip requires a unicast IPv4 address"
        );
        ensure!(
            self.l4_port != 0 && self.source_port != Some(0),
            "raw ports cannot be zero"
        );
        ensure!(
            (262_144..=67_108_864).contains(&self.sock_buf),
            "raw sock_buf must be 262144..67108864"
        );
        for ip in [
            self.local_ip,
            self.spoof_src_ip,
            self.spoof_dst_ip,
            self.gateway_ip,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast(),
                "raw IP fields require unicast IPv4 addresses"
            );
        }
        if let Some(interface) = &self.interface {
            ensure!(
                !interface.is_empty()
                    && interface.len() < 16
                    && interface
                        .bytes()
                        .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'_' | b'-' | b'.')),
                "invalid raw interface name"
            );
        }
        if let Some(mac) = &self.peer_mac {
            parse_mac(mac)?;
        }
        ensure!(
            !self.tcp_flags.is_empty()
                && self.tcp_flags.len() <= 6
                && self.tcp_flags.bytes().all(|v| b"fsrpau".contains(&v)),
            "raw tcp_flags require f/s/r/p/a/u"
        );
        // DCPI uses the real configured IPv4 endpoints in the original core.
        ensure!(
            !self.dcpi_enabled() || (self.spoof_src_ip.is_none() && self.spoof_dst_ip.is_none()),
            "DCPI and IP spoofing cannot be combined"
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PacketOptions {
    pub source_mac: [u8; 6],
    pub destination_mac: [u8; 6],
    pub source_ip: Ipv4Addr,
    pub destination_ip: Ipv4Addr,
    pub source_port: u16,
    pub destination_port: u16,
    pub profile: RawProfile,
    pub dcpi: bool,
    pub server: bool,
    pub tcp_camouflage: bool,
    pub tcp_flags: u8,
    pub timestamp_seed: u32,
}

impl PacketOptions {
    pub fn header_len(&self) -> usize {
        if self.dcpi {
            4
        } else if self.tcp_camouflage && self.profile == RawProfile::Tcp {
            if self.tcp_flags & 2 != 0 { 40 } else { 32 }
        } else {
            self.profile.header_len()
        }
    }
}

/// Ethernet II, a 20-byte IPv4 header, the selected outer header, then payload.
/// TCP is deliberately a packet disguise, without a TCP handshake or TCP state.
pub fn encode(options: &PacketOptions, payload: &[u8], sequence: u32) -> Result<Vec<u8>> {
    let extra = options.header_len();
    let ip_length = IPV4_HEADER + extra + payload.len();
    ensure!(
        ip_length <= u16::MAX as usize,
        "raw IPv4 payload exceeds packet size"
    );
    let mut frame = vec![0; ETHERNET_HEADER + ip_length];
    frame[..6].copy_from_slice(&options.destination_mac);
    frame[6..12].copy_from_slice(&options.source_mac);
    frame[12..14].copy_from_slice(&[8, 0]);
    let ip = &mut frame[14..34];
    ip[0] = 0x45;
    if options.tcp_camouflage && options.profile == RawProfile::Tcp && !options.dcpi {
        ip[1] = 0xb8; // Recovered Quantum IPv4 template: DSCP 46, ECN zero.
        ip[6] = 0x40; // Quantum's raw TCP template also forbids fragmentation.
    }
    ip[2..4].copy_from_slice(&(ip_length as u16).to_be_bytes());
    ip[8] = 64;
    ip[9] = if options.dcpi {
        58
    } else {
        options.profile.protocol()
    };
    if options.dcpi {
        ip[4..6].copy_from_slice(&(sequence as u16).to_be_bytes());
        ip[6] = 0x40; // Original DCPI forbids fragmentation.
    }
    ip[12..16].copy_from_slice(&options.source_ip.octets());
    ip[16..20].copy_from_slice(&options.destination_ip.octets());
    let sum = checksum(ip);
    ip[10..12].copy_from_slice(&sum.to_be_bytes());
    let body = &mut frame[34..];
    if options.dcpi {
        body[..4].copy_from_slice(&DCPI_MARKER);
    } else {
        match options.profile {
            RawProfile::Tcp | RawProfile::Udp => {
                body[..2].copy_from_slice(&options.source_port.to_be_bytes());
                body[2..4].copy_from_slice(&options.destination_port.to_be_bytes());
                if options.profile == RawProfile::Tcp {
                    body[4..8].copy_from_slice(&sequence.to_be_bytes());
                    body[12..16].copy_from_slice(&[0x50, 0x18, 0xff, 0xff]);
                    if options.tcp_camouflage {
                        let syn = options.tcp_flags & 2 != 0;
                        let tsval = (sequence >> 3).wrapping_add(options.timestamp_seed);
                        let tsecr = tsval.wrapping_sub(sequence % 200).wrapping_sub(50);
                        let seq = if syn {
                            (sequence & 7) + 1
                        } else {
                            sequence
                                .wrapping_mul(128)
                                .wrapping_add(options.timestamp_seed)
                        };
                        let ack = if syn {
                            if options.tcp_flags & 16 != 0 {
                                (sequence & 7) + 2
                            } else {
                                0
                            }
                        } else {
                            seq.wrapping_sub(sequence & 1023).wrapping_add(1400)
                        };
                        body[4..8].copy_from_slice(&seq.to_be_bytes());
                        body[8..12].copy_from_slice(&ack.to_be_bytes());
                        body[12] = (extra as u8 / 4) << 4;
                        body[13] = options.tcp_flags;
                        let offset = if syn {
                            body[20..28].copy_from_slice(&[2, 4, 5, 180, 4, 2, 8, 10]);
                            body[36..40].copy_from_slice(&[1, 3, 3, 8]);
                            28
                        } else {
                            body[20..24].copy_from_slice(&[1, 1, 8, 10]);
                            24
                        };
                        body[offset..offset + 4].copy_from_slice(&tsval.to_be_bytes());
                        body[offset + 4..offset + 8].copy_from_slice(&tsecr.to_be_bytes());
                    }
                } else {
                    let udp_length = body.len() as u16;
                    body[4..6].copy_from_slice(&udp_length.to_be_bytes());
                    // An IPv4 UDP checksum of zero matches the recovered encoder.
                }
            }
            RawProfile::Icmp | RawProfile::Bip => {
                body[0] = if options.server { 0 } else { 8 };
                body[4..6].copy_from_slice(&options.source_port.to_be_bytes());
                body[6..8].copy_from_slice(&(sequence as u16).to_be_bytes());
            }
            RawProfile::Gre => body[..4].copy_from_slice(&[0, 0, 8, 0]),
            RawProfile::Ipip | RawProfile::Raw => {}
        }
    }
    body[extra..].copy_from_slice(payload);
    if !options.dcpi {
        if options.profile == RawProfile::Tcp {
            let sum = transport_checksum(options.source_ip, options.destination_ip, 6, body);
            body[16..18].copy_from_slice(&sum.to_be_bytes());
        } else if matches!(options.profile, RawProfile::Icmp | RawProfile::Bip) {
            let sum = checksum(body);
            body[2..4].copy_from_slice(&sum.to_be_bytes());
        }
    }
    Ok(frame)
}

/// Validates lengths and checksums before returning a borrowed datagram payload.
/// Ethernet padding is excluded using IPv4's total length.
pub fn decode<'a>(options: &PacketOptions, frame: &'a [u8]) -> Result<&'a [u8]> {
    ensure!(
        frame.len() >= 34 && frame[12..14] == [8, 0],
        "not Ethernet IPv4"
    );
    let ip = &frame[14..];
    let ihl = usize::from(ip[0] & 15) * 4;
    ensure!(
        ip[0] >> 4 == 4 && ihl >= 20 && ihl <= ip.len(),
        "invalid IPv4 header"
    );
    let length = usize::from(u16::from_be_bytes([ip[2], ip[3]]));
    ensure!(length >= ihl && length <= ip.len(), "truncated IPv4 packet");
    ensure!(checksum(&ip[..ihl]) == 0, "IPv4 checksum mismatch");
    let fragment = u16::from_be_bytes([ip[6], ip[7]]);
    ensure!(fragment & 0x3fff == 0, "fragmented raw IPv4 packet");
    ensure!(
        ip[9]
            == if options.dcpi {
                58
            } else {
                options.profile.protocol()
            },
        "wrong outer protocol"
    );
    let body = &ip[ihl..length];
    if options.dcpi {
        ensure!(
            body.len() >= 4 && body[..4] == DCPI_MARKER,
            "invalid DCPI marker"
        );
        return Ok(&body[4..]);
    }
    let header = match options.profile {
        RawProfile::Tcp => {
            ensure!(body.len() >= 20, "truncated raw TCP header");
            let offset = usize::from(body[12] >> 4) * 4;
            ensure!(
                offset >= 20 && offset <= body.len(),
                "invalid TCP data offset"
            );
            ensure!(
                u16::from_be_bytes([body[2], body[3]]) == options.source_port,
                "wrong TCP destination port"
            );
            ensure!(
                u16::from_be_bytes([body[0], body[1]]) == options.destination_port,
                "wrong TCP source port"
            );
            let src = Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]);
            let dst = Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]);
            ensure!(
                transport_checksum(src, dst, 6, body) == 0,
                "TCP checksum mismatch"
            );
            offset
        }
        RawProfile::Udp => {
            ensure!(
                body.len() >= 8
                    && usize::from(u16::from_be_bytes([body[4], body[5]])) == body.len(),
                "invalid UDP length"
            );
            ensure!(
                u16::from_be_bytes([body[2], body[3]]) == options.source_port,
                "wrong UDP destination port"
            );
            ensure!(
                u16::from_be_bytes([body[0], body[1]]) == options.destination_port,
                "wrong UDP source port"
            );
            if body[6..8] != [0, 0] {
                let src = Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]);
                let dst = Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]);
                ensure!(
                    transport_checksum(src, dst, 17, body) == 0,
                    "UDP checksum mismatch"
                );
            }
            8
        }
        RawProfile::Icmp | RawProfile::Bip => {
            ensure!(
                body.len() >= 8 && body[1] == 0 && body[0] == if options.server { 8 } else { 0 },
                "invalid ICMP echo disguise"
            );
            ensure!(
                u16::from_be_bytes([body[4], body[5]]) == options.destination_port,
                "unexpected ICMP peer identifier"
            );
            ensure!(checksum(body) == 0, "ICMP checksum mismatch");
            8
        }
        RawProfile::Gre => {
            ensure!(
                body.len() >= 4 && body[..4] == [0, 0, 8, 0],
                "unsupported GRE header"
            );
            4
        }
        RawProfile::Ipip | RawProfile::Raw => 0,
    };
    Ok(&body[header..])
}

pub fn parse_mac(value: &str) -> Result<[u8; 6]> {
    let parts: Vec<_> = value.split(':').collect();
    ensure!(parts.len() == 6, "MAC address requires six octets");
    let mut bytes = [0; 6];
    for (slot, value) in bytes.iter_mut().zip(parts) {
        ensure!(
            value.len() == 2,
            "MAC address octets require two hex digits"
        );
        *slot = u8::from_str_radix(value, 16)?;
    }
    if bytes == [0; 6] || bytes[0] & 1 != 0 {
        bail!("peer MAC must be a nonzero unicast address");
    }
    Ok(bytes)
}

pub fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in data.chunks(2) {
        sum += u32::from(pair[0]) << 8;
        if pair.len() == 2 {
            sum += u32::from(pair[1]);
        }
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn transport_checksum(source: Ipv4Addr, destination: Ipv4Addr, protocol: u8, data: &[u8]) -> u16 {
    let mut pseudo = Vec::with_capacity(data.len() + 12);
    pseudo.extend_from_slice(&source.octets());
    pseudo.extend_from_slice(&destination.octets());
    pseudo.extend_from_slice(&[0, protocol]);
    pseudo.extend_from_slice(&(data.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(data);
    checksum(&pseudo)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(profile: RawProfile, server: bool) -> PacketOptions {
        PacketOptions {
            source_mac: [2, 0, 0, 0, 0, 1],
            destination_mac: [2, 0, 0, 0, 0, 2],
            source_ip: Ipv4Addr::new(192, 0, 2, 1),
            destination_ip: Ipv4Addr::new(192, 0, 2, 2),
            source_port: 443,
            destination_port: 444,
            profile,
            dcpi: false,
            server,
            tcp_camouflage: false,
            tcp_flags: 24,
            timestamp_seed: 0,
        }
    }
    fn peer(options: &PacketOptions) -> PacketOptions {
        let mut p = options.clone();
        p.source_port = options.destination_port;
        p.destination_port = options.source_port;
        p.server = !options.server;
        p
    }
    #[test]
    fn recovered_outer_layout_vectors() {
        let mut o = options(RawProfile::Gre, false);
        let f = encode(&o, &[0x45, 0, 0, 20], 0).unwrap();
        assert_eq!(
            hex::encode(&f),
            "02000000000202000000000108004500001c00000000402ff6afc0000201c00002020000080045000014"
        );
        o.dcpi = true;
        let f = encode(&o, &[0x45, 0, 0, 20], 0x1234).unwrap();
        assert_eq!(
            hex::encode(&f),
            "02000000000202000000000108004500001c12344000403aa470c0000201c0000202da66e70145000014"
        );
    }
    #[test]
    fn profiles_decode_both_directions_and_padding() {
        for profile in [
            RawProfile::Tcp,
            RawProfile::Udp,
            RawProfile::Icmp,
            RawProfile::Gre,
            RawProfile::Ipip,
            RawProfile::Bip,
            RawProfile::Raw,
        ] {
            for server in [true, false] {
                for dcpi in [true, false] {
                    let mut o = options(profile, server);
                    o.dcpi = dcpi;
                    for payload in [
                        vec![],
                        vec![1],
                        (0..1400).map(|v| (v % 251) as u8).collect(),
                    ] {
                        let mut f = encode(&o, &payload, 0x10203040).unwrap();
                        f.extend_from_slice(&[0; 30]);
                        assert_eq!(decode(&peer(&o), &f).unwrap(), payload);
                    }
                }
            }
        }
    }
    #[test]
    fn malformed_outer_packets_rejected() {
        for profile in [
            RawProfile::Tcp,
            RawProfile::Udp,
            RawProfile::Icmp,
            RawProfile::Gre,
            RawProfile::Ipip,
            RawProfile::Bip,
            RawProfile::Raw,
        ] {
            let o = options(profile, false);
            let f = encode(&o, &[1, 2, 3], 7).unwrap();
            for length in 0..f.len() {
                assert!(decode(&peer(&o), &f[..length]).is_err());
            }
            let mut corrupt = f.clone();
            corrupt[30] ^= 1;
            assert!(decode(&peer(&o), &corrupt).is_err());
            let mut fragment = f.clone();
            fragment[20] = 0x20;
            fragment[24] = 0;
            fragment[25] = 0;
            let sum = checksum(&fragment[14..34]);
            fragment[24..26].copy_from_slice(&sum.to_be_bytes());
            assert!(decode(&peer(&o), &fragment).is_err());
        }
        let mut o = options(RawProfile::Tcp, false);
        o.dcpi = true;
        let mut f = encode(&o, &[1, 2, 3], 7).unwrap();
        f[34] ^= 1;
        assert!(decode(&peer(&o), &f).is_err());
    }
    #[test]
    fn quantum_tcp_camouflage_options() {
        let mut o = options(RawProfile::Tcp, false);
        o.tcp_camouflage = true;
        o.timestamp_seed = 10;
        let f = encode(&o, &[1, 2, 3], 17).unwrap();
        assert_eq!(f[46], 0x80);
        assert_eq!(&f[54..58], &[1, 1, 8, 10]);
        assert_eq!(&f[38..42], &2186u32.to_be_bytes());
        assert_eq!(decode(&peer(&o), &f).unwrap(), [1, 2, 3]);
        o.tcp_flags = 2;
        let f = encode(&o, &[1, 2, 3], 17).unwrap();
        assert_eq!(f[46], 0xa0);
        assert_eq!(&f[54..62], &[2, 4, 5, 180, 4, 2, 8, 10]);
        assert_eq!(&f[70..74], &[1, 3, 3, 8]);
        assert_eq!(decode(&peer(&o), &f).unwrap(), [1, 2, 3]);
    }
    #[test]
    fn recovered_quantum_and_dcpi_ipv4_templates_are_distinct() {
        let mut o = options(RawProfile::Tcp, false);
        let ordinary = encode(&o, &[1, 2, 3], 17).unwrap();
        assert_eq!(&ordinary[14..16], &[0x45, 0]);
        assert_eq!(&ordinary[18..22], &[0, 0, 0, 0]);
        assert_eq!(ordinary[23], 6);
        o.tcp_camouflage = true;
        let quantum = encode(&o, &[1, 2, 3], 17).unwrap();
        assert_eq!(&quantum[14..16], &[0x45, 0xb8]);
        assert_eq!(&quantum[18..22], &[0, 0, 0x40, 0]);
        assert_eq!(quantum[23], 6);
        assert_eq!(decode(&peer(&o), &quantum).unwrap(), [1, 2, 3]);
        o.dcpi = true;
        let dcpi = encode(&o, &[1, 2, 3], 17).unwrap();
        assert_eq!(&dcpi[14..16], &[0x45, 0]);
        assert_eq!(&dcpi[18..22], &[0, 17, 0x40, 0]);
        assert_eq!(dcpi[23], 58);
        assert_eq!(&dcpi[34..38], &DCPI_MARKER);
        assert_eq!(decode(&peer(&o), &dcpi).unwrap(), [1, 2, 3]);
    }
    #[test]
    fn proto58_is_dcpi_alias_and_legacy_ipv6_is_rejected() {
        for (dcpi_mode, proto58) in [(false, false), (true, false), (false, true), (true, true)] {
            let raw: RawOptions = serde_json::from_value(
                serde_json::json!({"peer_ip":"192.0.2.2","dcpi_mode":dcpi_mode,"proto58":proto58}),
            )
            .unwrap();
            raw.validate().unwrap();
            assert_eq!(raw.dcpi_enabled(), dcpi_mode || proto58);
            let mut packet = options(RawProfile::Tcp, false);
            packet.dcpi = raw.dcpi_enabled();
            let frame = encode(&packet, &[1, 2, 3], 7).unwrap();
            assert_eq!(frame[23], if dcpi_mode || proto58 { 58 } else { 6 });
            if raw.dcpi_enabled() {
                assert_eq!(&frame[34..38], &DCPI_MARKER);
            }
            assert_eq!(decode(&peer(&packet), &frame).unwrap(), [1, 2, 3]);
        }
        for field in ["proto58_src_ipv6", "proto58_dst_ipv6"] {
            let mut value = serde_json::json!({"peer_ip":"192.0.2.2","proto58":true});
            value[field] = serde_json::json!("2001:db8::1");
            let raw: RawOptions = serde_json::from_value(value).unwrap();
            assert!(
                raw.validate()
                    .unwrap_err()
                    .to_string()
                    .contains("IPv4 outer packets")
            );
        }
        let spoof: RawOptions = serde_json::from_value(
            serde_json::json!({"peer_ip":"192.0.2.2","proto58":true,"spoof_src_ip":"192.0.2.3"}),
        )
        .unwrap();
        assert!(spoof.validate().is_err());
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
