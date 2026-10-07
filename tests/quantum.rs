//! Real UDP/KCP/FEC tests, including loss of one data shard in every FEC block.
use anyhow::{Result, ensure};
use dagger_rs::{
    config::{ClientPath, Listener},
    quantum_carrier::{self, QuantumListener},
};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UdpSocket,
    time::timeout,
};

fn listener(addr: &str, knock: bool) -> Result<Listener> {
    Ok(serde_json::from_value(
        json!({"addr":addr,"transport":"quantum+",
        "quantum":{"knock":knock},"maps":[]}),
    )?)
}
fn path(addr: SocketAddr, knock: bool) -> Result<ClientPath> {
    Ok(serde_json::from_value(
        json!({"addr":addr.to_string(),"transport":"quantum+",
        "server_public_key":"00".repeat(32),"quantum":{"knock":knock}}),
    )?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quantum_plus_transfers_with_one_lost_data_shard_per_block() -> Result<()> {
    let losses = Arc::new(AtomicUsize::new(0));
    let parities = Arc::new(AtomicUsize::new(0));
    let client_stage = Arc::new(AtomicUsize::new(0));
    let server_stage = Arc::new(AtomicUsize::new(0));
    let result = timeout(Duration::from_secs(45), async {
        let mut server = QuantumListener::bind(&listener("127.0.0.1:0", false)?).await?;
        let server_address = server.local_addr();
        let proxy = UdpSocket::bind("127.0.0.1:0").await?;
        let proxy_address = proxy.local_addr()?;
        let dropped = losses.clone();
        let parity = parities.clone();
        let proxy_task = tokio::spawn(async move {
            let mut client = None;
            let mut buffer = vec![0; 9001];
            while let Ok((size, remote)) = proxy.recv_from(&mut buffer).await {
                let target = if remote == server_address {
                    if let Some(address) = client {
                        address
                    } else {
                        continue;
                    }
                } else {
                    client = Some(remote);
                    server_address
                };
                if size >= 8 {
                    let seq = u32::from_le_bytes(buffer[..4].try_into().unwrap());
                    let kind = u16::from_le_bytes(buffer[4..6].try_into().unwrap());
                    if seq % 11 == 3 && kind == 0xf1 {
                        dropped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    if kind == 0xf2 {
                        parity.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if proxy.send_to(&buffer[..size], target).await.is_err() {
                    break;
                }
            }
        });
        let echo_stage = server_stage.clone();
        let echo = tokio::spawn(async move {
            echo_stage.store(1, Ordering::Relaxed);
            let mut stream = server.accept().await?;
            let (mut read, mut write) = tokio::io::split(&mut stream);
            let mut payload = vec![0; 1_048_577];
            echo_stage.store(2, Ordering::Relaxed);
            read.read_exact(&mut payload).await?;
            echo_stage.store(3, Ordering::Relaxed);
            write.write_all(&payload).await?;
            echo_stage.store(4, Ordering::Relaxed);
            write.flush().await?;
            // Keep retransmissions alive until the client received the full echo.
            echo_stage.store(5, Ordering::Relaxed);
            let mut finished = [0; 4];
            read.read_exact(&mut finished).await?;
            ensure!(
                &finished == b"done",
                "missing final loss-test acknowledgement"
            );
            echo_stage.store(6, Ordering::Relaxed);
            Ok::<_, anyhow::Error>(())
        });
        let mut client = quantum_carrier::connect(&path(proxy_address, false)?).await?;
        client_stage.store(1, Ordering::Relaxed);
        let payload: Vec<_> = (0..1_048_577).map(|index| (index * 31) as u8).collect();
        let (mut reader, mut writer) = tokio::io::split(&mut client);
        let mut received = vec![0; payload.len()];
        tokio::try_join!(
            async {
                writer.write_all(&payload).await?;
                client_stage.fetch_max(2, Ordering::Relaxed);
                writer.flush().await?;
                client_stage.fetch_max(3, Ordering::Relaxed);
                Ok::<_, std::io::Error>(())
            },
            async {
                reader.read_exact(&mut received).await?;
                client_stage.fetch_max(4, Ordering::Relaxed);
                Ok::<_, std::io::Error>(())
            }
        )?;
        ensure!(received == payload, "Quantum+ lost or changed bytes");
        ensure!(
            losses.load(Ordering::Relaxed) > 10,
            "loss injector was not exercised"
        );
        ensure!(
            parities.load(Ordering::Relaxed) > 10,
            "wire carried no parity packets"
        );
        writer.write_all(b"done").await?;
        writer.flush().await?;
        client_stage.store(5, Ordering::Relaxed);
        echo.await??;
        proxy_task.abort();
        Ok::<_, anyhow::Error>(())
    })
    .await;
    ensure!(
        result.is_ok(),
        "Quantum+ loss deadline: client stage{} server stage{} lost shards{} parity packets{} (client4=received, server5=awaiting receipt)",
        client_stage.load(Ordering::Relaxed),
        server_stage.load(Ordering::Relaxed),
        losses.load(Ordering::Relaxed),
        parities.load(Ordering::Relaxed)
    );
    result??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quantum_plus_knock_connects_and_listener_drop_releases_both_ports() -> Result<()> {
    timeout(Duration::from_secs(10), async {
        let mut server = QuantumListener::bind(&listener("127.0.0.1:0", true)?).await?;
        let address = server.local_addr();
        let echo = tokio::spawn(async move {
            let mut stream = server.accept().await?;
            let mut data = [0; 4];
            stream.read_exact(&mut data).await?;
            stream.write_all(&data).await?;
            stream.flush().await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok::<_, anyhow::Error>(())
        });
        let mut client = quantum_carrier::connect(&path(address, true)?).await?;
        client.write_all(b"ping").await?;
        client.flush().await?;
        let mut response = [0; 4];
        client.read_exact(&mut response).await?;
        ensure!(
            &response == b"ping",
            "knock accepted but KCP transfer failed"
        );
        echo.await??;
        drop(client);
        tokio::task::yield_now().await;
        let _released = UdpSocket::bind(address).await?;
        let knock_port = if address.port() <= 55_535 {
            address.port() + 10_000
        } else {
            address.port() - 1000
        };
        let _knock_released = UdpSocket::bind(SocketAddr::new(address.ip(), knock_port)).await?;
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_network_peer_can_reconnect_without_evicting_an_existing_conversation() -> Result<()> {
    timeout(Duration::from_secs(15), async {
        let mut settings = listener("127.0.0.1:0", false)?;
        settings.quantum.max_peers = 2;
        let mut server = QuantumListener::bind(&settings).await?;
        let address = server.local_addr();
        let proxy = UdpSocket::bind("127.0.0.1:0").await?;
        let proxy_address = proxy.local_addr()?;
        let proxy_task = tokio::spawn(async move {
            let mut clients = std::collections::HashMap::new();
            let mut buffer = vec![0; 1501];
            while let Ok((size, remote)) = proxy.recv_from(&mut buffer).await {
                if size < 32 {
                    continue;
                }
                let conversation = u32::from_le_bytes(buffer[8..12].try_into().unwrap());
                let target = if remote == address {
                    if let Some(client) = clients.get(&conversation) {
                        *client
                    } else {
                        continue;
                    }
                } else {
                    clients.insert(conversation, remote);
                    address
                };
                if proxy.send_to(&buffer[..size], target).await.is_err() {
                    break;
                }
            }
        });
        let echo = tokio::spawn(async move {
            let mut echoes = Vec::new();
            for _ in 0..2 {
                let mut stream = server.accept().await?;
                echoes.push(tokio::spawn(async move {
                    for _ in 0..2 {
                        let mut data = [0; 4];
                        stream.read_exact(&mut data).await?;
                        stream.write_all(&data).await?;
                        stream.flush().await?;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok::<_, anyhow::Error>(())
                }));
            }
            for task in echoes {
                task.await??;
            }
            Ok::<_, anyhow::Error>(())
        });
        async fn exchange<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
            stream: &mut S,
            payload: &[u8; 4],
        ) -> Result<()> {
            stream.write_all(payload).await?;
            stream.flush().await?;
            let mut response = [0; 4];
            stream.read_exact(&mut response).await?;
            ensure!(&response == payload, "conversation changed or lost bytes");
            Ok(())
        }
        let mut old = quantum_carrier::connect(&path(proxy_address, false)?).await?;
        exchange(&mut old, b"old1").await?;
        let mut fresh = quantum_carrier::connect(&path(proxy_address, false)?).await?;
        exchange(&mut fresh, b"new1").await?;
        let mut excess = quantum_carrier::connect(&path(proxy_address, false)?).await?;
        excess.write_all(b"cap3").await?;
        excess.flush().await?;
        let mut ignored = [0; 4];
        ensure!(
            timeout(Duration::from_millis(200), excess.read_exact(&mut ignored))
                .await
                .is_err(),
            "conversation cap was not enforced"
        );
        exchange(&mut old, b"old2").await?;
        exchange(&mut fresh, b"new2").await?;
        echo.await??;
        proxy_task.abort();
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_adaptive_tuning_keeps_a_live_udp_stream_usable_across_timer_updates() -> Result<()>
{
    let _ = tracing_subscriber::fmt()
        .with_env_filter("dagger_rs=debug")
        .try_init();
    let client_stage = Arc::new(AtomicUsize::new(0));
    let server_stage = Arc::new(AtomicUsize::new(0));
    let pushed = Arc::new(AtomicUsize::new(0));
    let acked = Arc::new(AtomicUsize::new(0));
    let largest_window = Arc::new(AtomicUsize::new(0));
    let result = timeout(Duration::from_secs(15), async {
        let mut settings = listener("127.0.0.1:0", false)?;
        settings.quantum.tuner.mode = quantum_carrier::QuantumTunerMode::Auto;
        settings.quantum.tuner.memory_budget_mb = 32;
        settings.quantum.socket_buf_bytes = 8 * 1024 * 1024;
        settings.quantum.tuner.min_buffer_bytes = 4 * 1024 * 1024;
        settings.quantum.nodelay = Some(false);
        settings.quantum.interval_ms = Some(30);
        settings.quantum.congestion_control = Some(true);
        settings.quantum.write_delay = Some(true);
        settings.quantum.ack_no_delay = Some(false);
        let mut server = QuantumListener::bind(&settings).await?;
        let address = server.local_addr();
        let proxy = UdpSocket::bind("127.0.0.1:0").await?;
        let proxy_address = proxy.local_addr()?;
        let observed_window = largest_window.clone();
        let observed_pushes = pushed.clone();
        let observed_acks = acked.clone();
        let proxy_task = tokio::spawn(async move {
            let mut client = None;
            let mut buffer = vec![0; 1501];
            while let Ok((size, remote)) = proxy.recv_from(&mut buffer).await {
                let target = if remote == address {
                    if let Some(client) = client {
                        client
                    } else {
                        continue;
                    }
                } else {
                    client = Some(remote);
                    address
                };
                if size >= 32 && u16::from_le_bytes(buffer[4..6].try_into().unwrap()) == 0xf1 {
                    // Data FEC header8bytes, then KCP's receive-window field6bytes in.
                    let window = u16::from_le_bytes(buffer[14..16].try_into().unwrap()) as usize;
                    observed_window.fetch_max(window, Ordering::Relaxed);
                    if buffer[12] == 81 {
                        observed_pushes.fetch_add(1, Ordering::Relaxed);
                    }
                    if buffer[12] == 82 {
                        observed_acks.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if proxy.send_to(&buffer[..size], target).await.is_err() {
                    break;
                }
            }
        });
        let echo_stage = server_stage.clone();
        let echo = tokio::spawn(async move {
            let mut stream = server.accept().await?;
            for iteration in 0..4 {
                echo_stage.store(iteration * 10 + 1, Ordering::Relaxed);
                let mut data = vec![0; 65_537];
                stream.read_exact(&mut data).await?;
                echo_stage.store(iteration * 10 + 2, Ordering::Relaxed);
                stream.write_all(&data).await?;
                echo_stage.store(iteration * 10 + 3, Ordering::Relaxed);
                stream.flush().await?;
                eprintln!("adaptive server echoed round {iteration}");
            }
            // A KCP flush schedules output; congestion control can leave a tail
            // queued. Keep the server alive until the client confirms receipt.
            echo_stage.store(40, Ordering::Relaxed);
            let mut finished = [0; 4];
            stream.read_exact(&mut finished).await?;
            ensure!(
                &finished == b"done",
                "missing final application acknowledgement"
            );
            Ok::<_, anyhow::Error>(())
        });
        let mut client_settings = path(proxy_address, false)?;
        client_settings.quantum.tuner.mode = quantum_carrier::QuantumTunerMode::Auto;
        client_settings.quantum.tuner.memory_budget_mb = 32;
        client_settings.quantum.socket_buf_bytes = 8 * 1024 * 1024;
        client_settings.quantum.tuner.min_buffer_bytes = 4 * 1024 * 1024;
        client_settings.quantum.nodelay = Some(false);
        client_settings.quantum.interval_ms = Some(30);
        client_settings.quantum.congestion_control = Some(true);
        client_settings.quantum.write_delay = Some(true);
        client_settings.quantum.ack_no_delay = Some(false);
        let mut client = quantum_carrier::connect(&client_settings).await?;
        for iteration in 0..4 {
            client_stage.store(iteration * 10 + 1, Ordering::Relaxed);
            let payload: Vec<_> = (0..65_537)
                .map(|index| (index * 17 + iteration) as u8)
                .collect();
            client.write_all(&payload).await?;
            client_stage.store(iteration * 10 + 2, Ordering::Relaxed);
            client.flush().await?;
            client_stage.store(iteration * 10 + 3, Ordering::Relaxed);
            let mut response = vec![0; payload.len()];
            client.read_exact(&mut response).await?;
            client_stage.store(iteration * 10 + 4, Ordering::Relaxed);
            eprintln!("adaptive client received round {iteration}");
            ensure!(
                payload == response,
                "adaptive controller changed stream data"
            );
            if iteration < 3 {
                tokio::time::sleep(Duration::from_millis(1100)).await;
            }
        }
        ensure!(
            largest_window.load(Ordering::Relaxed) > 256,
            "live KCP window was never increased on the wire"
        );
        client_stage.store(40, Ordering::Relaxed);
        client.write_all(b"done").await?;
        client_stage.store(41, Ordering::Relaxed);
        client.flush().await?;
        echo.await??;
        proxy_task.abort();
        Ok::<_, anyhow::Error>(())
    })
    .await;
    ensure!(
        result.is_ok(),
        "adaptive deadline: client stage{} server stage{} PUSH packets{} ACK packets{} largest window{}",
        client_stage.load(Ordering::Relaxed),
        server_stage.load(Ordering::Relaxed),
        pushed.load(Ordering::Relaxed),
        acked.load(Ordering::Relaxed),
        largest_window.load(Ordering::Relaxed)
    );
    result??;
    Ok(())
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
