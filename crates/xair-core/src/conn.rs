//! Network thread: discovery handshake, full sync, keepalives, coalesced sends, reconnect.

use crate::state::{MixerInfo, Status};
use crate::Shared;
use crossbeam_channel::{Receiver, TryRecvError};
use rosc::{OscMessage, OscType};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};
use xair_proto::{meters, osc, Param, Value};

pub(crate) enum Cmd {
    Set(Param, Value),
    Raw(String, Vec<Value>),
    Resync,
    Shutdown,
}

/// Tunables (overridable in tests).
#[derive(Debug, Clone)]
pub struct Timing {
    pub keepalive: Duration,
    pub flush: Duration,
    pub lost_after: Duration,
    pub hello_every: Duration,
    /// Re-query an unanswered param after this long (max 3 retries).
    pub sync_retry: Duration,
    /// Max queries in flight during sync.
    pub sync_window: usize,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            keepalive: Duration::from_secs(8),
            flush: Duration::from_millis(15),
            lost_after: Duration::from_secs(3),
            hello_every: Duration::from_secs(1),
            sync_retry: Duration::from_millis(250),
            sync_window: 24,
        }
    }
}

struct Conn {
    sock: UdpSocket,
    to: SocketAddr,
    shared: Arc<Shared>,
    timing: Timing,
    connected: bool,
    last_rx: Instant,
    last_hello: Option<Instant>,
    last_keepalive: Option<Instant>,
    last_flush: Instant,
    pending: HashMap<Param, Value>,
    /// Params still to query, and those queried but not yet answered.
    sync_queue: Vec<Param>,
    /// In flight: param -> (last sent, retries).
    sync_waiting: HashMap<Param, (Instant, u32)>,
    sync_total: usize,
}

pub(crate) fn run(to: SocketAddr, shared: Arc<Shared>, rx: Receiver<Cmd>, timing: Timing) {
    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("bind failed: {e}");
            return;
        }
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(5)));
    let now = Instant::now();
    let mut c = Conn {
        sock,
        to,
        shared,
        timing,
        connected: false,
        last_rx: now,
        last_hello: None,
        last_keepalive: None,
        last_flush: now,
        pending: HashMap::new(),
        sync_queue: Vec::new(),
        sync_waiting: HashMap::new(),
        sync_total: 0,
    };
    c.set_status(Status::Connecting);
    let mut buf = vec![0u8; 8192];
    loop {
        loop {
            match rx.try_recv() {
                Ok(Cmd::Set(p, v)) => {
                    c.pending.insert(p, v);
                }
                Ok(Cmd::Raw(path, args)) => {
                    let args = args.iter().map(osc::to_osc).collect();
                    c.send(&osc::encode(&path, args));
                }
                Ok(Cmd::Resync) => c.start_sync(),
                Ok(Cmd::Shutdown) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }
        // Drain socket (first recv blocks up to the read timeout).
        while let Ok((n, _)) = c.sock.recv_from(&mut buf) {
            for m in osc::decode(&buf[..n]) {
                c.handle(m);
            }
            let _ = c.sock.set_nonblocking(true);
        }
        let _ = c.sock.set_nonblocking(false);
        c.tick();
    }
}

impl Conn {
    fn send(&self, bytes: &[u8]) {
        if let Err(e) = self.sock.send_to(bytes, self.to) {
            tracing::debug!("send failed: {e}");
        }
    }

    fn set_status(&self, s: Status) {
        *self.shared.status.lock().unwrap() = s;
        self.shared.repaint();
    }

    fn start_sync(&mut self) {
        self.sync_queue = Param::sync_set();
        self.sync_queue.reverse(); // pop from the end in console order
        self.sync_total = self.sync_queue.len();
        self.sync_waiting.clear();
        self.set_status(Status::Syncing { done: 0, total: self.sync_total });
    }

    fn handle(&mut self, m: OscMessage) {
        self.last_rx = Instant::now();
        if !self.connected {
            if m.addr == "/xinfo" {
                let s = |i: usize| match m.args.get(i) {
                    Some(OscType::String(s)) => s.clone(),
                    _ => String::new(),
                };
                let info = MixerInfo { ip: s(0), name: s(1), model: s(2), firmware: s(3) };
                tracing::info!("connected to {} ({} fw {})", info.name, info.model, info.firmware);
                self.shared.state.write().unwrap().info = Some(info);
                self.connected = true;
                self.last_keepalive = None;
                self.start_sync();
            }
            return;
        }
        if let Some(n) = m.addr.strip_prefix("/meters/") {
            if let Some(OscType::Blob(b)) = m.args.first() {
                if let Some(vals) = meters::parse(b) {
                    let mut mt = self.shared.meters.lock().unwrap();
                    match n {
                        "1" => mt.m1 = vals,
                        "2" => mt.m2 = vals,
                        _ => {}
                    }
                    mt.updated = Some(Instant::now());
                    drop(mt);
                    self.shared.repaint();
                }
            }
            return;
        }
        if let Some((p, v)) = osc::as_param_update(&m) {
            let was_waiting = self.sync_waiting.remove(&p).is_some();
            // Don't let a stale reply overwrite a local change still queued for sending.
            if !self.pending.contains_key(&p) {
                self.shared.state.write().unwrap().values.insert(p, v);
                self.shared.bump();
            }
            if was_waiting {
                self.update_sync_status();
            }
            self.shared.repaint();
        }
    }

    fn update_sync_status(&mut self) {
        if self.sync_total == 0 {
            return;
        }
        if self.sync_queue.is_empty() && self.sync_waiting.is_empty() {
            self.sync_total = 0;
            self.set_status(Status::Connected);
        } else {
            let done = self.sync_total - self.sync_queue.len() - self.sync_waiting.len();
            self.set_status(Status::Syncing { done, total: self.sync_total });
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if !self.connected {
            if self.last_hello.is_none_or(|t| now - t >= self.timing.hello_every) {
                self.send(&osc::encode("/xinfo", vec![]));
                self.last_hello = Some(now);
            }
            return;
        }
        if now - self.last_rx > self.timing.lost_after {
            tracing::warn!("mixer stopped answering; reconnecting");
            self.connected = false;
            self.sync_total = 0;
            self.set_status(Status::Lost);
            return;
        }
        if self.last_keepalive.is_none_or(|t| now - t >= self.timing.keepalive) {
            self.send(&osc::encode("/xremote", vec![]));
            for m in ["/meters/1", "/meters/2"] {
                self.send(&osc::encode("/meters", vec![OscType::String(m.into())]));
            }
            self.last_keepalive = Some(now);
        }
        if !self.pending.is_empty() && now - self.last_flush >= self.timing.flush {
            for (p, v) in self.pending.drain() {
                let bytes = osc::encode_set(p, &v);
                if let Err(e) = self.sock.send_to(&bytes, self.to) {
                    tracing::debug!("send failed: {e}");
                }
            }
            self.last_flush = now;
        }
        if self.sync_total > 0 {
            self.sync_step(now);
        }
    }

    /// Sliding-window sync: keep `sync_window` queries in flight, retry stragglers.
    fn sync_step(&mut self, now: Instant) {
        let mut gave_up = 0;
        let mut resend = Vec::new();
        self.sync_waiting.retain(|p, (sent, retries)| {
            if now - *sent < self.timing.sync_retry {
                return true;
            }
            if *retries >= 3 {
                gave_up += 1;
                return false;
            }
            *retries += 1;
            *sent = now;
            resend.push(*p);
            true
        });
        for p in resend {
            self.send(&osc::encode_query(p));
        }
        while self.sync_waiting.len() < self.timing.sync_window {
            let Some(p) = self.sync_queue.pop() else { break };
            self.send(&osc::encode_query(p));
            self.sync_waiting.insert(p, (now, 0));
        }
        if gave_up > 0 {
            tracing::warn!("{gave_up} params unanswered after retries");
            self.update_sync_status();
        }
    }
}
