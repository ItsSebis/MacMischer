//! xair-core against the in-process mock mixer.

use std::time::{Duration, Instant};
use xair_core::{Mixer, Status, Timing};
use xair_mock::MockMixer;
use xair_proto::{Param, StripId, Value};

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn fast() -> Timing {
    Timing {
        lost_after: Duration::from_millis(500),
        hello_every: Duration::from_millis(100),
        ..Timing::default()
    }
}

#[test]
fn syncs_full_state() {
    let mock = MockMixer::start();
    let mixer = Mixer::connect_with(mock.addr(), fast());
    wait_for("connected", 5, || mixer.status() == Status::Connected);
    let st = mixer.state();
    assert_eq!(st.info.as_ref().unwrap().name, "MOCK-XR18");
    assert_eq!(st.values.len(), Param::sync_set().len());
    assert_eq!(st.fader(StripId::Ch(1)), 0.75);
}

#[test]
fn sets_are_sent_and_coalesced() {
    let mock = MockMixer::start();
    let mixer = Mixer::connect_with(mock.addr(), fast());
    wait_for("connected", 5, || mixer.status() == Status::Connected);
    let p = Param::Fader(StripId::Ch(3));
    for i in 0..100 {
        mixer.set(p, Value::Float(i as f32 / 100.0));
    }
    assert_eq!(mixer.state().fader(StripId::Ch(3)), 0.99); // optimistic
    wait_for("mock has final value", 2, || mock.get(&p.path()) == Some(Value::Float(0.99)));
    assert!(mock.sets_received() < 20, "not coalesced: {}", mock.sets_received());
}

#[test]
fn receives_pushes_and_meters() {
    let mock = MockMixer::start();
    let mixer = Mixer::connect_with(mock.addr(), fast());
    wait_for("connected", 5, || mixer.status() == Status::Connected);
    mock.console_set("/ch/05/mix/on", Value::Int(0));
    wait_for("push applied", 2, || !mixer.state().on(StripId::Ch(5)));
    wait_for("meters", 2, || mixer.meters().updated.is_some());
    assert_eq!(mixer.meters().m1.len(), 40);
}

#[test]
fn reconnects_after_outage() {
    let mock = MockMixer::start();
    let mixer = Mixer::connect_with(mock.addr(), fast());
    wait_for("connected", 5, || mixer.status() == Status::Connected);
    mock.set_offline(true);
    wait_for("lost", 3, || mixer.status() == Status::Lost);
    mock.console_set("/ch/02/config/name", Value::Str("Changed".into()));
    mock.set_offline(false);
    wait_for("reconnected", 5, || mixer.status() == Status::Connected);
    assert_eq!(mixer.state().name(StripId::Ch(2)), "Changed");
}

#[test]
fn linked_pairs_mirror() {
    let mock = MockMixer::start();
    let mixer = Mixer::connect_with(mock.addr(), fast());
    wait_for("connected", 5, || mixer.status() == Status::Connected);
    let st = mixer.state();
    assert_eq!(st.linked_partner(StripId::Ch(3)), Some(StripId::Ch(4)));
    assert_eq!(st.link_leader(StripId::Ch(4)), StripId::Ch(3));
    assert_eq!(st.linked_partner(StripId::Ch(1)), None);
    let shown = st.display_strips(&[StripId::Ch(1), StripId::Ch(4), StripId::Ch(3), StripId::Ch(5)]);
    assert_eq!(shown, vec![StripId::Ch(1), StripId::Ch(3), StripId::Ch(5)]);
    drop(st);
    mixer.set(Param::Fader(StripId::Ch(3)), Value::Float(0.5));
    assert_eq!(mixer.state().fader(StripId::Ch(4)), 0.5);
    wait_for("partner sent", 2, || mock.get("/ch/04/mix/fader") == Some(Value::Float(0.5)));
}
