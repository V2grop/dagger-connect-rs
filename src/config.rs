use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Server,
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MapKind {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Tcp,
    Kcp,
    Http,
    Https,
    Ws,
    Wss,
    Xhttp,
    Xhttps,
    Dc6,
    #[serde(rename = "quantum+", alias = "qplus")]
    QuantumPlus,
    Quantum,
    #[serde(rename = "quantum-gaming")]
    QuantumGaming,
    Tun,
}

impl Transport {
    pub fn uses_tls(self) -> bool {
        matches!(self, Self::Https | Self::Wss | Self::Xhttps)
    }

    pub fn uses_http(self) -> bool {
        matches!(
            self,
            Self::Http | Self::Https | Self::Ws | Self::Wss | Self::Xhttp | Self::Xhttps
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum XhttpMode {
    Auto,
    #[default]
    StreamUp,
    PacketUp,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct XhttpOptions {
    pub mode: XhttpMode,
    pub host: Option<String>,
    pub edge_addrs: Vec<String>,
    pub user_agent: String,
    pub pad_min: usize,
    pub pad_max: usize,
    pub socket_buf_bytes: usize,
    pub up_max_bytes: usize,
    pub up_concurrency: usize,
    pub buffer_bytes: usize,
    pub probe_ms: u64,
    pub session_timeout_sec: u64,
}

impl Default for XhttpOptions {
    fn default() -> Self {
        Self {
            mode: XhttpMode::StreamUp,
            host: None,
            edge_addrs: Vec::new(),
            user_agent: "dagger-rs".into(),
            pad_min: 0,
            pad_max: 0,
            socket_buf_bytes: 1_048_576,
            up_max_bytes: 65_536,
            up_concurrency: 2,
            buffer_bytes: 32_768,
            probe_ms: 8000,
            session_timeout_sec: 40,
        }
    }
}

impl XhttpOptions {
    fn validate(&self) -> Result<()> {
        ensure!(
            (1024..=1_048_576).contains(&self.up_max_bytes),
            "XHTTP up_max_bytes must be 1024..1048576"
        );
        ensure!(
            (1..=16).contains(&self.up_concurrency),
            "XHTTP up_concurrency must be 1..16"
        );
        ensure!(
            (1024..=1_048_576).contains(&self.buffer_bytes),
            "XHTTP buffer_bytes must be 1024..1048576"
        );
        ensure!(
            (100..=60_000).contains(&self.probe_ms),
            "XHTTP probe_ms must be 100..60000"
        );
        ensure!(
            (5..=3600).contains(&self.session_timeout_sec),
            "XHTTP session_timeout_sec must be 5..3600"
        );
        ensure!(
            self.edge_addrs.len() <= 16,
            "XHTTP edge_addrs supports at most 16 explicitly configured addresses"
        );
        ensure!(
            self.pad_min <= self.pad_max && self.pad_max <= 4096,
            "XHTTP padding range must be ordered and at most 4096 bytes"
        );
        ensure!(
            !self.user_agent.is_empty()
                && self.user_agent.len() <= 256
                && self.user_agent.is_ascii()
                && !self.user_agent.bytes().any(|b| b.is_ascii_control()),
            "XHTTP user_agent must be 1..256 ASCII bytes without control characters"
        );
        ensure!(
            (65_536..=67_108_864).contains(&self.socket_buf_bytes),
            "XHTTP socket_buf_bytes must be 65536..67108864"
        );
        for addr in &self.edge_addrs {
            endpoint(addr).context("invalid XHTTP edge address")?;
        }
        if let Some(host) = &self.host {
            crate::carrier::http_authority(host)?;
            ensure!(
                !host.is_empty()
                    && host.len() <= 253
                    && host.is_ascii()
                    && !host.bytes().any(|b| b.is_ascii_control()
                        || b.is_ascii_whitespace()
                        || matches!(b, b'/' | b'\\' | b'?' | b'#')),
                "XHTTP host must be an ASCII HTTP authority without whitespace or URL path"
            );
        }
        Ok(())
    }
}

impl Listener {
    /// Physical HTTP dial settings for a logical server in reverse mode.
    pub fn dial_path(&self) -> ClientPath {
        ClientPath {
            addr: self.addr.clone(),
            transport: self.transport,
            ca_file: self.ca_file.clone(),
            server_name: self.server_name.clone(),
            cert_file: None,
            key_file: None,
            xhttp: self.xhttp.clone(),
            quantum: self.quantum.clone(),
            raw: self.raw.clone(),
            http_path: self.http_path.clone(),
            server_public_key: String::new(),
            connection_pool: self.connection_pool,
            retry_interval: 1,
            dial_timeout: 30,
        }
    }
}

impl ClientPath {
    /// Physical HTTP listening settings for a logical client in reverse mode.
    pub fn listen_config(&self) -> Listener {
        Listener {
            addr: self.addr.clone(),
            transport: self.transport,
            cert_file: self.cert_file.clone(),
            key_file: self.key_file.clone(),
            ca_file: None,
            server_name: None,
            connection_pool: 1,
            xhttp: self.xhttp.clone(),
            quantum: self.quantum.clone(),
            raw: self.raw.clone(),
            http_path: self.http_path.clone(),
            maps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortMap {
    #[serde(rename = "type")]
    pub kind: MapKind,
    pub bind: String,
    pub target: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Listener {
    pub addr: String,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub cert_file: Option<PathBuf>,
    #[serde(default)]
    pub key_file: Option<PathBuf>,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default = "one")]
    pub connection_pool: usize,
    #[serde(default)]
    pub xhttp: XhttpOptions,
    #[serde(default)]
    pub quantum: crate::quantum_carrier::QuantumOptions,
    #[serde(default)]
    pub raw: Option<crate::raw_packet::RawOptions>,
    #[serde(default = "default_http_path")]
    pub http_path: String,
    #[serde(default)]
    pub maps: Vec<PortMap>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientPath {
    pub addr: String,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub cert_file: Option<PathBuf>,
    #[serde(default)]
    pub key_file: Option<PathBuf>,
    #[serde(default)]
    pub xhttp: XhttpOptions,
    #[serde(default)]
    pub quantum: crate::quantum_carrier::QuantumOptions,
    #[serde(default)]
    pub raw: Option<crate::raw_packet::RawOptions>,
    #[serde(default = "default_http_path")]
    pub http_path: String,
    pub server_public_key: String,
    #[serde(default = "one")]
    pub connection_pool: usize,
    #[serde(default = "one_u64")]
    pub retry_interval: u64,
    #[serde(default = "ten")]
    pub dial_timeout: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TunConfig {
    pub name: String,
    pub address: String,
    pub peer_address: String,
    #[serde(default = "default_tun_mtu")]
    pub mtu: u16,
    /// Optional authenticated forwarding service carried by TCP over the inner TUN IPs.
    #[serde(default)]
    pub forwarding_port: Option<u16>,
}

fn default_tun_mtu() -> u16 {
    1400
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Non-operational JSON comment metadata, discarded during deserialization.
    #[serde(default, rename = "_comment", skip_serializing)]
    pub attribution_comment: Option<serde::de::IgnoredAny>,
    pub mode: Mode,
    /// Reverse physical HTTP dialing for CDN deployments. Logical roles and identities stay fixed.
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub tun: Option<TunConfig>,
    pub private_key_file: PathBuf,
    #[serde(default)]
    pub peer_public_keys: Vec<String>,
    #[serde(default)]
    pub listeners: Vec<Listener>,
    #[serde(default)]
    pub paths: Vec<ClientPath>,
    #[serde(default)]
    pub socks5: Option<String>,
    #[serde(default)]
    pub allowed_targets: Vec<String>,
    #[serde(default = "ten")]
    pub heartbeat_sec: u64,
    #[serde(default = "forty")]
    pub dead_timeout_sec: u64,
    #[serde(default = "streams")]
    pub max_streams: usize,
    #[serde(default = "connections")]
    pub max_connections: usize,
}

fn one() -> usize {
    1
}

fn default_http_path() -> String {
    "/tunnel".into()
}

fn validate_http_path(path: &str) -> Result<()> {
    ensure!(
        path.starts_with('/')
            && path.len() <= 512
            && path.is_ascii()
            && !path.bytes().any(|b| b.is_ascii_control()
                || b.is_ascii_whitespace()
                || matches!(b, b'?' | b'#' | b'\\')),
        "http_path must be an ASCII absolute URL path of at most 512 bytes without query or fragment"
    );
    Ok(())
}

fn validate_server_name(name: Option<&str>) -> Result<()> {
    let name = name.context("TLS dialer requires server_name matching the certificate")?;
    ensure!(
        !name.is_empty()
            && name.len() <= 253
            && name.is_ascii()
            && !name
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control()),
        "invalid TLS server_name"
    );
    Ok(())
}

fn validate_transport_address(transport: Transport, addr: &str) -> Result<()> {
    if transport == Transport::Dc6 {
        let address = addr
            .parse::<SocketAddr>()
            .context("dc6 requires a literal [IPv6]:port address")?;
        ensure!(address.is_ipv6(), "dc6 requires IPv6");
    }
    Ok(())
}

fn validate_carrier_options(
    transport: Transport,
    quantum: &crate::quantum_carrier::QuantumOptions,
    raw: Option<&crate::raw_packet::RawOptions>,
    tun: bool,
) -> Result<()> {
    quantum.validate()?;
    ensure!(
        quantum.peer_mtu.is_none()
            || matches!(transport, Transport::Quantum | Transport::QuantumGaming),
        "quantum peer_mtu requires a raw Quantum carrier"
    );
    if matches!(
        transport,
        Transport::Quantum | Transport::QuantumGaming | Transport::Tun
    ) {
        raw.context("quantum, quantum-gaming and tun require raw configuration")?
            .validate()?;
        if transport != Transport::Tun {
            ensure!(quantum.mtu >= 512, "raw Quantum MTU must be at least 512");
        }
    } else {
        ensure!(
            raw.is_none(),
            "raw configuration requires quantum, quantum-gaming or tun"
        );
    }
    ensure!(
        transport != Transport::Tun || tun,
        "the tun carrier requires a root tun interface configuration"
    );
    Ok(())
}

fn resolve_optional_path(path: &mut Option<PathBuf>, base: &Path) -> Result<()> {
    if let Some(path) = path {
        ensure!(
            !path.as_os_str().is_empty(),
            "TLS file path must not be empty"
        );
        if path.is_relative() {
            *path = base.join(&*path);
        }
    }
    Ok(())
}
fn one_u64() -> u64 {
    1
}
fn ten() -> u64 {
    10
}
fn forty() -> u64 {
    40
}
fn streams() -> usize {
    256
}
fn connections() -> usize {
    16
}

pub fn decode_key(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64,
        "key must contain exactly 64 hexadecimal characters"
    );
    let mut key = [0u8; 32];
    hex::decode_to_slice(value, &mut key).context("key contains invalid hexadecimal characters")?;
    ensure!(
        key.iter().any(|&b| b != 0),
        "all-zero keys are not accepted"
    );
    Ok(key)
}

fn endpoint(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 1024,
        "endpoint exceeds the wire protocol's 1024-byte target limit"
    );
    ensure!(
        !value.is_empty() && !value.chars().any(char::is_whitespace),
        "endpoint must be host:port without whitespace"
    );
    let (host, port) = value
        .rsplit_once(':')
        .context("endpoint must include a port")?;
    ensure!(!host.is_empty(), "endpoint must include a host");
    if host.contains(':') {
        ensure!(
            host.starts_with('[') && host.ends_with(']'),
            "IPv6 endpoints must use [address]:port"
        );
        host[1..host.len() - 1]
            .parse::<std::net::Ipv6Addr>()
            .context("invalid IPv6 address")?;
    } else {
        ensure!(
            host.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'),
            "invalid endpoint hostname"
        );
    }
    ensure!(
        port.parse::<u16>().context("invalid endpoint port")? != 0,
        "target port must be nonzero"
    );
    Ok(())
}

fn bind_address(value: &str) -> Result<()> {
    value
        .parse::<SocketAddr>()
        .context("listen/bind address must be an IP address and port (IPv6 uses brackets)")?;
    Ok(())
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)
            .with_context(|| format!("read configuration {}", path.display()))?;
        let mut config: Self =
            serde_json::from_str(&input).context("invalid configuration JSON")?;
        ensure!(
            !config.private_key_file.as_os_str().is_empty(),
            "private_key_file is required"
        );
        if config.private_key_file.is_relative() {
            config.private_key_file = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&config.private_key_file);
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for listener in &mut config.listeners {
            resolve_optional_path(&mut listener.cert_file, base)?;
            resolve_optional_path(&mut listener.key_file, base)?;
            resolve_optional_path(&mut listener.ca_file, base)?;
        }
        for path in &mut config.paths {
            resolve_optional_path(&mut path.ca_file, base)?;
            resolve_optional_path(&mut path.cert_file, base)?;
            resolve_optional_path(&mut path.key_file, base)?;
        }
        config.validate()?;
        Ok(config)
    }

    pub fn private_key(&self) -> Result<[u8; 32]> {
        let metadata = fs::metadata(&self.private_key_file).context("read private key metadata")?;
        ensure!(
            metadata.is_file() && metadata.len() <= 256,
            "private key must be a small regular hex text file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "private key permissions must exclude group and others; use chmod 600"
            );
        }
        let text = fs::read_to_string(&self.private_key_file).context("read private key file")?;
        decode_key(text.trim()).context("invalid private key file")
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.private_key_file.as_os_str().is_empty(),
            "private_key_file is required"
        );
        ensure!(
            (1..=3600).contains(&self.heartbeat_sec),
            "heartbeat_sec must be 1..3600"
        );
        ensure!(
            self.dead_timeout_sec > self.heartbeat_sec && self.dead_timeout_sec <= 86400,
            "dead_timeout_sec must exceed heartbeat_sec and be at most 86400"
        );
        ensure!(
            (1..=65536).contains(&self.max_streams),
            "max_streams must be 1..65536"
        );
        ensure!(
            (1..=1024).contains(&self.max_connections),
            "max_connections must be 1..1024"
        );
        for key in &self.peer_public_keys {
            decode_key(key).context("invalid peer_public_keys entry")?;
        }
        for target in &self.allowed_targets {
            if target != "*" {
                endpoint(target).context("invalid allowed_targets entry")?;
            }
        }
        if let Some(tun) = &self.tun {
            ensure!(
                !tun.name.is_empty()
                    && tun.name.len() <= 15
                    && tun
                        .name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')),
                "TUN name must contain 1..15 ASCII letters, digits, underscores or hyphens"
            );
            let address = tun
                .address
                .parse::<std::net::IpAddr>()
                .context("TUN address must be a literal IP without a prefix")?;
            let peer = tun
                .peer_address
                .parse::<std::net::IpAddr>()
                .context("TUN peer_address must be a literal IP without a prefix")?;
            ensure!(
                address.is_ipv4() == peer.is_ipv4() && address != peer,
                "TUN addresses must be distinct and use the same IP version"
            );
            ensure!(
                !address.is_unspecified()
                    && !address.is_multicast()
                    && !peer.is_unspecified()
                    && !peer.is_multicast(),
                "TUN addresses must be unicast"
            );
            ensure!(
                (576..=9000).contains(&tun.mtu) && (address.is_ipv4() || tun.mtu >= 1280),
                "TUN MTU must be 576..9000 (at least 1280 for IPv6)"
            );
            if let Some(port) = tun.forwarding_port {
                ensure!(port >= 1024, "tun.forwarding_port must be 1024..65535");
                ensure!(
                    !address.is_loopback() && !peer.is_loopback(),
                    "TUN forwarding addresses must not be loopback addresses"
                );
                for bind in self
                    .socks5
                    .iter()
                    .chain(self.listeners.iter().flat_map(|listener| {
                        listener
                            .maps
                            .iter()
                            .filter(|map| map.kind == MapKind::Tcp)
                            .map(|map| &map.bind)
                    }))
                {
                    let bind = bind
                        .parse::<SocketAddr>()
                        .context("invalid TUN forwarding service bind")?;
                    ensure!(
                        bind.port() != port
                            || bind.ip() != address
                                && !(bind.ip().is_unspecified()
                                    && bind.is_ipv4() == address.is_ipv4()),
                        "a TCP map or SOCKS bind conflicts with tun.forwarding_port"
                    );
                }
            } else {
                ensure!(
                    self.socks5.is_none()
                        && self.allowed_targets.is_empty()
                        && self.listeners.iter().all(|l| l.maps.is_empty()),
                    "TUN maps, socks5 or allowed_targets require tun.forwarding_port on both peers"
                );
            }
            match self.mode {
                Mode::Server => ensure!(
                    self.listeners.len() == 1 && self.listeners[0].connection_pool == 1,
                    "TUN server requires exactly one listener with connection_pool 1"
                ),
                Mode::Client => ensure!(
                    self.paths.len() == 1 && self.paths[0].connection_pool == 1,
                    "TUN client requires exactly one path with connection_pool 1"
                ),
            }
        }
        match self.mode {
            Mode::Server => {
                ensure!(
                    !self.listeners.is_empty(),
                    "server requires at least one listener"
                );
                ensure!(
                    !self.peer_public_keys.is_empty(),
                    "server requires at least one authorized peer_public_keys entry"
                );
                ensure!(self.paths.is_empty(), "paths are only valid in client mode");
                ensure!(
                    self.allowed_targets.is_empty(),
                    "allowed_targets are only valid in client mode"
                );
                if let Some(addr) = &self.socks5 {
                    bind_address(addr).context("invalid socks5 address")?;
                }
                let mut tcp_binds = std::collections::HashSet::new();
                let mut udp_binds = std::collections::HashSet::new();
                if self.reverse {
                    let total = self
                        .listeners
                        .iter()
                        .try_fold(0usize, |n, l| n.checked_add(l.connection_pool))
                        .context("reverse connection pool total overflow")?;
                    ensure!(
                        total <= self.max_connections,
                        "reverse listener pools exceed max_connections"
                    );
                }
                if let Some(addr) = &self.socks5 {
                    tcp_binds.insert(addr.parse::<SocketAddr>()?);
                }
                for listener in &self.listeners {
                    if self.reverse {
                        endpoint(&listener.addr)?;
                    } else {
                        bind_address(&listener.addr).context("invalid listener address")?;
                    }
                    validate_transport_address(listener.transport, &listener.addr)?;
                    validate_carrier_options(
                        listener.transport,
                        &listener.quantum,
                        listener.raw.as_ref(),
                        self.tun.is_some(),
                    )?;
                    if listener.raw.is_some() {
                        ensure!(
                            listener.connection_pool == 1,
                            "raw carriers require one connection per configured peer"
                        );
                    }
                    validate_http_path(&listener.http_path)?;
                    listener.xhttp.validate()?;
                    ensure!(
                        (1..=self.max_connections).contains(&listener.connection_pool),
                        "listener connection_pool exceeds max_connections"
                    );
                    if self.reverse {
                        ensure!(
                            matches!(listener.transport, Transport::Xhttp | Transport::Xhttps),
                            "reverse mode requires XHTTP or XHTTPS"
                        );
                        ensure!(
                            listener.cert_file.is_none() && listener.key_file.is_none(),
                            "reverse server uses ca_file/server_name for TLS"
                        );
                        if listener.transport.uses_tls() {
                            validate_server_name(listener.server_name.as_deref())?;
                        } else {
                            ensure!(
                                listener.ca_file.is_none() && listener.server_name.is_none(),
                                "TLS settings require XHTTPS"
                            );
                        }
                    } else if listener.transport.uses_tls() {
                        ensure!(
                            listener.cert_file.is_some() && listener.key_file.is_some(),
                            "TLS listener requires cert_file and key_file"
                        );
                    } else {
                        ensure!(
                            listener.cert_file.is_none() && listener.key_file.is_none(),
                            "cert_file and key_file require a TLS transport"
                        );
                    }
                    if !self.reverse {
                        ensure!(
                            listener.ca_file.is_none() && listener.server_name.is_none(),
                            "ca_file/server_name require reverse server mode"
                        );
                        let addr = listener.addr.parse::<SocketAddr>()?;
                        let listener_binds = if matches!(
                            listener.transport,
                            Transport::Kcp | Transport::QuantumPlus
                        ) {
                            &mut udp_binds
                        } else {
                            &mut tcp_binds
                        };
                        ensure!(
                            addr.port() == 0 || listener_binds.insert(addr),
                            "duplicate listener bind address"
                        );
                    }
                    for map in &listener.maps {
                        bind_address(&map.bind).context("invalid mapping bind address")?;
                        endpoint(&map.target).context("invalid mapping target")?;
                        let addr = map.bind.parse::<SocketAddr>()?;
                        let binds = match map.kind {
                            MapKind::Tcp => &mut tcp_binds,
                            MapKind::Udp => &mut udp_binds,
                        };
                        ensure!(
                            addr.port() == 0 || binds.insert(addr),
                            "duplicate mapping bind address"
                        );
                    }
                }
            }
            Mode::Client => {
                ensure!(!self.paths.is_empty(), "client requires at least one path");
                ensure!(
                    self.listeners.is_empty()
                        && self.socks5.is_none()
                        && self.peer_public_keys.is_empty(),
                    "listeners, socks5 and peer_public_keys are server-only settings"
                );
                let mut total = 0usize;
                for path in &self.paths {
                    endpoint(&path.addr).context("invalid path address")?;
                    if self.reverse {
                        bind_address(&path.addr)?;
                    }
                    validate_transport_address(path.transport, &path.addr)?;
                    validate_carrier_options(
                        path.transport,
                        &path.quantum,
                        path.raw.as_ref(),
                        self.tun.is_some(),
                    )?;
                    if path.raw.is_some() {
                        ensure!(
                            path.connection_pool == 1,
                            "raw carriers require connection_pool 1 per configured peer"
                        );
                    }
                    validate_http_path(&path.http_path)?;
                    path.xhttp.validate()?;
                    if self.reverse {
                        ensure!(
                            matches!(path.transport, Transport::Xhttp | Transport::Xhttps),
                            "reverse mode requires XHTTP or XHTTPS"
                        );
                        ensure!(
                            path.ca_file.is_none() && path.server_name.is_none(),
                            "reverse client uses cert_file/key_file for TLS"
                        );
                        ensure!(
                            path.connection_pool == 1,
                            "reverse client requires connection_pool 1 per listener"
                        );
                        if path.transport.uses_tls() {
                            ensure!(
                                path.cert_file.is_some() && path.key_file.is_some(),
                                "reverse XHTTPS client requires cert_file/key_file"
                            );
                        } else {
                            ensure!(
                                path.cert_file.is_none() && path.key_file.is_none(),
                                "TLS settings require XHTTPS"
                            );
                        }
                    } else if path.transport.uses_tls() {
                        let name = path.server_name.as_deref().context(
                            "TLS path requires server_name matching the server certificate",
                        )?;
                        ensure!(
                            !name.is_empty()
                                && name.len() <= 253
                                && name.is_ascii()
                                && !name
                                    .bytes()
                                    .any(|b| b.is_ascii_whitespace() || b.is_ascii_control()),
                            "invalid TLS server_name"
                        );
                    } else {
                        ensure!(
                            path.ca_file.is_none() && path.server_name.is_none(),
                            "ca_file and server_name require a TLS transport"
                        );
                    }
                    if !self.reverse {
                        ensure!(
                            path.cert_file.is_none() && path.key_file.is_none(),
                            "cert_file/key_file require reverse client mode"
                        );
                    }
                    decode_key(&path.server_public_key).context("invalid server_public_key")?;
                    ensure!(
                        (1..=1024).contains(&path.connection_pool),
                        "connection_pool must be 1..1024"
                    );
                    ensure!(
                        (1..=30).contains(&path.retry_interval),
                        "retry_interval must be 1..30 seconds"
                    );
                    ensure!(
                        (1..=300).contains(&path.dial_timeout),
                        "dial_timeout must be 1..300 seconds"
                    );
                    total = total
                        .checked_add(path.connection_pool)
                        .context("connection pool total overflow")?;
                }
                ensure!(
                    total <= self.max_connections,
                    "total connection_pool exceeds max_connections"
                );
            }
        }
        Ok(())
    }

    /// Builds the local forwarding overlay without re-entering TUN setup.
    pub(crate) fn tun_forwarding_config(&self) -> Result<Option<Self>> {
        let Some(tun) = &self.tun else {
            return Ok(None);
        };
        let Some(port) = tun.forwarding_port else {
            return Ok(None);
        };
        let mut inner = self.clone();
        inner.tun = None;
        inner.reverse = false;
        match inner.mode {
            Mode::Server => {
                let listener = inner
                    .listeners
                    .first_mut()
                    .context("TUN forwarding listener missing")?;
                listener.addr = SocketAddr::new(tun.address.parse()?, port).to_string();
                listener.transport = Transport::Tcp;
                listener.raw = None;
                listener.quantum = Default::default();
                listener.xhttp = Default::default();
                listener.http_path = default_http_path();
                listener.cert_file = None;
                listener.key_file = None;
                listener.ca_file = None;
                listener.server_name = None;
            }
            Mode::Client => {
                let path = inner
                    .paths
                    .first_mut()
                    .context("TUN forwarding path missing")?;
                path.addr = SocketAddr::new(tun.peer_address.parse()?, port).to_string();
                path.transport = Transport::Tcp;
                path.raw = None;
                path.quantum = Default::default();
                path.xhttp = Default::default();
                path.http_path = default_http_path();
                path.cert_file = None;
                path.key_file = None;
                path.ca_file = None;
                path.server_name = None;
            }
        }
        inner.validate()?;
        Ok(Some(inner))
    }
}

pub fn generate_keypair(directory: &Path) -> Result<(PathBuf, String)> {
    fs::create_dir_all(directory).context("create key directory")?;
    let private_path = directory.join("private.key");
    let public_path = directory.join("public.key");
    if private_path.exists() || public_path.exists() {
        bail!("key files already exist; choose a new directory");
    }
    let params = "Noise_IK_25519_ChaChaPoly_BLAKE2s".parse()?;
    let pair = snow::Builder::new(params)
        .generate_keypair()
        .context("generate Noise keypair")?;
    let public = hex::encode(&pair.public);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut private_file = options
        .open(&private_path)
        .context("create private key without overwriting")?;
    writeln!(private_file, "{}", hex::encode(&pair.private)).context("write private key")?;
    private_file.sync_all()?;
    let mut public_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&public_path)
        .context("create public key without overwriting")?;
    writeln!(public_file, "{public}")?;
    public_file.sync_all()?;
    Ok((private_path, public))
}

pub fn generate_certificate(directory: &Path, names: Vec<String>) -> Result<(PathBuf, PathBuf)> {
    ensure!(
        !names.is_empty(),
        "at least one certificate name is required"
    );
    let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(names)
        .context("generate self-signed TLS certificate")?;
    fs::create_dir_all(directory).context("create certificate directory")?;
    let cert_path = directory.join("cert.pem");
    let key_path = directory.join("key.pem");
    ensure!(
        !cert_path.exists() && !key_path.exists(),
        "certificate or key already exists; choose a new directory"
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut key_file = options
        .open(&key_path)
        .context("create TLS private key without overwriting")?;
    key_file.write_all(key_pair.serialize_pem().as_bytes())?;
    key_file.sync_all()?;
    let mut cert_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&cert_path)
        .context("create certificate without overwriting")?;
    cert_file.write_all(cert.pem().as_bytes())?;
    cert_file.sync_all()?;
    Ok((cert_path, key_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_legacy_psk_and_unknown_fields() {
        let value =
            serde_json::json!({"mode":"client","private_key_file":"private.key","psk":"legacy"});
        assert!(serde_json::from_value::<Config>(value).is_err());
    }

    #[test]
    fn generated_keys_load_relative_to_config_without_overwrite() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let key_dir = directory.path().join("keys");
        let (private_path, public) = generate_keypair(&key_dir)?;
        let original = fs::read(&private_path)?;
        assert!(generate_keypair(&key_dir).is_err());
        assert_eq!(fs::read(&private_path)?, original);
        let config_path = directory.path().join("config.json");
        fs::write(
            &config_path,
            serde_json::to_vec(&serde_json::json!({
                "mode":"client", "private_key_file":"keys/private.key",
                "paths":[{"addr":"localhost:7000","server_public_key":public}]
            }))?,
        )?;
        let config = Config::load(&config_path)?;
        assert_eq!(config.private_key_file, private_path);
        assert_eq!(
            config.private_key()?,
            decode_key(std::str::from_utf8(&original)?.trim())?
        );
        Ok(())
    }

    #[test]
    fn validates_endpoint_syntax_without_dns() {
        assert!(endpoint(&format!("{}:80", "a".repeat(1024))).is_err());
        for valid in ["localhost:80", "127.0.0.1:443", "[::1]:9000"] {
            assert!(endpoint(valid).is_ok());
        }
        for invalid in [
            "http://example.com:80",
            "::1:80",
            "host:0",
            "host:65536",
            "host",
            "host name:80",
        ] {
            assert!(endpoint(invalid).is_err());
        }
    }

    #[test]
    fn validates_transport_specific_settings() -> Result<()> {
        let mut value = serde_json::json!({
            "mode":"client", "private_key_file":"private.key",
            "paths":[{"addr":"127.0.0.1:7000", "transport":"wss", "server_public_key":"11".repeat(32)}]
        });
        assert!(
            serde_json::from_value::<Config>(value.clone())?
                .validate()
                .is_err()
        );
        value["paths"][0]["server_name"] = "localhost".into();
        serde_json::from_value::<Config>(value.clone())?.validate()?;
        value["paths"][0]["http_path"] = "/tunnel\r\ninjected: true".into();
        assert!(
            serde_json::from_value::<Config>(value.clone())?
                .validate()
                .is_err()
        );
        value["paths"][0]["http_path"] = "/tunnel".into();
        value["paths"][0]["transport"] = "quantum".into();
        assert!(serde_json::from_value::<Config>(value)?.validate().is_err());
        assert!(validate_transport_address(Transport::Dc6, "127.0.0.1:7000").is_err());
        validate_transport_address(Transport::Dc6, "[::1]:7000")?;
        Ok(())
    }

    #[test]
    fn certificate_generation_preserves_existing_files() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (cert, key) = generate_certificate(directory.path(), vec!["localhost".into()])?;
        let original_key = fs::read(&key)?;
        assert!(fs::read_to_string(cert)?.contains("BEGIN CERTIFICATE"));
        assert!(generate_certificate(directory.path(), vec!["localhost".into()]).is_err());
        assert_eq!(fs::read(&key)?, original_key);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(key)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn tun_validation_rejects_incompatible_routes_and_settings() -> Result<()> {
        let mut value = serde_json::json!({
            "mode":"client", "private_key_file":"private.key",
            "paths":[{"addr":"127.0.0.1:7000", "server_public_key":"11".repeat(32)}],
            "tun":{"name":"dagger0", "address":"10.25.0.2", "peer_address":"10.25.0.1"}
        });
        serde_json::from_value::<Config>(value.clone())?.validate()?;
        value["tun"]["peer_address"] = "::1".into();
        assert!(
            serde_json::from_value::<Config>(value.clone())?
                .validate()
                .is_err()
        );
        value["tun"]["peer_address"] = "10.25.0.1".into();
        value["allowed_targets"] = serde_json::json!(["*"]);
        assert!(
            serde_json::from_value::<Config>(value.clone())?
                .validate()
                .is_err()
        );
        value["allowed_targets"] = serde_json::json!([]);
        value["paths"][0]["connection_pool"] = 2.into();
        assert!(serde_json::from_value::<Config>(value)?.validate().is_err());
        Ok(())
    }

    #[test]
    fn tun_forwarding_uses_inner_ips_and_preserves_peer_authorization() -> Result<()> {
        for (local, peer) in [("10.25.0.1", "10.25.0.2"), ("fd42:25::1", "fd42:25::2")] {
            let server: Config = serde_json::from_value(serde_json::json!({
                "mode":"server", "private_key_file":"private.key", "peer_public_keys":["22".repeat(32)],
                "heartbeat_sec":2,"dead_timeout_sec":8,"socks5":"127.0.0.1:1080",
                "tun":{"name":"dagger0","address":local,"peer_address":peer,"forwarding_port":47475},
                "listeners":[{"addr":"192.0.2.1:443","transport":"tun","raw":{"peer_ip":"192.0.2.2"},
                    "maps":[{"type":"tcp","bind":"127.0.0.1:2222","target":"127.0.0.1:22"}]}]
            }))?;
            server.validate()?;
            let inner = server.tun_forwarding_config()?.unwrap();
            assert!(inner.tun.is_none() && !inner.reverse);
            assert_eq!(
                inner.listeners[0].addr,
                SocketAddr::new(local.parse()?, 47475).to_string()
            );
            assert_eq!(inner.listeners[0].transport, Transport::Tcp);
            assert!(inner.listeners[0].raw.is_none());
            assert_eq!(
                inner.listeners[0].maps[0].target,
                server.listeners[0].maps[0].target
            );
            assert_eq!(inner.socks5, server.socks5);
            assert_eq!(inner.peer_public_keys, server.peer_public_keys);
            assert_eq!(inner.private_key_file, server.private_key_file);
            assert_eq!((inner.heartbeat_sec, inner.dead_timeout_sec), (2, 8));
            let client: Config = serde_json::from_value(serde_json::json!({
                "mode":"client", "private_key_file":"private.key", "allowed_targets":["127.0.0.1:22"],
                "tun":{"name":"dagger0","address":peer,"peer_address":local,"forwarding_port":47475},
                "paths":[{"addr":"192.0.2.1:443","transport":"tun","raw":{"peer_ip":"192.0.2.1"},
                    "server_public_key":"11".repeat(32),"retry_interval":2,"dial_timeout":5}]
            }))?;
            client.validate()?;
            let inner = client.tun_forwarding_config()?.unwrap();
            assert_eq!(
                inner.paths[0].addr,
                SocketAddr::new(local.parse()?, 47475).to_string()
            );
            assert_eq!(inner.paths[0].transport, Transport::Tcp);
            assert!(inner.paths[0].raw.is_none());
            assert_eq!(
                inner.paths[0].server_public_key,
                client.paths[0].server_public_key
            );
            assert_eq!(inner.allowed_targets, client.allowed_targets);
            assert_eq!(
                (inner.paths[0].retry_interval, inner.paths[0].dial_timeout),
                (2, 5)
            );
        }
        Ok(())
    }

    #[test]
    fn tun_forwarding_requires_explicit_port_and_rejects_service_collisions() -> Result<()> {
        let mut value = serde_json::json!({
            "mode":"server", "private_key_file":"private.key", "peer_public_keys":["22".repeat(32)],
            "tun":{"name":"dagger0","address":"10.25.0.1","peer_address":"10.25.0.2"},
            "listeners":[{"addr":"192.0.2.1:443","transport":"tun","raw":{"peer_ip":"192.0.2.2"}}]
        });
        let packet_only: Config = serde_json::from_value(value.clone())?;
        packet_only.validate()?;
        assert!(packet_only.tun_forwarding_config()?.is_none());
        value["socks5"] = "127.0.0.1:1080".into();
        assert!(
            serde_json::from_value::<Config>(value.clone())?
                .validate()
                .is_err()
        );
        for port in [0, 1023] {
            value["tun"]["forwarding_port"] = port.into();
            assert!(
                serde_json::from_value::<Config>(value.clone())?
                    .validate()
                    .is_err()
            );
        }
        value["tun"]["forwarding_port"] = 47475.into();
        serde_json::from_value::<Config>(value.clone())?.validate()?;
        for bind in ["10.25.0.1:47475", "0.0.0.0:47475"] {
            value["socks5"] = bind.into();
            assert!(
                serde_json::from_value::<Config>(value.clone())?
                    .validate()
                    .is_err()
            );
        }
        value["socks5"] = "127.0.0.1:1080".into();
        value["listeners"][0]["maps"] = serde_json::json!([
            {"type":"tcp","bind":"0.0.0.0:47475","target":"127.0.0.1:22"}
        ]);
        assert!(serde_json::from_value::<Config>(value)?.validate().is_err());
        Ok(())
    }

    #[test]
    fn tun_forwarding_clears_outer_tls_and_http_options() -> Result<()> {
        let config: Config = serde_json::from_value(serde_json::json!({
            "mode":"client","reverse":true,"private_key_file":"private.key",
            "tun":{"name":"dagger0","address":"fd42:25::2","peer_address":"fd42:25::1","forwarding_port":47475},
            "paths":[{"addr":"[::1]:443","transport":"xhttps","cert_file":"cert.pem","key_file":"key.pem",
                "http_path":"/outer","xhttp":{"host":"localhost","edge_addrs":["127.0.0.1:443"]},
                "server_public_key":"11".repeat(32)}]
        }))?;
        config.validate()?;
        let inner = config.tun_forwarding_config()?.unwrap();
        inner.validate()?;
        let path = &inner.paths[0];
        assert!(!inner.reverse && inner.tun.is_none());
        assert!(
            path.cert_file.is_none()
                && path.key_file.is_none()
                && path.ca_file.is_none()
                && path.server_name.is_none()
        );
        assert!(path.xhttp.host.is_none() && path.xhttp.edge_addrs.is_empty());
        assert_eq!(path.http_path, default_http_path());
        assert_eq!(path.transport, Transport::Tcp);
        Ok(())
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
