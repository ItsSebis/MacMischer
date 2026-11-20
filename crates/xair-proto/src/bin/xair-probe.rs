//! xair-probe: tiny CLI to explore an X Air mixer's OSC interface (M1 protocol spike).
//!
//! Usage:
//!   xair-probe discover [broadcast-addr]
//!   xair-probe get <ip> <addr>...              query one or more addresses
//!   xair-probe set <ip> <addr> <f|i|s>:<value> set a value, then read it back
//!   xair-probe node <ip> <path>...             /node bulk query (e.g. "ch/01/mix")
//!   xair-probe listen <ip> <secs>              /xremote; print all pushes
//!   xair-probe meters <ip> <n> <secs>          subscribe /meters/<n>; decode blobs

use rosc::{decoder, encoder, OscMessage, OscPacket, OscType};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use xair_proto::MIXER_PORT;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("discover") => discover(args.get(1).map(String::as_str).unwrap_or("255.255.255.255")),
        Some("get") if args.len() >= 3 => get(&args[1], &args[2..]),
        Some("set") if args.len() == 4 => set(&args[1], &args[2], &args[3]),
        Some("node") if args.len() >= 3 => node(&args[1], &args[2..]),
        Some("listen") if args.len() == 3 => listen(&args[1], args[2].parse().unwrap_or(10)),
        Some("meters") if args.len() == 4 => meters(&args[1], &args[2], args[3].parse().unwrap_or(3)),
        _ => {
            eprintln!("see source header for usage");
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

type R = Result<(), Box<dyn std::error::Error>>;

fn sock() -> std::io::Result<UdpSocket> {
    let s = UdpSocket::bind("0.0.0.0:0")?;
    s.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(s)
}

fn target(ip: &str) -> SocketAddr {
    format!("{ip}:{MIXER_PORT}").parse().expect("bad ip")
}

fn send(s: &UdpSocket, to: SocketAddr, addr: &str, args: Vec<OscType>) -> std::io::Result<()> {
    let buf = encoder::encode(&OscPacket::Message(OscMessage { addr: addr.into(), args }))
        .expect("encode");
    s.send_to(&buf, to).map(|_| ())
}

/// Receive packets until `dur` elapsed, calling `f` for each message.
fn recv_for(s: &UdpSocket, dur: Duration, mut f: impl FnMut(SocketAddr, OscMessage)) {
    let end = Instant::now() + dur;
    let mut buf = [0u8; 4096];
    while Instant::now() < end {
        if let Ok((n, from)) = s.recv_from(&mut buf) {
            match decoder::decode_udp(&buf[..n]) {
                Ok((_, OscPacket::Message(m))) => f(from, m),
                Ok((_, OscPacket::Bundle(b))) => {
                    for p in b.content {
                        if let OscPacket::Message(m) = p {
                            f(from, m)
                        }
                    }
                }
                Err(e) => println!(
                    "<{from}> undecodable ({n} bytes, {e:?}): {:?}",
                    String::from_utf8_lossy(&buf[..n]).replace('\0', "\u{2400}")
                ),
            }
        }
    }
}

fn fmt_args(args: &[OscType]) -> String {
    args.iter()
        .map(|a| match a {
            OscType::Float(f) => format!("f:{f}"),
            OscType::Int(i) => format!("i:{i}"),
            OscType::String(s) => format!("s:{s:?}"),
            OscType::Blob(b) => format!("b:[{} bytes]", b.len()),
            o => format!("{o:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn discover(bcast: &str) -> R {
    let s = sock()?;
    s.set_broadcast(true)?;
    send(&s, target(bcast), "/xinfo", vec![])?;
    recv_for(&s, Duration::from_secs(2), |from, m| {
        println!("<{from}> {} {}", m.addr, fmt_args(&m.args))
    });
    Ok(())
}

fn get(ip: &str, addrs: &[String]) -> R {
    let s = sock()?;
    for a in addrs {
        send(&s, target(ip), a, vec![])?;
        recv_for(&s, Duration::from_millis(300), |_, m| {
            println!("{} {}", m.addr, fmt_args(&m.args))
        });
    }
    Ok(())
}

fn parse_val(v: &str) -> OscType {
    match v.split_once(':') {
        Some(("f", x)) => OscType::Float(x.parse().expect("float")),
        Some(("i", x)) => OscType::Int(x.parse().expect("int")),
        Some(("s", x)) => OscType::String(x.into()),
        _ => panic!("value must be f:/i:/s:"),
    }
}

fn set(ip: &str, addr: &str, val: &str) -> R {
    let s = sock()?;
    send(&s, target(ip), addr, vec![parse_val(val)])?;
    std::thread::sleep(Duration::from_millis(50));
    send(&s, target(ip), addr, vec![])?;
    recv_for(&s, Duration::from_millis(400), |_, m| {
        println!("{} {}", m.addr, fmt_args(&m.args))
    });
    Ok(())
}

fn node(ip: &str, paths: &[String]) -> R {
    let s = sock()?;
    for p in paths {
        send(&s, target(ip), "/node", vec![OscType::String(p.clone())])?;
        recv_for(&s, Duration::from_millis(400), |_, m| {
            println!("{} {}", m.addr, fmt_args(&m.args))
        });
    }
    Ok(())
}

fn listen(ip: &str, secs: u64) -> R {
    let s = sock()?;
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        send(&s, target(ip), "/xremote", vec![])?;
        recv_for(&s, Duration::from_secs(5).min(end - Instant::now()), |_, m| {
            println!("{} {}", m.addr, fmt_args(&m.args))
        });
    }
    Ok(())
}

fn meters(ip: &str, n: &str, secs: u64) -> R {
    let s = sock()?;
    let path = format!("/meters/{n}");
    send(&s, target(ip), "/meters", vec![OscType::String(path.clone())])?;
    let mut count = 0;
    let mut last = Instant::now();
    recv_for(&s, Duration::from_secs(secs), |_, m| {
        if m.addr != path {
            println!("{} {}", m.addr, fmt_args(&m.args));
            return;
        }
        count += 1;
        if let Some(OscType::Blob(b)) = m.args.first() {
            if count == 1 || last.elapsed() > Duration::from_secs(1) {
                last = Instant::now();
                // Blob layout (expected): i32 LE count, then count * i16 LE (1/256 dB)
                let cnt = i32::from_le_bytes(b[0..4].try_into().unwrap());
                let vals: Vec<String> = b[4..]
                    .chunks_exact(2)
                    .map(|c| format!("{:.0}", i16::from_le_bytes([c[0], c[1]]) as f32 / 256.0))
                    .collect();
                println!("{path} #{count} blob={}B count={cnt} n={} dB: {}", b.len(), vals.len(), vals.join(" "));
            }
        }
    });
    println!("received {count} meter packets in {secs}s");
    Ok(())
}
