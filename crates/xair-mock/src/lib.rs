//! In-process fake XR18 for tests and offline development.

use rosc::OscType;
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use xair_proto::fader::to_db;
use xair_proto::osc::{decode, encode, from_osc, to_osc};
use xair_proto::strip::StripId;
use xair_proto::{meters, routing, Param, Value};

const SUB_TTL: Duration = Duration::from_secs(10);
const METER_PERIOD: Duration = Duration::from_millis(50);

#[derive(Default)]
struct State {
    values: HashMap<String, Value>,
    remote: HashMap<SocketAddr, Instant>,
    meters: HashMap<(SocketAddr, u8), Instant>,
}

struct Shared {
    state: Mutex<State>,
    sets: AtomicUsize,
    offline: AtomicBool,
    stop: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct MockMixer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    sock: UdpSocket,
    thread: Option<JoinHandle<()>>,
}

fn default_for(p: Param) -> Value {
    match p {
        Param::Fader(_) => Value::Float(0.75),
        Param::On(_) => Value::Int(1),
        Param::Pan(_) => Value::Float(0.5),
        Param::Name(StripId::Ch(n)) => Value::Str(format!("Ch {n:02}")),
        Param::Name(StripId::Bus(n)) => Value::Str(format!("Bus {n}")),
        Param::Name(StripId::Dca(n)) => Value::Str(format!("DCA {n}")),
        Param::Name(StripId::Main) => Value::Str("Main".into()),
        Param::Name(_) => Value::Str(String::new()),
        Param::Color(s) => Value::Int(match s {
            StripId::Ch(n) => i32::from(n) % 16,
            _ => 0,
        }),
        Param::DcaGroups(_) | Param::MuteGroups(_) | Param::MuteGroupOn(_) => Value::Int(0),
        Param::MonitorLevel => Value::Float(0.75),
        Param::ChLink(k) => Value::Int((k == 2) as i32), // ch 3-4 linked in the mock
        Param::BusLink(_) => Value::Int(0),
        Param::MainAssign(_) => Value::Int(1),
        Param::SendLevel(..) => Value::Float(0.0),
        Param::SendTap(..) => Value::Int(4),
        Param::Gain(_) => Value::Float(0.5),
        Param::Phantom(_) | Param::Solo(_) => Value::Int(0),
        Param::InSrc(StripId::Ch(n)) => Value::Int(n as i32 - 1),
        Param::InSrc(_) => Value::Int(0),
        Param::RtnSrc(StripId::Ch(16)) => Value::Int(1),
        Param::RtnSrc(_) | Param::RtnSw(_) => Value::Int(0),
        Param::RouteSrc(o) => Value::Int(match o {
            routing::Output::Main(n) => n as i32 - 1,
            routing::Output::Aux(n) => 25 + n as i32,
            routing::Output::Usb(n) | routing::Output::P16(n) => n as i32 - 1,
        }),
        Param::RoutePos(o) => Value::Int(match o {
            routing::Output::Aux(_) => 10,
            routing::Output::P16(_) => 3,
            _ => 0,
        }),
        Param::SnapIndex => Value::Int(1),
        Param::SnapName(1) => Value::Str("Home".into()),
        Param::SnapName(_) => Value::Str(String::new()),
    }
}

fn send(sock: &UdpSocket, to: SocketAddr, addr: &str, args: Vec<OscType>) {
    let _ = sock.send_to(&encode(addr, args), to);
}

fn level_db(st: &State, path: &str) -> f32 {
    let f = st.values.get(path).and_then(Value::as_f32).unwrap_or(0.0);
    to_db(f).max(meters::FLOOR_DB)
}

fn is_on(st: &State, p: Param) -> bool {
    st.values.get(&p.path()).and_then(Value::as_i32).unwrap_or(1) != 0
}

fn meter_values(st: &State, bank: u8, t: f32) -> Vec<f32> {
    let n = if bank == 1 { meters::m1::LEN } else { 36 };
    let mut out = vec![meters::FLOOR_DB; n];
    let wobble = |i: usize| 3.0 * (t * 2.0 + i as f32).sin();
    let strip_db = |s: StripId, i: usize| {
        if !is_on(st, Param::On(s)) {
            return meters::FLOOR_DB;
        }
        let f = Param::Fader(s).path();
        (level_db(st, &f) - 20.0 + wobble(i)).max(meters::FLOOR_DB)
    };
    for (i, o) in out.iter_mut().enumerate() {
        let s = match i {
            0..=15 => StripId::Ch(i as u8 + 1),
            _ => StripId::Main,
        };
        *o = strip_db(s, i);
    }
    out
}

fn handle(shared: &Shared, sock: &UdpSocket, from: SocketAddr, buf: &[u8]) {
    let now = Instant::now();
    for m in decode(buf) {
        let mut st = shared.lock();
        match (m.addr.as_str(), m.args.as_slice()) {
            ("/xinfo", []) => send(
                sock,
                from,
                "/xinfo",
                ["127.0.0.1", "MOCK-XR18", "XR18", "1.19"]
                    .map(|s| OscType::String(s.into()))
                    .to_vec(),
            ),
            ("/status", []) => send(
                sock,
                from,
                "/status",
                ["active", "127.0.0.1", "MOCK-XR18"].map(|s| OscType::String(s.into())).to_vec(),
            ),
            ("/xremote", _) => {
                st.remote.insert(from, now + SUB_TTL);
            }
            ("/meters", [OscType::String(s)]) => {
                if let Some(b @ (1 | 2)) = s.strip_prefix("/meters/").and_then(|n| n.parse::<u8>().ok()) {
                    st.meters.insert((from, b), now + SUB_TTL);
                }
            }
            ("/-snap/load", [a]) => {
                if let Some(Value::Int(n)) = from_osc(a) {
                    shared.sets.fetch_add(1, Ordering::SeqCst);
                    st.values.insert("/-snap/index".into(), Value::Int(n));
                }
            }
            (path, []) => {
                if let Some(v) = st.values.get(path) {
                    send(sock, from, path, vec![to_osc(v)]);
                }
            }
            (path, [a]) => {
                let Some(v) = from_osc(a) else { continue };
                shared.sets.fetch_add(1, Ordering::SeqCst);
                st.values.insert(path.to_string(), v);
                st.remote.retain(|_, exp| *exp > now);
                for peer in st.remote.keys().filter(|p| **p != from) {
                    send(sock, *peer, path, vec![a.clone()]);
                }
            }
            _ => {}
        }
    }
}

fn run(shared: Arc<Shared>, sock: UdpSocket) {
    let start = Instant::now();
    let mut next_meter = Instant::now();
    let mut buf = [0u8; 2048];
    while !shared.stop.load(Ordering::SeqCst) {
        let got = sock.recv_from(&mut buf);
        let offline = shared.offline.load(Ordering::SeqCst);
        if let Ok((n, from)) = got {
            if !offline {
                handle(&shared, &sock, from, &buf[..n]);
            }
        }
        let now = Instant::now();
        if now >= next_meter {
            next_meter = now + METER_PERIOD;
            if offline {
                continue;
            }
            let mut st = shared.lock();
            st.meters.retain(|_, exp| *exp > now);
            let t = start.elapsed().as_secs_f32();
            for &(addr, bank) in st.meters.keys() {
                let blob = meters::encode(&meter_values(&st, bank, t));
                send(&sock, addr, &format!("/meters/{bank}"), vec![OscType::Blob(blob)]);
            }
        }
    }
}

impl MockMixer {
    /// Bind 127.0.0.1:0, spawn the server thread, pre-populate state.
    pub fn start() -> MockMixer {
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind mock mixer socket");
        sock.set_read_timeout(Some(Duration::from_millis(5))).expect("set read timeout");
        let addr = sock.local_addr().expect("local addr");
        let mut st = State::default();
        for p in Param::sync_set() {
            st.values.insert(p.path(), default_for(p));
        }
        for i in 1..=18 {
            let p = Param::Solo(i);
            st.values.insert(p.path(), default_for(p));
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(st),
            sets: AtomicUsize::new(0),
            offline: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let tsock = sock.try_clone().expect("clone socket");
        let sh = shared.clone();
        let thread = std::thread::spawn(move || run(sh, tsock));
        MockMixer { addr, shared, sock, thread: Some(thread) }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Current value at an OSC path (e.g. "/ch/01/mix/fader").
    pub fn get(&self, path: &str) -> Option<Value> {
        self.shared.lock().values.get(path).cloned()
    }

    /// Simulate a change made elsewhere: store it and push it to all `/xremote` subscribers.
    pub fn console_set(&self, path: &str, v: Value) {
        let mut st = self.shared.lock();
        st.values.insert(path.to_string(), v.clone());
        let now = Instant::now();
        st.remote.retain(|_, exp| *exp > now);
        if self.shared.offline.load(Ordering::SeqCst) {
            return;
        }
        for peer in st.remote.keys() {
            send(&self.sock, *peer, path, vec![to_osc(&v)]);
        }
    }

    /// Number of OSC set messages (with args) received from clients so far.
    pub fn sets_received(&self) -> usize {
        self.shared.sets.load(Ordering::SeqCst)
    }

    /// Stop answering (simulate power-off) / resume.
    pub fn set_offline(&self, offline: bool) {
        self.shared.offline.store(offline, Ordering::SeqCst);
    }
}

impl Drop for MockMixer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(m: &MockMixer) -> UdpSocket {
        let c = UdpSocket::bind("127.0.0.1:0").unwrap();
        c.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        c.connect(m.addr()).unwrap();
        c
    }

    fn recv(c: &UdpSocket) -> Option<rosc::OscMessage> {
        let mut buf = [0u8; 2048];
        let n = c.recv(&mut buf).ok()?;
        decode(&buf[..n]).into_iter().next()
    }

    #[test]
    fn query_returns_value() {
        let m = MockMixer::start();
        let c = client(&m);
        c.send(&encode("/ch/01/mix/fader", vec![])).unwrap();
        let r = recv(&c).unwrap();
        assert_eq!(r.addr, "/ch/01/mix/fader");
        assert_eq!(r.args, vec![OscType::Float(0.75)]);
        c.send(&encode("/nope", vec![])).unwrap();
        assert!(recv(&c).is_none());
    }

    #[test]
    fn set_then_get() {
        let m = MockMixer::start();
        let c = client(&m);
        c.send(&encode("/ch/02/mix/on", vec![OscType::Int(0)])).unwrap();
        c.send(&encode("/ch/02/mix/on", vec![])).unwrap();
        assert_eq!(recv(&c).unwrap().args, vec![OscType::Int(0)]);
        assert_eq!(m.get("/ch/02/mix/on"), Some(Value::Int(0)));
        assert_eq!(m.sets_received(), 1);
    }

    #[test]
    fn console_set_and_peer_push() {
        let m = MockMixer::start();
        let a = client(&m);
        let b = client(&m);
        a.send(&encode("/xremote", vec![])).unwrap();
        b.send(&encode("/xremote", vec![])).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        m.console_set("/ch/03/mix/fader", Value::Float(0.1));
        assert_eq!(recv(&a).unwrap().args, vec![OscType::Float(0.1)]);
        assert_eq!(recv(&b).unwrap().args, vec![OscType::Float(0.1)]);
        // a's set reaches b but not a
        a.send(&encode("/ch/04/mix/fader", vec![OscType::Float(0.2)])).unwrap();
        let r = recv(&b).unwrap();
        assert_eq!(r.addr, "/ch/04/mix/fader");
        assert!(recv(&a).is_none());
    }

    #[test]
    fn meters_delivered() {
        let m = MockMixer::start();
        let c = client(&m);
        c.send(&encode("/meters", vec![OscType::String("/meters/1".into())])).unwrap();
        let r = recv(&c).unwrap();
        assert_eq!(r.addr, "/meters/1");
        let OscType::Blob(b) = &r.args[0] else { panic!("no blob") };
        assert_eq!(meters::parse(b).unwrap().len(), 40);
    }

    #[test]
    fn offline_drops_queries() {
        let m = MockMixer::start();
        let c = client(&m);
        m.set_offline(true);
        c.send(&encode("/xinfo", vec![])).unwrap();
        assert!(recv(&c).is_none());
        m.set_offline(false);
        c.send(&encode("/xinfo", vec![])).unwrap();
        assert_eq!(recv(&c).unwrap().addr, "/xinfo");
    }
}
