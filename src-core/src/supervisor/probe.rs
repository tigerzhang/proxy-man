use crate::config::ProbeType;
use anyhow::{bail, Context, Result};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;
use tracing::debug;

pub struct ProbeResult {
    pub success: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}

pub async fn run_probe(
    probe_type: ProbeType,
    host: &str,
    port: u16,
    test_target: Option<&str>,
    timeout_duration: Duration,
) -> ProbeResult {
    let start = Instant::now();
    let res = match probe_type {
        ProbeType::Socks5 => probe_socks5(host, port, timeout_duration).await,
        ProbeType::Http => probe_http(host, port, test_target, timeout_duration).await,
        ProbeType::Dns => probe_dns(host, port, test_target, timeout_duration).await,
        ProbeType::Tcp => probe_tcp(host, port, timeout_duration).await,
        ProbeType::None => Ok(()),
    };

    let elapsed = start.elapsed().as_millis() as u64;
    match res {
        Ok(()) => ProbeResult {
            success: true,
            latency_ms: elapsed.max(1),
            error: None,
        },
        Err(err) => {
            debug!("Probe failed for {}:{}: {}", host, port, err);
            ProbeResult {
                success: false,
                latency_ms: elapsed,
                error: Some(err.to_string()),
            }
        }
    }
}

pub async fn probe_tcp(host: &str, port: u16, timeout_duration: Duration) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    timeout(timeout_duration, TcpStream::connect(&addr))
        .await
        .context("Connection timed out")?
        .context("TCP connect failed")?;
    Ok(())
}

pub async fn probe_socks5(host: &str, port: u16, timeout_duration: Duration) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    let mut stream = timeout(timeout_duration, TcpStream::connect(&addr))
        .await
        .context("Connection timed out")?
        .context("TCP connect failed")?;

    // SOCKS5 greeting: VER=0x05, NMETHODS=0x01, METHOD=0x00 (NO AUTH)
    let greeting = [0x05, 0x01, 0x00];
    timeout(timeout_duration, stream.write_all(&greeting))
        .await
        .context("Write timed out")?
        .context("Failed to send SOCKS5 greeting")?;

    let mut response = [0u8; 2];
    timeout(timeout_duration, stream.read_exact(&mut response))
        .await
        .context("Read timed out")?
        .context("Failed to read SOCKS5 greeting response")?;

    if response[0] != 0x05 {
        bail!("Invalid SOCKS version: 0x{:02x}", response[0]);
    }
    if response[1] != 0x00 {
        bail!(
            "SOCKS5 server rejected NO AUTH method: 0x{:02x}",
            response[1]
        );
    }

    Ok(())
}

pub async fn probe_http(
    host: &str,
    port: u16,
    _test_target: Option<&str>,
    timeout_duration: Duration,
) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    let mut stream = timeout(timeout_duration, TcpStream::connect(&addr))
        .await
        .context("Connection timed out")?
        .context("TCP connect failed")?;

    // Send HTTP CONNECT probe or simple HEAD probe to test responsiveness
    let probe_req = "CONNECT 127.0.0.1:80 HTTP/1.1\r\nHost: 127.0.0.1:80\r\n\r\n";
    timeout(timeout_duration, stream.write_all(probe_req.as_bytes()))
        .await
        .context("Write timed out")?
        .context("Failed to send HTTP probe")?;

    let mut buf = [0u8; 12];
    let n = timeout(timeout_duration, stream.read(&mut buf))
        .await
        .context("Read timed out")?
        .context("Failed to read response")?;

    if n >= 4 && &buf[..4] == b"HTTP" {
        Ok(())
    } else {
        bail!("Non-HTTP response received from proxy");
    }
}

pub async fn probe_dns(
    host: &str,
    port: u16,
    test_domain: Option<&str>,
    timeout_duration: Duration,
) -> Result<()> {
    let domain = test_domain.unwrap_or("one.one.one.one");
    let target_addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .context("Invalid target DNS address")?;

    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("Failed to bind UDP socket for DNS probe")?;

    // Build standard 12-byte DNS header + Question section (A record query)
    // ID=0x1234, QR=0, Opcode=0, RD=1 -> 0x0100
    // QDCOUNT=1, ANCOUNT=0, NSCOUNT=0, ARCOUNT=0
    let mut packet = vec![
        0x12, 0x34, // Transaction ID
        0x01, 0x00, // Flags: Standard query, recursion desired
        0x00, 0x01, // Questions: 1
        0x00, 0x00, // Answer RRs: 0
        0x00, 0x00, // Authority RRs: 0
        0x00, 0x00, // Additional RRs: 0
    ];

    for label in domain.split('.') {
        if label.is_empty() {
            continue;
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0x00); // Root null byte

    // QTYPE: A (0x0001)
    packet.extend_from_slice(&[0x00, 0x01]);
    // QCLASS: IN (0x0001)
    packet.extend_from_slice(&[0x00, 0x01]);

    timeout(timeout_duration, socket.send_to(&packet, target_addr))
        .await
        .context("DNS send timed out")?
        .context("DNS send failed")?;

    let mut buf = [0u8; 512];
    let (len, _) = timeout(timeout_duration, socket.recv_from(&mut buf))
        .await
        .context("DNS recv timed out")?
        .context("DNS recv failed")?;

    if len >= 12 && buf[0] == 0x12 && buf[1] == 0x34 {
        Ok(())
    } else {
        bail!("Malformed DNS response or transaction ID mismatch");
    }
}
