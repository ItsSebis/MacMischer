//! Broadcast discovery of X Air mixers.

use crate::state::MixerInfo;
use rosc::OscType;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use xair_proto::{osc, MIXER_PORT};

/// Result of a discovery round.
#[derive(Debug, Default)]
pub struct Discovery {
    pub found: Vec<(SocketAddr, MixerInfo)>,
    /// Set when no probe could be sent at all (e.g. macOS blocking local-network access, no network).
    pub send_error: Option<String>,
}

/// Subnet broadcast address of every up, non-loopback IPv4 interface.
fn interface_broadcasts() -> Vec<IpAddr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) => v4.broadcast.map(IpAddr::V4),
            _ => None,
        })
        .collect()
}

/// Broadcast `/xinfo` to each target, 255.255.255.255 and every interface's subnet broadcast,
/// and collect replies for `wait`. Pass explicit mixer IPs (e.g. the remembered one) as extra targets.
pub fn discover(extra_targets: &[IpAddr], wait: Duration) -> Vec<(SocketAddr, MixerInfo)> {
    discover_report(extra_targets, wait).found
}

pub fn discover_report(extra_targets: &[IpAddr], wait: Duration) -> Discovery {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return Discovery { found: vec![], send_error: Some("cannot open a network socket".into()) };
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
    let msg = osc::encode("/xinfo", vec![]);
    let mut targets = vec![IpAddr::V4(Ipv4Addr::BROADCAST)];
    for t in interface_broadcasts().into_iter().chain(extra_targets.iter().copied()) {
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    let mut sent = 0;
    let mut first_err = None;
    for t in &targets {
        match sock.send_to(&msg, SocketAddr::new(*t, MIXER_PORT)) {
            Ok(_) => sent += 1,
            Err(e) => {
                first_err.get_or_insert(e.to_string());
            }
        }
    }
    let send_error = if sent == 0 { first_err } else { None };
    let mut found: Vec<(SocketAddr, MixerInfo)> = Vec::new();
    let end = Instant::now() + wait;
    let mut buf = [0u8; 1024];
    while Instant::now() < end {
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        for m in osc::decode(&buf[..n]) {
            if m.addr != "/xinfo" {
                continue;
            }
            let s = |i: usize| match m.args.get(i) {
                Some(OscType::String(s)) => s.clone(),
                _ => String::new(),
            };
            let info = MixerInfo { ip: s(0), name: s(1), model: s(2), firmware: s(3) };
            if !found.iter().any(|(a, _)| *a == from) {
                found.push((from, info));
            }
        }
    }
    Discovery { found, send_error }
}
