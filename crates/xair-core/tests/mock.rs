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

#[test]
fn rta_only_while_wanted() {
    let mock = MockMixer::start();
    let mixer = Mixer::connect_with(mock.addr(), fast());
    wait_for("connected", 5, || mixer.status() == Status::Connected);
    wait_for("meters flowing", 3, || mixer.meters().updated.is_some());
    std::thread::sleep(Duration::from_millis(300));
    assert!(mixer.meters().rta.is_empty(), "no RTA before it is wanted");
    assert!(mock.meter_subscribes(1) > 0);
    assert_eq!(mock.meter_subscribes(4), 0, "bank 4 not subscribed before it is wanted");
    mixer.set_rta_wanted(true);
    wait_for("rta", 3, || mixer.meters().rta.len() == 100);
    assert!(mock.meter_subscribes(4) > 0);
    assert!(mixer.meters().rta_updated.is_some());
    mixer.set_rta_wanted(false);
    assert!(mixer.meters().rta.is_empty() || {
        std::thread::sleep(Duration::from_millis(100));
        mixer.meters().rta.is_empty()
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(mixer.meters().rta.is_empty(), "stragglers ignored after off");
}

#[test]
fn solo_follows_the_linked_partner() {
    let mock = MockMixer::start();
    let mixer = Mixer::connect_with(mock.addr(), fast());
    wait_for("connected", 5, || mixer.status() == Status::Connected);
    let (a, b) = (StripId::Ch(1), StripId::Ch(2));
    mixer.set(Param::Solo(a.solo_index()), Value::Int(1));
    assert!(mixer.state().soloed(a) && !mixer.state().soloed(b), "unlinked: no mirror");
    wait_for("mock has the solo", 2, || {
        mock.get(&Param::Solo(a.solo_index()).path()) == Some(Value::Int(1))
    });
    assert_ne!(
        mock.get(&Param::Solo(b.solo_index()).path()),
        Some(Value::Int(1)),
        "unlinked partner receives no solo write"
    );
    mixer.set(Param::Solo(a.solo_index()), Value::Int(0));
    mixer.set(Param::ChLink(1), Value::Int(1));
    mixer.set(Param::Solo(a.solo_index()), Value::Int(1));
    assert!(mixer.state().soloed(b), "linked: partner follows");
    wait_for("mock has both", 2, || {
        mock.get(&Param::Solo(b.solo_index()).path()) == Some(Value::Int(1))
    });
}
