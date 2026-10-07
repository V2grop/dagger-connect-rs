use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use dagger_rs::{
    config::{Config, generate_certificate, generate_keypair},
    engine,
};
use std::path::PathBuf;
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

#[derive(Parser)]
#[command(
    version,
    about = "Independent encrypted reverse tunnel using locally pinned peer keys",
    after_help = dagger_rs::COMMUNITY
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(short, long, conflicts_with = "gen")]
    config: Option<PathBuf>,
    #[arg(long, requires = "config")]
    check: bool,
    #[arg(long, value_enum)]
    r#gen: Option<ExampleMode>,
}

#[derive(Subcommand)]
enum Command {
    /// Diagnose a link using only explicitly selected peers and ports.
    Linktest {
        #[command(subcommand)]
        action: LinkTestCommand,
    },
    /// Generate a fresh keypair. Existing files are never replaced.
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Generate a local TLS certificate. Copy cert.pem to the client's ca_file.
    Certgen {
        #[arg(long)]
        out: PathBuf,
        /// Certificate DNS name or IP address; may be repeated.
        #[arg(long, default_value = "localhost")]
        name: Vec<String>,
    },
}

#[derive(Subcommand)]
enum LinkTestCommand {
    /// Listen for the selected peer's connectivity and throughput probes.
    Listen {
        #[arg(long, default_value = "127.0.0.1:47000")]
        bind: SocketAddr,
        #[arg(long)]
        peer: IpAddr,
        #[arg(long, value_delimiter = ',')]
        extra_ports: Vec<u16>,
        #[arg(long,default_value_t=900,value_parser=clap::value_parser!(u64).range(1..=3600))]
        wait_secs: u64,
        #[arg(long)]
        keep: bool,
    },
    /// Check TCP/UDP in both directions, integrity and local test throughput.
    Probe {
        #[arg(long)]
        peer: SocketAddr,
        #[arg(long)]
        bind: Option<IpAddr>,
        #[arg(long, default_value_t = 0)]
        local_port: u16,
        #[arg(long, value_delimiter = ',')]
        extra_ports: Vec<u16>,
        #[arg(long,default_value="4",value_parser=throughput_seconds)]
        seconds: f64,
        #[arg(long,default_value_t=3,value_parser=clap::value_parser!(u64).range(1..=60))]
        timeout_secs: u64,
        #[arg(long)]
        quick: bool,
        #[arg(long)]
        save: Option<PathBuf>,
    },
}

fn throughput_seconds(value: &str) -> std::result::Result<f64, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| "seconds must be a number".to_string())?;
    if seconds.is_finite() && (0.05..=30.0).contains(&seconds) {
        Ok(seconds)
    } else {
        Err("seconds must be 0.05..30".into())
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ExampleMode {
    Server,
    Client,
}

fn example(mode: ExampleMode) -> serde_json::Value {
    match mode {
        ExampleMode::Server => serde_json::json!({
            "_comment":dagger_rs::ATTRIBUTION_COMMENT,
            "mode":"server", "private_key_file":"server-keys/private.key",
            "peer_public_keys":["REPLACE_WITH_CLIENT_PUBLIC_KEY_HEX"],
            "listeners":[{"addr":"0.0.0.0:7000","maps":[
                {"type":"tcp","bind":"127.0.0.1:8080","target":"127.0.0.1:8080"}
            ]}]
        }),
        ExampleMode::Client => serde_json::json!({
            "_comment":dagger_rs::ATTRIBUTION_COMMENT,
            "mode":"client", "private_key_file":"client-keys/private.key",
            "allowed_targets":["127.0.0.1:8080"],
            "paths":[{"addr":"127.0.0.1:7000","server_public_key":"REPLACE_WITH_SERVER_PUBLIC_KEY_HEX","connection_pool":1}]
        }),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        anyhow::ensure!(
            cli.config.is_none() && cli.r#gen.is_none() && !cli.check,
            "subcommands cannot be combined with configuration options"
        );
        match command {
            Command::Linktest { action } => match action {
                LinkTestCommand::Listen {
                    bind,
                    peer,
                    extra_ports,
                    wait_secs,
                    keep,
                } => {
                    eprintln!("Link tester listening on {bind} for peer {peer}");
                    let options = dagger_rs::linktest::ListenOptions {
                        bind,
                        expected_peer: peer,
                        extra_ports,
                        wait: Duration::from_secs(wait_secs),
                        keep,
                    };
                    tokio::select! {
                        result=dagger_rs::linktest::listen(options)=>result?,
                        result=tokio::signal::ctrl_c()=>result.context("install shutdown signal handler")?,
                    }
                }
                LinkTestCommand::Probe {
                    peer,
                    bind,
                    local_port,
                    extra_ports,
                    seconds,
                    timeout_secs,
                    quick,
                    save,
                } => {
                    let bind = bind.unwrap_or_else(|| {
                        if peer.is_ipv4() {
                            IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
                        } else {
                            IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)
                        }
                    });
                    let report = dagger_rs::linktest::probe(dagger_rs::linktest::ProbeOptions {
                        peer,
                        bind,
                        local_port,
                        extra_ports,
                        seconds: Duration::from_secs_f64(seconds),
                        timeout: Duration::from_secs(timeout_secs),
                        quick,
                    })
                    .await?;
                    let json = serde_json::to_string_pretty(&report)?;
                    if let Some(path) = save {
                        std::fs::write(&path, &json)
                            .with_context(|| format!("save link report {}", path.display()))?;
                    }
                    println!("{json}");
                    anyhow::ensure!(
                        report.passed(),
                        "one or more link checks failed; see JSON results"
                    );
                }
            },
            Command::Keygen { out } => {
                let (path, public) = generate_keypair(&out)?;
                println!(
                    "Private key saved to {}\nPublic key: {public}",
                    path.display()
                );
            }
            Command::Certgen { out, name } => {
                let (cert, key) = generate_certificate(&out, name)?;
                println!(
                    "Certificate saved to {}\nTLS private key saved to {}",
                    cert.display(),
                    key.display()
                );
            }
        }
        return Ok(());
    }
    if let Some(mode) = cli.r#gen {
        println!("{}", serde_json::to_string_pretty(&example(mode))?);
        return Ok(());
    }
    let path = cli
        .config
        .context("provide --config <file>, --gen server|client, or keygen --out <directory>")?;
    let config = Config::load(&path)?;
    config.private_key()?;
    dagger_rs::transport::validate_tls_config(&config)?;
    if cli.check {
        println!("Configuration and private key are valid.");
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    tokio::select! {
        result = engine::run(config) => result,
        signal = tokio::signal::ctrl_c() => {
            signal.context("install shutdown signal handler")?;
            tracing::info!("shutting down");
            Ok(())
        }
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
