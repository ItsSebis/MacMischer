//! MIDI control surface input: raw bytes -> [`MidiMsg`] -> bindings -> [`Effect`] -> [`Action`].
//!
//! Everything MIDI does goes through [`Action`] (and so through the lock filter and the undo
//! history). Mapping logic is pure and unit-tested; the device layer (`midir`, behind the `midi`
//! feature) only delivers parsed messages. Mixer writes are rate-limited by the network thread
//! (`Mixer::set` is coalesced, latest value wins), so a flood of CC messages is harmless.
//!
//! Conventions (same spirit as the web remote): values sent are absolute, faders have a level
//! cap (`max_db`, default +6 dB), and faders/pans use soft takeover so a controller never makes
//! the mixer jump.


mod binding;
mod engine;
mod learn;
mod pickup;
pub use binding::*;
pub use engine::*;
pub use learn::*;
pub use pickup::*;

// ---------------------------------------------------------------------------------------------
// Device layer

#[cfg_attr(not(feature = "midi"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub enum MidiStatus {
    Off,
    NoDeviceChosen,
    /// Chosen device is not plugged in; polling.
    Waiting(String),
    Connected(String),
    Error(String),
}

/// What the poller should do given what it sees.
#[cfg_attr(not(feature = "midi"), allow(dead_code))]
#[derive(Debug, PartialEq)]
pub enum Plan {
    Keep,
    Disconnect,
    Connect(String),
    Idle,
}

#[cfg_attr(not(feature = "midi"), allow(dead_code))]
pub fn plan(wanted: Option<&str>, ports: &[String], connected: Option<&str>) -> Plan {
    match (wanted, connected) {
        (None, Some(_)) => Plan::Disconnect,
        (None, None) => Plan::Idle,
        (Some(w), Some(c)) if w == c => {
            if ports.iter().any(|p| p == c) {
                Plan::Keep
            } else {
                Plan::Disconnect
            }
        }
        (Some(_), Some(_)) => Plan::Disconnect,
        (Some(w), None) if ports.iter().any(|p| p == w) => Plan::Connect(w.to_string()),
        (Some(_), None) => Plan::Idle,
    }
}

pub mod input;

#[cfg(feature = "midi")]
mod device;
#[cfg(not(feature = "midi"))]
#[path = "midi/device_stub.rs"]
mod device;

pub use device::MidiService;
#[cfg(test)]
use device::list_ports;

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Action;
    use std::time::{Duration, Instant};
    use xair_proto::{fader, Param, StripId, Value};

    struct Fake {
        fader: f32,
        pan: f32,
        muted: Option<bool>,
        soloed: bool,
        partner: Option<StripId>,
    }
    impl View for Fake {
        fn fader(&self, _: StripId) -> Option<f32> {
            Some(self.fader)
        }
        fn pan(&self, _: StripId) -> Option<f32> {
            Some(self.pan)
        }
        fn muted(&self, _: StripId) -> Option<bool> {
            self.muted
        }
        fn soloed(&self, _: StripId) -> Option<bool> {
            Some(self.soloed)
        }
        fn scene_name(&self, n: u8) -> String {
            if n == 9 { String::new() } else { "Scene".into() }
        }
        fn partner(&self, _: StripId) -> Option<StripId> {
            self.partner
        }
    }
    fn fake() -> Fake {
        Fake { fader: 0.5, pan: 0.5, muted: Some(false), soloed: false, partner: None }
    }
    const CH1: StripId = StripId::Ch(1);
    fn cc(cc: u8, value: u8) -> MidiMsg {
        MidiMsg::Cc { ch: 1, cc, value }
    }

    #[test]
    fn parse_messages() {
        assert_eq!(MidiMsg::parse(&[0xB0, 7, 100]), Some(MidiMsg::Cc { ch: 1, cc: 7, value: 100 }));
        assert_eq!(MidiMsg::parse(&[0xBF, 1, 0]), Some(MidiMsg::Cc { ch: 16, cc: 1, value: 0 }));
        assert_eq!(MidiMsg::parse(&[0x92, 60, 90]), Some(MidiMsg::NoteOn { ch: 3, note: 60, vel: 90 }));
        assert_eq!(MidiMsg::parse(&[0x92, 60, 0]), Some(MidiMsg::NoteOff { ch: 3, note: 60 }));
        assert_eq!(MidiMsg::parse(&[0x82, 60, 40]), Some(MidiMsg::NoteOff { ch: 3, note: 60 }));
        // ignored / malformed
        assert_eq!(MidiMsg::parse(&[0xF0, 1, 2, 0xF7]), None);
        assert_eq!(MidiMsg::parse(&[0xF8]), None);
        assert_eq!(MidiMsg::parse(&[0xFE]), None);
        assert_eq!(MidiMsg::parse(&[0xC0, 5]), None);
        assert_eq!(MidiMsg::parse(&[0xE0, 0, 64]), None);
        assert_eq!(MidiMsg::parse(&[0xB0, 7]), None);
        assert_eq!(MidiMsg::parse(&[0xB0, 200, 1]), None);
        assert_eq!(MidiMsg::parse(&[0x40, 1, 1]), None);
        assert_eq!(MidiMsg::parse(&[]), None);
    }

    #[test]
    fn value_laws() {
        assert_eq!(cc_to_fader(0, None), 0.0);
        assert_eq!(cc_to_fader(127, None), 1.0);
        let db95 = fader::to_db(cc_to_fader(95, None));
        assert!(db95.abs() < 0.3, "{db95}");
        // cap
        assert!(fader::to_db(cc_to_fader(127, Some(0.0))).abs() < 0.1);
        assert!(fader::to_db(cc_to_fader(127, Some(6.0))) <= 6.05);
        assert_eq!(cc_to_pan(0), 0.0);
        assert_eq!(cc_to_pan(64), 0.5);
        assert_eq!(cc_to_pan(127), 1.0);
    }

    #[test]
    fn pickup_state_machine() {
        let mut p = Pickup::default();
        // far below the fader: ignored
        assert_eq!(p.feed(0.1, 0.5), None);
        assert_eq!(p.feed(0.3, 0.5), None);
        // crosses it: engages (and passes the value through)
        assert_eq!(p.feed(0.6, 0.5), Some(0.6));
        assert_eq!(p.feed(0.8, 0.6), Some(0.8));
        // someone moves the fader elsewhere: re-arm until crossed again
        assert_eq!(p.feed(0.81, 0.2), None);
        assert_eq!(p.feed(0.5, 0.2), None);
        assert_eq!(p.feed(0.1, 0.2), Some(0.1));
        // near on first message engages immediately
        let mut q = Pickup::default();
        assert_eq!(q.feed(0.51, 0.5), Some(0.51));
        // from above works too
        let mut r = Pickup::default();
        assert_eq!(r.feed(0.9, 0.5), None);
        assert_eq!(r.feed(0.4, 0.5), Some(0.4));
    }

    #[test]
    fn engine_fader_pickup_and_cap() {
        let b = [Binding::new(1, Source::Cc(7), Target::StripFader(CH1))];
        let mut e = Engine::default();
        let mut v = fake(); // fader 0.5
        assert!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 7, value: 0 }, &v, false).is_empty());
        assert!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 7, value: 30 }, &v, false).is_empty());
        let out = e.handle(&b, MidiMsg::Cc { ch: 1, cc: 7, value: 80 }, &v, false);
        assert_eq!(out, vec![Effect::Fader(CH1, cc_to_fader(80, Some(DEFAULT_MAX_DB)))]);
        v.fader = cc_to_fader(80, Some(DEFAULT_MAX_DB)); // the mixer now holds what we sent
        // wrong channel / other cc ignored
        assert!(e.handle(&b, MidiMsg::Cc { ch: 2, cc: 7, value: 80 }, &v, false).is_empty());
        assert!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 8, value: 80 }, &v, false).is_empty());
        // capped at +6 dB
        let top = e.handle(&b, MidiMsg::Cc { ch: 1, cc: 7, value: 127 }, &v, false);
        match top[..] {
            [Effect::Fader(_, f)] => assert!(fader::to_db(f) <= 6.05),
            _ => panic!("{top:?}"),
        }
    }

    #[test]
    fn engine_buttons() {
        let b = [
            Binding::new(1, Source::Note(36), Target::StripMute(CH1)),
            Binding::new(1, Source::Cc(20), Target::StripSolo(CH1)),
            Binding::new(1, Source::Note(37), Target::RecallScene(3)),
            Binding::new(1, Source::Cc(21), Target::Select(StripId::Bus(2))),
        ];
        let mut e = Engine::default();
        let v = fake();
        assert_eq!(e.handle(&b, MidiMsg::NoteOn { ch: 1, note: 36, vel: 100 }, &v, false), vec![Effect::Mute(CH1, Op::Toggle)]);
        assert!(e.handle(&b, MidiMsg::NoteOff { ch: 1, note: 36 }, &v, false).is_empty());
        assert_eq!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 20, value: 127 }, &v, false), vec![Effect::Solo(CH1, Op::Set(true))]);
        assert_eq!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 20, value: 0 }, &v, false), vec![Effect::Solo(CH1, Op::Set(false))]);
        assert_eq!(e.handle(&b, MidiMsg::NoteOn { ch: 1, note: 37, vel: 1 }, &v, false), vec![Effect::Scene(3)]);
        // CC buttons fire on the rising edge only
        let down = MidiMsg::Cc { ch: 1, cc: 21, value: 127 };
        assert_eq!(e.handle(&b, down, &v, false), vec![Effect::Select(StripId::Bus(2))]);
        assert!(e.handle(&b, down, &v, false).is_empty());
        assert!(e.handle(&b, MidiMsg::Cc { ch: 1, cc: 21, value: 0 }, &v, false).is_empty());
        assert_eq!(e.handle(&b, down, &v, false).len(), 1);
    }

    #[test]
    fn effects_to_actions() {
        let mut v = fake();
        assert_eq!(
            Effect::Mute(CH1, Op::Toggle).to_action(&v, true),
            Ok(Some(Action::Set(Param::On(CH1), Value::Int(0))))
        );
        v.muted = Some(true);
        assert_eq!(
            Effect::Mute(CH1, Op::Toggle).to_action(&v, true),
            Ok(Some(Action::Set(Param::On(CH1), Value::Int(1))))
        );
        assert_eq!(Effect::Mute(CH1, Op::Set(true)).to_action(&v, true), Ok(None));
        assert_eq!(Effect::Solo(CH1, Op::Toggle).to_action(&v, false), Ok(Some(Action::SetSolo(CH1, true))));
        // Safe mode refuses scene recall; empty scenes refused
        assert!(Effect::Scene(3).to_action(&v, true).is_err());
        assert_eq!(Effect::Scene(3).to_action(&v, false), Ok(Some(Action::RecallScene(3))));
        assert!(Effect::Scene(9).to_action(&v, false).is_err());
        // locked console filters faders via the normal Action filter
        let a = Effect::Fader(CH1, 0.5).to_action(&v, false).unwrap().unwrap();
        assert!(!a.allowed_when_locked(CH1));
        let a = Effect::Fader(StripId::Main, 0.5).to_action(&v, false).unwrap().unwrap();
        assert!(a.allowed_when_locked(CH1));
        assert!(!Effect::Scene(3).to_action(&v, false).unwrap().unwrap().allowed_when_locked(CH1));
    }

    #[test]
    fn toml_round_trip_and_validation() {
        let map = MidiMap {
            bindings: vec![
                Binding::new(1, Source::Cc(7), Target::StripFader(StripId::Bus(3))),
                Binding::new(2, Source::Note(60), Target::StripMute(StripId::Main)),
                Binding::new(16, Source::Cc(10), Target::StripPan(StripId::AuxIn)),
                Binding::new(1, Source::Note(61), Target::RecallScene(64)),
            ],
        };
        let text = map.to_toml().unwrap();
        assert!(text.contains("target = \"fader:B3\""), "{text}");
        assert_eq!(MidiMap::from_toml(&text).unwrap(), map);

        let bad = |t: &str| MidiMap::from_toml(&format!("[[bindings]]\n{t}\n")).is_err();
        assert!(bad("channel = 0\nsource = \"cc:1\"\ntarget = \"mute:1\""));
        assert!(bad("channel = 17\nsource = \"cc:1\"\ntarget = \"mute:1\""));
        assert!(bad("channel = 1\nsource = \"cc:128\"\ntarget = \"mute:1\""));
        assert!(bad("channel = 1\nsource = \"note:1\"\ntarget = \"fader:1\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"pan:D1\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"mute:99\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"scene:65\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"scene:0\""));
        assert!(bad("channel = 1\nsource = \"cc:1\"\ntarget = \"fader:1\"\nmax_db = 30.0"));
        assert!(bad("channel = 1\nsource = \"foo\"\ntarget = \"mute:1\""));
        let dup = "[[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"mute:1\"\n[[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"mute:2\"\n";
        assert!(MidiMap::from_toml(dup).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn prefs_are_lenient() {
        let text = "enabled = true\ndevice = \"X\"\n\
            [[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"mute:1\"\n\
            [[bindings]]\nchannel = 99\nsource = \"cc:2\"\ntarget = \"mute:1\"\n\
            [[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"mute:2\"\n\
            [[bindings]]\ngarbage = 1\n";
        let p: MidiPrefs = toml::from_str(text).unwrap();
        assert!(p.enabled);
        assert_eq!(p.bindings.len(), 1);
        assert_eq!(toml::from_str::<MidiPrefs>("").unwrap(), MidiPrefs::default());
    }

    #[test]
    fn learning() {
        let mut b = vec![Binding::new(1, Source::Cc(7), Target::StripFader(CH1))];
        let req = LearnReq { target: Target::StripMute(CH1), replace: None };
        assert_eq!(learn(&mut b, &req, MidiMsg::NoteOff { ch: 1, note: 5 }), LearnOutcome::Ignored);
        assert_eq!(learn(&mut b, &req, MidiMsg::NoteOn { ch: 1, note: 5, vel: 9 }), LearnOutcome::Bound(1));
        assert_eq!(b[1].source, Source::Note(5));
        // already-bound source: consumed, keep waiting
        assert!(matches!(learn(&mut b, &req, MidiMsg::Cc { ch: 1, cc: 7, value: 1 }), LearnOutcome::Taken(_)));
        // CC-only target ignores notes
        let f = LearnReq { target: Target::StripFader(StripId::Ch(2)), replace: None };
        assert_eq!(learn(&mut b, &f, MidiMsg::NoteOn { ch: 1, note: 6, vel: 9 }), LearnOutcome::Ignored);
        // relearn keeps cap and may keep its own source
        let re = LearnReq { target: b[0].target, replace: Some(0) };
        assert_eq!(learn(&mut b, &re, MidiMsg::Cc { ch: 3, cc: 9, value: 1 }), LearnOutcome::Bound(0));
        assert_eq!((b[0].channel, b[0].source, b[0].max_db), (3, Source::Cc(9), Some(DEFAULT_MAX_DB)));
        assert_eq!(learn(&mut b, &re, MidiMsg::Cc { ch: 3, cc: 9, value: 1 }), LearnOutcome::Bound(0));
        assert!(b.iter().all(|x| x.validate().is_ok()));
    }

    #[test]
    fn reconnect_plan() {
        let ports = vec!["A".to_string(), "B".to_string()];
        assert_eq!(plan(None, &ports, None), Plan::Idle);
        assert_eq!(plan(None, &ports, Some("A")), Plan::Disconnect);
        assert_eq!(plan(Some("A"), &ports, None), Plan::Connect("A".into()));
        assert_eq!(plan(Some("A"), &ports, Some("A")), Plan::Keep);
        assert_eq!(plan(Some("C"), &ports, None), Plan::Idle);
        // device unplugged -> disconnect, then it is reconnected once it reappears
        assert_eq!(plan(Some("A"), &["B".to_string()], Some("A")), Plan::Disconnect);
        assert_eq!(plan(Some("A"), &ports, None), Plan::Connect("A".into()));
        // other device chosen
        assert_eq!(plan(Some("B"), &ports, Some("A")), Plan::Disconnect);
    }

    #[test]
    fn port_listing_never_panics() {
        // Without MIDI hardware / CoreMIDI access this is Ok(empty) or Err; either is fine.
        match list_ports() {
            Ok(p) => assert!(p.len() < 1000),
            Err(e) => assert!(!e.is_empty()),
        }
    }

    #[test]
    fn batch_sees_own_writes_with_unchanging_view() {
        // Fast sweep: many CCs in one frame while the mixer view never updates.
        let b = [Binding::new(1, Source::Cc(7), Target::StripFader(CH1))];
        let mut e = Engine::default();
        let v = fake(); // fader stays 0.5 in the base view
        let msgs: Vec<_> = [40, 60, 70, 75, 80, 85].iter().map(|&x| cc(7, x)).collect();
        let (acts, errs) = e.process(&b, &msgs, &v, false, false);
        assert!(errs.is_empty());
        // engages at the crossing (63 -> 70) and then keeps following every message
        let vals: Vec<f32> = acts
            .iter()
            .map(|a| match a {
                Action::Set(Param::Fader(_), Value::Float(f)) => *f,
                _ => panic!("{a:?}"),
            })
            .collect();
        assert_eq!(vals.len(), 4, "{vals:?}");
        assert!(vals.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn batch_toggle_twice_is_consistent() {
        let b = [Binding::new(1, Source::Note(36), Target::StripMute(CH1))];
        let mut e = Engine::default();
        let v = fake();
        let on = MidiMsg::NoteOn { ch: 1, note: 36, vel: 100 };
        let off = MidiMsg::NoteOff { ch: 1, note: 36 };
        let (acts, _) = e.process(&b, &[on, off, on], &v, false, false);
        assert_eq!(acts, vec![
            Action::Set(Param::On(CH1), Value::Int(0)),
            Action::Set(Param::On(CH1), Value::Int(1)),
        ]);
    }

    #[test]
    fn recall_stops_batch_and_levels_can_be_blocked() {
        let b = [
            Binding::new(1, Source::Note(37), Target::RecallScene(3)),
            Binding::new(1, Source::Cc(7), Target::StripFader(CH1)),
            Binding::new(1, Source::Note(36), Target::StripMute(CH1)),
        ];
        let mut e = Engine::default();
        let v = fake();
        let recall = MidiMsg::NoteOn { ch: 1, note: 37, vel: 1 };
        let mute = MidiMsg::NoteOn { ch: 1, note: 36, vel: 1 };
        let (acts, _) = e.process(&b, &[mute, recall, mute], &v, false, false);
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[1], Action::RecallScene(3));
        // blocked levels: faders ignored, buttons still work
        let (acts, _) = e.process(&b, &[cc(7, 64), cc(7, 66), mute], &v, false, true);
        assert_eq!(acts.len(), 1);
        // unblocked: the same sweep moves the fader
        let (acts, _) = e.process(&b, &[cc(7, 64), cc(7, 66)], &v, false, false);
        assert!(!acts.is_empty());
    }

    #[test]
    fn pickup_ignores_stale_crossings() {
        let t0 = Instant::now();
        let mut p = Pickup::default();
        assert_eq!(p.feed_at(0.1, 0.5, t0), None);
        // a big jump after a long pause is not a crossing
        assert_eq!(p.feed_at(0.9, 0.5, t0 + Duration::from_secs(5)), None);
        // a big jump in a fast sweep is
        assert_eq!(p.feed_at(0.1, 0.5, t0 + Duration::from_secs(5) + Duration::from_millis(10)), Some(0.1));
        // a small step is a crossing even after a pause
        let mut q = Pickup::default();
        assert_eq!(q.feed_at(0.46, 0.5, t0), None);
        assert_eq!(q.feed_at(0.54, 0.5, t0 + Duration::from_secs(9)), Some(0.54));
    }

    #[test]
    fn learn_hold_and_stale_messages() {
        let t0 = Instant::now();
        let m = MidiMsg::NoteOn { ch: 1, note: 5, vel: 100 };
        let mut hold = Some(Hold { key: m.key(), at: t0 });
        assert!(hold_suppresses(&mut hold, m, t0 + Duration::from_millis(50)));
        assert!(!hold_suppresses(&mut hold, cc(1, 1), t0 + Duration::from_millis(60))); // other source
        assert!(hold_suppresses(&mut hold, MidiMsg::NoteOff { ch: 1, note: 5 }, t0 + Duration::from_millis(100)));
        assert!(hold.is_none()); // released
        assert!(!hold_suppresses(&mut hold, m, t0 + Duration::from_millis(120)));
        // CC buttons: release is value < 64; timeout also ends it
        let mut hold = Some(Hold { key: (1, Source::Cc(9)), at: t0 });
        assert!(hold_suppresses(&mut hold, cc(9, 127), t0 + Duration::from_millis(10)));
        assert!(hold_suppresses(&mut hold, cc(9, 0), t0 + Duration::from_millis(20)));
        assert!(hold.is_none());
        let mut hold = Some(Hold { key: (1, Source::Cc(9)), at: t0 });
        assert!(!hold_suppresses(&mut hold, cc(9, 127), t0 + Duration::from_millis(500)));
        assert!(hold.is_none());
        // stale queue entries are dropped
        let items = vec![(t0, cc(1, 1)), (t0 + Duration::from_millis(900), cc(2, 2))];
        assert_eq!(fresh(items, t0 + Duration::from_secs(1), MAX_AGE), vec![cc(2, 2)]);
    }

    #[test]
    fn fader_cap_defaults_and_flooring() {
        let text = "[[bindings]]\nchannel = 1\nsource = \"cc:1\"\ntarget = \"fader:1\"\n";
        assert_eq!(MidiMap::from_toml(text).unwrap().bindings[0].max_db, Some(DEFAULT_MAX_DB));
        let p: MidiPrefs = toml::from_str(text).unwrap();
        assert_eq!(p.bindings[0].max_db, Some(DEFAULT_MAX_DB));
        // explicit +10 means no cap
        let t10 = format!("{text}max_db = 10.0\n");
        assert_eq!(cc_to_fader(127, MidiMap::from_toml(&t10).unwrap().bindings[0].max_db), 1.0);
        // caps are floored: never above the requested dB
        for db in [-20.0f32, -7.3, 0.0, 2.2, 6.0] {
            assert!(fader::to_db(cc_to_fader(127, Some(db))) <= db + 1e-3, "{db}");
        }
    }

    #[test]
    fn linked_strips_use_lowest_cap() {
        let mut quiet = Binding::new(1, Source::Cc(8), Target::StripFader(StripId::Ch(2)));
        quiet.max_db = Some(-6.0);
        let b = [Binding::new(1, Source::Cc(7), Target::StripFader(CH1)), quiet];
        let mut e = Engine::default();
        let v = Fake { partner: Some(StripId::Ch(2)), ..fake() };
        let (acts, _) = e.process(&b, &[cc(7, 64), cc(7, 66), cc(7, 127)], &v, false, false);
        match acts.last() {
            Some(Action::Set(Param::Fader(_), Value::Float(f))) => assert!(fader::to_db(*f) <= -6.0 + 1e-3),
            other => panic!("{other:?}"),
        }
        // without a link the +6 dB cap applies
        let (acts, _) = Engine::default().process(&b, &[cc(7, 64), cc(7, 66), cc(7, 127)], &fake(), false, false);
        match acts.last() {
            Some(Action::Set(Param::Fader(_), Value::Float(f))) => assert!(fader::to_db(*f) > 0.0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unsynced_mute_is_skipped() {
        let v = Fake { muted: None, ..fake() };
        assert_eq!(Effect::Mute(CH1, Op::Toggle).to_action(&v, false), Ok(None));
    }

    #[test]
    fn malformed_midi_table_does_not_reset_prefs() {
        let text = "safe_mode = true\nlast_mixer = \"1.2.3.4:10024\"\n[midi]\nenabled = \"yes\"\n";
        let p: crate::prefs::Prefs = toml::from_str(text).unwrap();
        assert!(p.safe_mode);
        assert_eq!(p.last_mixer.as_deref(), Some("1.2.3.4:10024"));
        assert_eq!(p.midi, MidiPrefs::default());
    }
}

