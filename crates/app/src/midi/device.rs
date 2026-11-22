//! Real MIDI input via midir (the `midi` feature).

use super::*;
use std::time::{Duration, Instant};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};

const POLL: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Shared {
    enabled: bool,
    watch: bool,
    device: Option<String>,
    ports: Vec<String>,
    status: Option<MidiStatus>,
    stop: bool,
}

type Sync_ = Arc<(Mutex<Shared>, Condvar)>;

/// Lock that survives a poisoned mutex (a crashed worker must not take the UI down).
fn lock(s: &Sync_) -> std::sync::MutexGuard<'_, Shared> {
    s.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// List input port names. Errors (no CoreMIDI/ALSA access, init failure) are returned, never panic.
pub fn list_ports() -> Result<Vec<String>, String> {
    let input = midir::MidiInput::new("MacMischer list").map_err(|e| e.to_string())?;
    Ok(input.ports().iter().filter_map(|p| input.port_name(p).ok()).collect())
}

fn connect(name: &str, tx: SyncSender<(Instant, MidiMsg)>, ctx: eframe::egui::Context) -> Result<midir::MidiInputConnection<()>, String> {
    let input = midir::MidiInput::new("MacMischer").map_err(|e| e.to_string())?;
    let port = input
        .ports()
        .into_iter()
        .find(|p| input.port_name(p).is_ok_and(|n| n == name))
        .ok_or_else(|| "device disappeared".to_string())?;
    input
        .connect(
            &port,
            "MacMischer in",
            move |_, bytes, _| {
                if let Some(m) = MidiMsg::parse(bytes) {
                    // Bounded: if the UI stalls, excess messages are dropped, not queued forever.
                    if tx.try_send((Instant::now(), m)).is_ok() {
                        ctx.request_repaint();
                    }
                }
            },
            (),
        )
        .map_err(|e| e.to_string())
}

pub struct MidiService {
    shared: Sync_,
    rx: Receiver<(Instant, MidiMsg)>,
}

impl MidiService {
    pub fn new(ctx: eframe::egui::Context) -> MidiService {
        let shared: Sync_ = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let (tx, rx) = sync_channel(256);
        let sh = shared.clone();
        let spawned = std::thread::Builder::new().name("midi".into()).spawn(move || {
            let sh2 = sh.clone();
            let ctx2 = ctx.clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker(sh, tx, ctx)));
            if r.is_err() {
                tracing::error!("MIDI thread panicked");
                lock(&sh2).status = Some(MidiStatus::Error("MIDI thread crashed; restart the app".into()));
                ctx2.request_repaint();
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("cannot start MIDI thread: {e}");
            lock(&shared).status = Some(MidiStatus::Error(e.to_string()));
        }
        MidiService { shared, rx }
    }

    /// Cheap; wakes the poller only when something changed. `watch` = list ports even while disabled.
    pub fn configure(&self, enabled: bool, device: Option<&str>, watch: bool) {
        let cv = &self.shared.1;
        let mut s = lock(&self.shared);
        if s.enabled != enabled || s.watch != watch || s.device.as_deref() != device {
            s.enabled = enabled;
            s.watch = watch;
            s.device = device.map(str::to_string);
            cv.notify_all();
        }
    }

    pub fn drain(&self) -> Vec<MidiMsg> {
        fresh(self.rx.try_iter().collect(), Instant::now(), MAX_AGE)
    }

    pub fn ports(&self) -> Vec<String> {
        lock(&self.shared).ports.clone()
    }

    pub fn status(&self) -> MidiStatus {
        let s = lock(&self.shared);
        match &s.status {
            Some(st) => st.clone(),
            None => MidiStatus::Off,
        }
    }

    pub fn available() -> bool {
        true
    }
}

impl Drop for MidiService {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        if let Ok(mut s) = m.lock() {
            s.stop = true;
        }
        cv.notify_all();
    }
}

fn worker(shared: Sync_, tx: SyncSender<(Instant, MidiMsg)>, ctx: eframe::egui::Context) {
    let (m, cv) = &*shared;
    let mut conn: Option<(String, midir::MidiInputConnection<()>)> = None;
    loop {
        let (enabled, watch, device) = {
            let s = m.lock().unwrap_or_else(|e| e.into_inner());
            if s.stop {
                return;
            }
            (s.enabled, s.watch, s.device.clone())
        };
        let mut status;
        let mut ports = Vec::new();
        if !enabled && !watch {
            conn = None;
            status = MidiStatus::Off;
        } else {
            match list_ports() {
                Err(e) => {
                    conn = None;
                    status = MidiStatus::Error(e);
                }
                Ok(p) => {
                    ports = p;
                    let wanted = device.as_deref().filter(|_| enabled);
                    status = if enabled { MidiStatus::NoDeviceChosen } else { MidiStatus::Off };
                    // Two rounds: a disconnect may be followed by an immediate (re)connect.
                    for _ in 0..2 {
                        match plan(wanted, &ports, conn.as_ref().map(|(n, _)| n.as_str())) {
                            Plan::Keep | Plan::Idle => break,
                            Plan::Disconnect => conn = None,
                            Plan::Connect(name) => {
                                match connect(&name, tx.clone(), ctx.clone()) {
                                    Ok(c) => conn = Some((name, c)),
                                    Err(e) => status = MidiStatus::Error(e),
                                }
                                break;
                            }
                        }
                    }
                    if enabled {
                        if let Some((n, _)) = &conn {
                            status = MidiStatus::Connected(n.clone());
                        } else if let Some(d) = wanted {
                            if !matches!(status, MidiStatus::Error(_)) {
                                status = MidiStatus::Waiting(d.to_string());
                            }
                        }
                    }
                }
            }
        }
        ctx.request_repaint();
        let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
        s.ports = ports;
        s.status = Some(status);
        // Sleep until something changes or the next poll.
        if !s.stop {
            let (g, _) = cv.wait_timeout(s, POLL).unwrap_or_else(|e| e.into_inner());
            if g.stop {
                return;
            }
        } else {
            return;
        }
    }
}
