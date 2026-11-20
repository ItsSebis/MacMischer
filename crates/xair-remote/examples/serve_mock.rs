//! Try the web remote without hardware: mock mixer + remote on port 8019.
//! `cargo run -p xair-remote --example serve_mock` then open http://localhost:8019/

use std::sync::Arc;
use xair_core::Mixer;
use xair_proto::{SendTo, StripId};
use xair_remote::{PageConfig, Remote, RemoteConfig};

fn main() {
    let mock = xair_mock::MockMixer::start();
    let mixer = Arc::new(Mixer::connect(mock.addr()));
    let mut remote = Remote::new();
    remote.set_mixer(Some(mixer));
    let pin = std::env::args().nth(1).unwrap_or_default();
    remote.configure(&RemoteConfig {
        enabled: true,
        port: 8019,
        pin,
        pages: vec![
            PageConfig { max_db: Some(4.0), ..PageConfig::new("Band", (1..=6).map(StripId::Ch).collect(), None) },
            PageConfig::new("Drummer monitor", vec![StripId::Ch(1), StripId::Ch(2), StripId::Ch(5), StripId::FxRtn(1)], Some(SendTo::Bus(2))),
            PageConfig::new("TV volume", vec![StripId::Dca(2)], None),
        ],
    });
    println!("web remote: http://localhost:8019/  ({:?})", remote.status());
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
