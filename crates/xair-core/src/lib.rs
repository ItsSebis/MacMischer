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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};
use std::thread::JoinHandle;
use xair_proto::{Param, Value};

pub(crate) struct Shared {
    pub state: RwLock<MixerState>,
    pub meters: Mutex<Meters>,
    pub status: Mutex<Status>,
    /// Incremented on every parameter change (cheap change detection for the UI).
    pub version: AtomicU64,
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
    pub fn set(&self, p: Param, v: Value) {
        self.shared.state.write().unwrap().values.insert(p, v.clone());
        self.shared.bump();
        let _ = self.tx.send(Cmd::Set(p, v));
    }

    /// Send an arbitrary OSC message (e.g. `/-snap/load`).
    pub fn send_raw(&self, path: &str, args: Vec<Value>) {
        let _ = self.tx.send(Cmd::Raw(path.into(), args));
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
