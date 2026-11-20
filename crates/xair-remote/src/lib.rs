//! Built-in web remote: a small HTTP + WebSocket server (own thread, tokio) that lets
//! phones/tablets control selected strips. Protocol: docs/REMOTE.md.
//!
//! The app owns a [`Remote`]; it passes the current mixer with [`Remote::set_mixer`] and the
//! configuration with [`Remote::configure`]. Changing the port restarts the listener.

pub mod config;
mod session;

pub use config::{PageConfig, RemoteConfig, DEFAULT_PORT};

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use xair_core::Mixer;

const INDEX_HTML: &str = include_str!("../web/index.html");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon/icon-512.png");

/// State shared between the app and all connections.
pub(crate) struct Shared {
    pub mixer: RwLock<Option<Arc<Mixer>>>,
    pub config: RwLock<RemoteConfig>,
    /// Console lock: only mutes are accepted from remotes.
    pub locked: AtomicBool,
    pub clients: AtomicUsize,
    /// Bumped on config change so sessions re-send pages.
    pub config_gen: AtomicUsize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteStatus {
    Off,
    Running { port: u16 },
    Error(String),
}

struct Server {
    port: u16,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

pub struct Remote {
    shared: Arc<Shared>,
    rt: tokio::runtime::Runtime,
    server: Option<Server>,
    status: Arc<Mutex<RemoteStatus>>,
}

impl Default for Remote {
    fn default() -> Self {
        Self::new()
    }
}

impl Remote {
    pub fn new() -> Remote {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("xair-remote")
            .enable_all()
            .build()
            .expect("tokio runtime");
        Remote {
            shared: Arc::new(Shared {
                mixer: RwLock::new(None),
                config: RwLock::new(RemoteConfig::default()),
                locked: AtomicBool::new(false),
                clients: AtomicUsize::new(0),
                config_gen: AtomicUsize::new(0),
            }),
            rt,
            server: None,
            status: Arc::new(Mutex::new(RemoteStatus::Off)),
        }
    }

    pub fn set_mixer(&self, mixer: Option<Arc<Mixer>>) {
        *self.shared.mixer.write().unwrap() = mixer;
    }

    pub fn set_locked(&self, locked: bool) {
        self.shared.locked.store(locked, Ordering::Relaxed);
    }

    pub fn clients(&self) -> usize {
        self.shared.clients.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> RemoteStatus {
        self.status.lock().unwrap().clone()
    }

    /// Apply a configuration: starts/stops/restarts the server as needed. Cheap if unchanged.
    pub fn configure(&mut self, cfg: &RemoteConfig) {
        {
            let mut cur = self.shared.config.write().unwrap();
            if *cur != *cfg {
                *cur = cfg.clone();
                self.shared.config_gen.fetch_add(1, Ordering::Relaxed);
            }
        }
        let want = cfg.enabled.then_some(cfg.port);
        let have = self.server.as_ref().map(|s| s.port);
        if want == have && !matches!(self.status(), RemoteStatus::Error(_)) {
            return;
        }
        if let Some(s) = self.server.take() {
            let _ = s.shutdown.send(());
        }
        *self.status.lock().unwrap() = RemoteStatus::Off;
        if let Some(port) = want {
            self.start(port);
        }
    }

    fn start(&mut self, port: u16) {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let app = Router::new()
            .route("/", get(index))
            .route("/manifest.webmanifest", get(manifest))
            .route("/icon.png", get(icon))
            .route("/ws", get(ws))
            .with_state(self.shared.clone());
        let status = self.status.clone();
        // Bind synchronously so errors (port in use) show up immediately.
        let listener = self.rt.block_on(tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))));
        match listener {
            Ok(l) => {
                *status.lock().unwrap() = RemoteStatus::Running { port };
                tracing::info!("web remote on port {port}");
                self.rt.spawn(async move {
                    let r = axum::serve(l, app)
                        .with_graceful_shutdown(async {
                            let _ = rx.await;
                        })
                        .await;
                    if let Err(e) = r {
                        *status.lock().unwrap() = RemoteStatus::Error(e.to_string());
                    }
                });
                self.server = Some(Server { port, shutdown: tx });
            }
            Err(e) => {
                tracing::warn!("web remote: cannot listen on {port}: {e}");
                *status.lock().unwrap() = RemoteStatus::Error(format!("port {port}: {e}"));
            }
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        if let Some(s) = self.server.take() {
            let _ = s.shutdown.send(());
        }
    }
}

/// Addresses phones can use (IPv4, non-loopback). Container/VM bridges are hidden;
/// VPN interfaces (Tailscale etc.) come after the LAN ones.
pub fn lan_addresses() -> Vec<IpAddr> {
    const HIDDEN: [&str; 5] = ["docker", "br-", "veth", "virbr", "vmnet"];
    const VPN: [&str; 3] = ["tailscale", "utun", "wg"];
    let mut v: Vec<(bool, IpAddr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && i.ip().is_ipv4())
        .filter(|i| !HIDDEN.iter().any(|p| i.name.starts_with(p)))
        .map(|i| (VPN.iter().any(|p| i.name.starts_with(p)), i.ip()))
        .collect();
    v.sort_by_key(|(vpn, ip)| (*vpn, *ip));
    v.into_iter().map(|(_, ip)| ip).collect()
}

async fn index() -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], INDEX_HTML).into_response()
}

async fn manifest() -> Response {
    ([(header::CONTENT_TYPE, "application/manifest+json")], MANIFEST).into_response()
}

async fn icon() -> Response {
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "max-age=86400")], ICON_PNG).into_response()
}

/// Origin must match Host (blocks other web pages from driving the mixer via the user's browser).
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true; // non-browser clients
    };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    let origin_host = origin.split("://").nth(1).unwrap_or("");
    !host.is_empty() && origin_host.eq_ignore_ascii_case(host)
}

async fn ws(State(shared): State<Arc<Shared>>, headers: HeaderMap, up: WebSocketUpgrade) -> Response {
    if !origin_ok(&headers) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    up.max_message_size(4096).on_upgrade(move |sock| session::run(shared, sock))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn origin_check() {
        let mut h = HeaderMap::new();
        assert!(origin_ok(&h));
        h.insert(header::HOST, HeaderValue::from_static("10.5.5.199:8018"));
        h.insert(header::ORIGIN, HeaderValue::from_static("http://10.5.5.199:8018"));
        assert!(origin_ok(&h));
        h.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!origin_ok(&h));
    }
}
