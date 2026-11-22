//! One WebSocket client: auth, page selection, state/meter push, validated commands.

use crate::config::PageConfig;
use crate::Shared;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use xair_core::{Mixer, Status};
use xair_proto::{fader, Param, StripId, Value};

const TICK: Duration = Duration::from_millis(50);
const METER_EVERY: u32 = 2; // every 2nd 50 ms tick → 10 Hz meters
const MAX_PIN_TRIES: u32 = 10;

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum In {
    Auth { pin: String },
    Open { page: usize },
    Level { strip: String, value: f32 },
    Mute { strip: String, muted: bool },
}

struct ClientGuard(Arc<Shared>);
impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) async fn run(shared: Arc<Shared>, sock: WebSocket) {
    shared.clients.fetch_add(1, Ordering::Relaxed);
    let _guard = ClientGuard(shared.clone());
    let (mut tx, mut rx) = sock.split();

    let mut s = Session { shared, authed: false, pin_tries: 0, page: None, last_state: None, gen: usize::MAX };
    let name = s.mixer().and_then(|m| m.state().info.as_ref().map(|i| i.name.clone())).unwrap_or_default();
    let pin_required = !s.pin().is_empty();
    s.authed = !pin_required;
    let mut out = vec![json!({"t":"hello","name":name,"pin":pin_required})];
    if s.authed {
        out.push(s.pages_msg());
        s.gen = s.shared.config_gen.load(Ordering::Relaxed);
    }

    let mut tick = tokio::time::interval(TICK);
    let mut n: u32 = 0;
    loop {
        for m in out.drain(..) {
            if tx.send(Message::Text(m.to_string().into())).await.is_err() {
                return;
            }
        }
        tokio::select! {
            msg = rx.next() => {
                let Some(Ok(msg)) = msg else { return };
                match msg {
                    Message::Text(t) => match serde_json::from_str::<In>(&t) {
                        Ok(m) => s.handle(m, &mut out),
                        Err(_) => out.push(json!({"t":"error","msg":"bad message"})),
                    },
                    Message::Close(_) => return,
                    _ => {}
                }
            }
            _ = tick.tick() => {
                n = n.wrapping_add(1);
                s.tick(n, &mut out);
            }
        }
    }
}

struct Session {
    shared: Arc<Shared>,
    authed: bool,
    pin_tries: u32,
    /// Index into config pages, plus a copy of that page.
    page: Option<(usize, PageConfig)>,
    last_state: Option<Json>,
    gen: usize,
}

impl Session {
    fn mixer(&self) -> Option<Arc<Mixer>> {
        self.shared.mixer.read().unwrap().clone()
    }
    fn pin(&self) -> String {
        self.shared.config.read().unwrap().pin.clone()
    }

    fn pages_msg(&self) -> Json {
        let cfg = self.shared.config.read().unwrap();
        let pages: Vec<Json> = cfg.pages.iter().map(|p| json!({"name": p.name})).collect();
        json!({"t":"pages","pages":pages})
    }

    fn handle(&mut self, m: In, out: &mut Vec<Json>) {
        match m {
            In::Auth { pin } => {
                if self.authed {
                    return;
                }
                if self.pin_tries >= MAX_PIN_TRIES {
                    out.push(json!({"t":"error","msg":"too many attempts"}));
                    return;
                }
                if pin == self.pin() {
                    self.authed = true;
                    out.push(json!({"t":"auth","ok":true}));
                    out.push(self.pages_msg());
                    self.gen = self.shared.config_gen.load(Ordering::Relaxed);
                } else {
                    self.pin_tries += 1;
                    out.push(json!({"t":"auth","ok":false}));
                }
            }
            _ if !self.authed => out.push(json!({"t":"error","msg":"PIN required"})),
            In::Open { page } => {
                let p = self.shared.config.read().unwrap().pages.get(page).cloned();
                match p {
                    Some(p) => {
                        self.page = Some((page, p));
                        self.last_state = None;
                        out.push(self.page_msg());
                        self.push_state(out);
                    }
                    None => out.push(json!({"t":"error","msg":"no such page"})),
                }
            }
            In::Level { strip, value } => {
                let Some((s, page)) = self.page_strip(&strip) else {
                    out.push(json!({"t":"error","msg":"strip not on this page"}));
                    return;
                };
                let p = level_param(s, &page);
                if !self.write_allowed(p, out) {
                    self.last_state = None; // re-send the real value
                    return;
                }
                if !value.is_finite() {
                    return;
                }
                let v = fader::quantize(value.clamp(0.0, page.max_value()));
                if let Some(m) = self.mixer() {
                    m.set(p, Value::Float(v));
                }
            }
            In::Mute { strip, muted } => {
                let Some((s, _)) = self.page_strip(&strip) else {
                    out.push(json!({"t":"error","msg":"strip not on this page"}));
                    return;
                };
                let p = Param::On(s);
                if !self.write_allowed(p, out) {
                    return;
                }
                if let Some(m) = self.mixer() {
                    m.set(p, Value::Int(!muted as i32));
                }
            }
        }
    }

    /// The Lock applies to the remote exactly as to the app (`Param::allowed_when_locked`).
    fn write_allowed(&self, p: Param, out: &mut Vec<Json>) -> bool {
        let ok = !self.shared.locked.load(Ordering::Relaxed) || p.allowed_when_locked();
        if !ok {
            out.push(json!({"t":"error","msg":"Console is locked"}));
        }
        ok
    }

    /// Resolve a strip id the client sent, only if it is on the open page.
    fn page_strip(&self, id: &str) -> Option<(StripId, PageConfig)> {
        let (_, page) = self.page.as_ref()?;
        let s = StripId::from_short(id)?;
        page.strips.contains(&s).then(|| (s, page.clone()))
    }

    fn page_msg(&self) -> Json {
        let Some((i, page)) = &self.page else { return json!({}) };
        let m = self.mixer();
        let st = m.as_ref().map(|m| m.state());
        let strips: Vec<Json> = page
            .strips
            .iter()
            .map(|s| {
                let (name, color, stereo) = match &st {
                    Some(st) => (st.name(*s), st.color(*s), st.linked_partner(*s).is_some()),
                    None => (s.to_string(), 0, false),
                };
                json!({"id": s.short(), "name": name, "color": color_hex(color), "stereo": stereo})
            })
            .collect();
        let bus = page.bus.map(|t| match &st {
            Some(st) => st.name(t.strip()),
            None => t.strip().to_string(),
        });
        json!({"t":"page","index":i,"name":page.name,"bus":bus,"max":page.max_value(),"meters":page.meters,"strips":strips})
    }

    fn push_state(&mut self, out: &mut Vec<Json>) {
        let Some((_, page)) = &self.page else { return };
        let m = self.mixer();
        let connected = m.as_ref().is_some_and(|m| m.status() == Status::Connected);
        let mut strips = BTreeMap::new();
        if let Some(m) = &m {
            let st = m.state();
            for s in &page.strips {
                let v = st.get(level_param(*s, page)).and_then(Value::as_f32).unwrap_or(0.0);
                let db = fader::to_db(v);
                let db = if db < -89.5 { "-inf".to_string() } else { format!("{db:.1}") };
                strips.insert(s.short(), json!({"value": v, "db": db, "muted": !st.on(*s)}));
            }
        }
        let msg = json!({"t":"state","connected":connected,"strips":strips});
        if self.last_state.as_ref() != Some(&msg) {
            out.push(msg.clone());
            self.last_state = Some(msg);
        }
    }

    fn tick(&mut self, n: u32, out: &mut Vec<Json>) {
        if !self.authed {
            return;
        }
        // Config changed in the app → re-send page list; refresh or drop the open page.
        let gen = self.shared.config_gen.load(Ordering::Relaxed);
        if gen != self.gen {
            self.gen = gen;
            out.push(self.pages_msg());
            if let Some((i, _)) = self.page.take() {
                if let Some(p) = self.shared.config.read().unwrap().pages.get(i).cloned() {
                    self.page = Some((i, p));
                }
                if self.page.is_some() {
                    out.push(self.page_msg());
                    self.last_state = None;
                }
            }
        }
        self.push_state(out);
        if n.is_multiple_of(METER_EVERY) {
            if let (Some((_, page)), Some(m)) = (&self.page, self.mixer()) {
                if page.meters {
                    let mt = m.meters();
                    let vals: BTreeMap<String, f32> = page
                        .strips
                        .iter()
                        .filter_map(|s| mt.strip(*s).map(|db| (s.short(), (db * 10.0).round() / 10.0)))
                        .collect();
                    out.push(json!({"t":"meters","m":vals}));
                }
            }
        }
    }
}

/// Fader, or the send to the page's bus for input strips.
fn level_param(s: StripId, page: &PageConfig) -> Param {
    Param::level(s, page.bus)
}

fn color_hex(code: i32) -> &'static str {
    match code.rem_euclid(8) {
        1 => "#e84a4a",
        2 => "#4ccf5e",
        3 => "#e8d23c",
        4 => "#4a7df0",
        5 => "#d05ce0",
        6 => "#3cd0d8",
        7 => "#e0e0e0",
        _ => "#5a5d64",
    }
}
