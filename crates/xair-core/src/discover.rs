//! Broadcast discovery of X Air mixers.

use crate::state::MixerInfo;
use rosc::OscType;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use xair_proto::{osc, MIXER_PORT};

/// Broadcast `/xinfo` to each target (plus 255.255.255.255) and collect replies for `wait`.
/// Pass subnet broadcast addresses (e.g. 10.5.255.255) for networks where the global
/// broadcast is filtered, or explicit mixer IPs.
pub fn discover(extra_targets: &[IpAddr], wait: Duration) -> Vec<(SocketAddr, MixerInfo)> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return vec![];
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
    let msg = osc::encode("/xinfo", vec![]);
    let mut targets = vec![IpAddr::V4(Ipv4Addr::BROADCAST)];
    targets.extend_from_slice(extra_targets);
    for t in &targets {
        let _ = sock.send_to(&msg, SocketAddr::new(*t, MIXER_PORT));
    }
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
    found
}
