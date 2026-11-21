//! X Air mixer state mirror + network connection.
//!
//! [`Mixer::connect`] spawns a network thread. The UI reads [`Mixer::state`] / [`Mixer::meters`]
//! every frame and writes through [`Mixer::set`] (optimistic: local state updates immediately).

mod conn;
mod discover;
mod state;

pub use conn::Timing;
pub use discover::discover;
pub use state::{Meters, MixerInfo, MixerState, Status};

use conn::Cmd;
use crossbeam_channel::Sender;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};
use std::thread::JoinHandle;
use xair_proto::{Param, Value};

pub(crate) struct Shared {
    pub state: RwLock<MixerState>,
    pub meters: Mutex<Meters>,
    pub status: Mutex<Status>,
    /// Incremented on every parameter change (cheap change detection for the UI).
    pub version: AtomicU64,
    /// Last value sent via `Mixer::set_rta_wanted` (to send commands only on change).
    rta_wanted: AtomicBool,
    repaint: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Shared {
    pub fn repaint(&self) {
        if let Some(f) = self.repaint.lock().unwrap().as_ref() {
            f();
        }
    }
    pub fn bump(&self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }
}

/// Handle to one mixer connection. Dropping it stops the network thread.
pub struct Mixer {
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    addr: SocketAddr,
}

impl Mixer {
    pub fn connect(addr: SocketAddr) -> Mixer {
        Self::connect_with(addr, Timing::default())
    }

    pub fn connect_with(addr: SocketAddr, timing: Timing) -> Mixer {
        let shared = Arc::new(Shared {
            state: RwLock::new(MixerState::default()),
            meters: Mutex::new(Meters::default()),
            status: Mutex::new(Status::Connecting),
            version: AtomicU64::new(0),
            rta_wanted: AtomicBool::new(false),
            repaint: Mutex::new(None),
        });
        let (tx, rx) = crossbeam_channel::unbounded();
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("xair-conn".into())
            .spawn(move || conn::run(addr, sh, rx, timing))
            .expect("spawn network thread");
        Mixer { shared, tx, thread: Some(thread), addr }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Called from the network thread whenever something visible changed.
    pub fn set_repaint(&self, f: impl Fn() + Send + 'static) {
        *self.shared.repaint.lock().unwrap() = Some(Box::new(f));
    }

    pub fn state(&self) -> RwLockReadGuard<'_, MixerState> {
        self.shared.state.read().unwrap()
    }

    pub fn meters(&self) -> MutexGuard<'_, Meters> {
        self.shared.meters.lock().unwrap()
    }

    pub fn status(&self) -> Status {
        *self.shared.status.lock().unwrap()
    }

    pub fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Relaxed)
    }

    /// Set a parameter: updates local state immediately, sends coalesced (latest wins).
    /// For stereo-linked strips the partner is updated too (the mixer mirrors linked
    /// faders/mutes itself but does not echo the partner change back to us).
    pub fn set(&self, p: Param, v: Value) {
        let mirror = {
            let mut st = self.shared.state.write().unwrap();
            st.values.insert(p, v.clone());
            let m = p.strip().and_then(|s| st.linked_partner(s)).and_then(|o| p.with_strip(o));
            if let Some(m) = m {
                st.values.insert(m, v.clone());
            }
            m
        };
        self.shared.bump();
        let _ = self.tx.send(Cmd::Set(p, v.clone()));
        if let Some(m) = mirror {
            let _ = self.tx.send(Cmd::Set(m, v));
        }
    }

    /// Send an arbitrary OSC message (e.g. `/-snap/load`).
    pub fn send_raw(&self, path: &str, args: Vec<Value>) {
        let _ = self.tx.send(Cmd::Raw(path.into(), args));
    }

    /// Subscribe to / unsubscribe from the RTA spectrum (`/meters/4`). Off by default: not
    /// subscribed unless wanted. Turning it off sends no unsubscribe; renewals just stop, so the
    /// mixer stops streaming when the subscription lapses (~10 s) and the client discards the
    /// data meanwhile. Cheap to call every frame.
    pub fn set_rta_wanted(&self, on: bool) {
        if self.shared.rta_wanted.swap(on, Ordering::Relaxed) != on {
            let _ = self.tx.send(Cmd::Rta(on));
        }
    }

    /// Re-read the full state (e.g. after a snapshot recall).
    pub fn resync(&self) {
        let _ = self.tx.send(Cmd::Resync);
    }

    /// Recall a snapshot (1..64) and re-read state.
    pub fn load_snapshot(&self, index: u8) {
        self.send_raw("/-snap/load", vec![Value::Int(index as i32)]);
        self.resync();
    }

    /// Save the current mix into snapshot slot (1..64).
    pub fn save_snapshot(&self, index: u8) {
        self.send_raw("/-snap/save", vec![Value::Int(index as i32)]);
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
