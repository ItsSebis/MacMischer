//! Web remote end-to-end: WebSocket client ↔ Remote server ↔ xair-core ↔ mock mixer.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value as Json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use xair_core::{Mixer, Status};
use xair_mock::MockMixer;
use xair_proto::{SendTo, StripId, Value};
use xair_remote::{PageConfig, Remote, RemoteConfig};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn wait(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timeout: {what}");
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_of(ws: &mut Ws, t: &str) -> Json {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(3), ws.next()).await.expect("timeout").unwrap().unwrap();
        if let Message::Text(s) = m {
            let v: Json = serde_json::from_str(&s).unwrap();
            if v["t"] == t {
                return v;
            }
        }
    }
}

async fn send(ws: &mut Ws, v: Json) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

fn setup(pin: &str) -> (MockMixer, Arc<Mixer>, Remote, u16) {
    let mock = MockMixer::start();
    let mixer = Arc::new(Mixer::connect(mock.addr()));
    wait("mixer sync", || mixer.status() == Status::Connected);
    let mut remote = Remote::new();
    remote.set_mixer(Some(mixer.clone()));
    let port = free_port();
    remote.configure(&RemoteConfig {
        enabled: true,
        port,
        pin: pin.into(),
        pages: vec![
            PageConfig { max_db: Some(0.0), ..PageConfig::new("Main", vec![StripId::Ch(1), StripId::Ch(2)], None) },
            PageConfig::new("Mon", vec![StripId::Ch(1)], Some(SendTo::Bus(2))),
        ],
    });
    (mock, mixer, remote, port)
}

#[test]
fn levels_mutes_and_caps() {
    let (mock, _mixer, _remote, port) = setup("");
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();
        let hello = next_of(&mut ws, "hello").await;
        assert_eq!(hello["pin"], false);
        let pages = next_of(&mut ws, "pages").await;
        assert_eq!(pages["pages"][1]["name"], "Mon");

        send(&mut ws, json!({"t":"open","page":0})).await;
        let page = next_of(&mut ws, "page").await;
        assert_eq!(page["strips"][0]["id"], "1");
        let st = next_of(&mut ws, "state").await;
        assert_eq!(st["connected"], true);

        // Level above the 0 dB cap is clamped.
        send(&mut ws, json!({"t":"level","strip":"1","value":1.0})).await;
        // Strip not on the page is rejected.
        send(&mut ws, json!({"t":"level","strip":"5","value":0.1})).await;
        assert_eq!(next_of(&mut ws, "error").await["msg"], "strip not on this page");
        send(&mut ws, json!({"t":"mute","strip":"2","muted":true})).await;

        // Monitor page drives the bus send, not the fader.
        send(&mut ws, json!({"t":"open","page":1})).await;
        next_of(&mut ws, "page").await;
        send(&mut ws, json!({"t":"level","strip":"1","value":0.5})).await;
    });
    wait("fader capped at unity", || matches!(mock.get("/ch/01/mix/fader"), Some(Value::Float(f)) if (f - 767.0 / 1023.0).abs() < 1e-4));
    wait("mute", || mock.get("/ch/02/mix/on") == Some(Value::Int(0)));
    wait("send", || matches!(mock.get("/ch/01/mix/02/level"), Some(Value::Float(f)) if (f - 0.5).abs() < 0.01));
    assert!(mock.get("/ch/05/mix/fader") == Some(Value::Float(0.75)), "untouched");
}

#[test]
fn pin_and_push() {
    let (mock, _mixer, _remote, port) = setup("4711");
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();
        assert_eq!(next_of(&mut ws, "hello").await["pin"], true);
        send(&mut ws, json!({"t":"open","page":0})).await;
        assert_eq!(next_of(&mut ws, "error").await["msg"], "PIN required");
        send(&mut ws, json!({"t":"auth","pin":"0000"})).await;
        assert_eq!(next_of(&mut ws, "auth").await["ok"], false);
        send(&mut ws, json!({"t":"auth","pin":"4711"})).await;
        assert_eq!(next_of(&mut ws, "auth").await["ok"], true);
        send(&mut ws, json!({"t":"open","page":0})).await;
        next_of(&mut ws, "state").await;
        // A change made on the console is pushed to the phone.
        mock.console_set("/ch/02/mix/on", Value::Int(0));
        loop {
            let st = next_of(&mut ws, "state").await;
            if st["strips"]["2"]["muted"] == true {
                break;
            }
        }
    });
}

#[test]
fn rejects_foreign_origin() {
    let (_mock, _mixer, _remote, port) = setup("");
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
        req.headers_mut().insert("Origin", "https://evil.example".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(req).await.is_err());
    });
}
