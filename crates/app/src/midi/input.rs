//! One frame of MIDI input: Learn, the hold that follows a Learn, soft-takeover re-arming and the
//! mapping to Actions, in that order. `App` hands over the drained messages and gets Actions back.

use super::{hold_suppresses, learn, Binding, Engine, Hold, LearnOutcome, LearnReq, MidiMsg, MidiPrefs, MidiStatus, View};
use crate::actions::Action;
use std::time::Instant;
use xair_proto::{Param, StripId, Value};

/// What the MIDI window shows about Learn.
#[derive(Default)]
pub struct LearnState {
    /// Waiting for the next CC / note to bind.
    pub learning: Option<LearnReq>,
    pub last_msg: Option<MidiMsg>,
    pub note: Option<String>,
}

/// Everything one call needs from the app.
pub struct Frame<'a> {
    pub prefs: &'a mut MidiPrefs,
    pub learn: &'a mut LearnState,
    pub status: MidiStatus,
    pub view: &'a dyn View,
    pub safe_mode: bool,
    /// The mixer is re-reading its state (not connected, or just after a scene recall).
    pub resyncing: bool,
    pub now: Instant,
}

#[derive(Debug, Default)]
pub struct Fed {
    pub actions: Vec<Action>,
    /// Learn changed the bindings: the preferences need saving.
    pub bindings_changed: bool,
    /// First refusal message, if any (e.g. empty scene).
    pub error: Option<String>,
}

type Sig = (MidiStatus, bool, Option<String>, Vec<Binding>);

#[derive(Default)]
pub struct MidiInput {
    engine: Engine,
    hold: Option<Hold>,
    /// What the mapping last saw; a change re-arms soft takeover.
    sig: Option<Sig>,
}

impl MidiInput {
    /// Forget soft-takeover state (mixer switched or scene recalled).
    pub fn reset(&mut self) {
        self.engine.reset();
    }

    pub fn feed(&mut self, msgs: Vec<MidiMsg>, f: Frame) -> Fed {
        let Frame { prefs, learn: ls, status, view, safe_mode, resyncing, now } = f;
        // Re-arm soft takeover whenever the situation changes (device, bindings, caps, enable).
        let sig = (status, prefs.enabled, prefs.device.clone(), prefs.bindings.clone());
        if self.sig.as_ref() != Some(&sig) {
            self.engine.reset();
            self.sig = Some(sig);
        }
        let mut fed = Fed::default();
        let mut pending = Vec::new();
        for msg in msgs {
            ls.last_msg = Some(msg);
            if let Some(req) = ls.learning {
                match learn(&mut prefs.bindings, &req, msg) {
                    // Not usable for learning: handle normally.
                    LearnOutcome::Ignored => {}
                    LearnOutcome::Bound(_) => {
                        ls.learning = None;
                        ls.note = Some(format!("Bound {}", msg.describe()));
                        self.hold = Some(Hold { key: msg.key(), at: now });
                        fed.bindings_changed = true;
                        continue;
                    }
                    LearnOutcome::Taken(text) => {
                        ls.note = Some(text);
                        continue;
                    }
                }
            }
            if !hold_suppresses(&mut self.hold, msg, now) {
                pending.push(msg);
            }
        }
        if pending.is_empty() {
            return fed;
        }
        let (actions, errors) = self.engine.process(&prefs.bindings, &pending, view, safe_mode, resyncing);
        fed.actions = actions;
        fed.error = errors.into_iter().next();
        fed
    }
}

/// Mixer state as seen by the MIDI mapping.
pub struct MixerView<'a> {
    pub st: &'a xair_core::MixerState,
}

impl View for MixerView<'_> {
    fn fader(&self, s: StripId) -> Option<f32> {
        self.st.get(Param::Fader(s)).and_then(Value::as_f32)
    }
    fn pan(&self, s: StripId) -> Option<f32> {
        self.st.get(Param::Pan(s)).and_then(Value::as_f32)
    }
    fn muted(&self, s: StripId) -> Option<bool> {
        self.st.get(Param::On(s)).and_then(Value::as_i32).map(|v| v == 0)
    }
    fn soloed(&self, s: StripId) -> Option<bool> {
        self.st.get(Param::Solo(s.solo_index())).and_then(Value::as_i32).map(|v| v != 0)
    }
    fn partner(&self, s: StripId) -> Option<StripId> {
        self.st.linked_partner(s)
    }
    fn scene_name(&self, n: u8) -> String {
        self.st.snap_name(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::{cc_to_fader, Source, Target};

    const CH1: StripId = StripId::Ch(1);

    struct Fake;
    impl View for Fake {
        fn fader(&self, _: StripId) -> Option<f32> {
            Some(0.5)
        }
        fn pan(&self, _: StripId) -> Option<f32> {
            Some(0.5)
        }
        fn muted(&self, _: StripId) -> Option<bool> {
            Some(false)
        }
        fn soloed(&self, _: StripId) -> Option<bool> {
            Some(false)
        }
        fn scene_name(&self, _: u8) -> String {
            "Scene".into()
        }
        fn partner(&self, _: StripId) -> Option<StripId> {
            None
        }
    }

    struct Rig {
        input: MidiInput,
        prefs: MidiPrefs,
        learn: LearnState,
        status: MidiStatus,
        resyncing: bool,
        now: Instant,
    }

    impl Rig {
        fn new(bindings: Vec<Binding>) -> Rig {
            let prefs = MidiPrefs { enabled: true, device: Some("pad".into()), bindings };
            Rig {
                input: MidiInput::default(),
                prefs,
                learn: LearnState::default(),
                status: MidiStatus::Connected("pad".into()),
                resyncing: false,
                now: Instant::now(),
            }
        }

        fn feed(&mut self, msgs: Vec<MidiMsg>) -> Fed {
            self.input.feed(
                msgs,
                Frame {
                    prefs: &mut self.prefs,
                    learn: &mut self.learn,
                    status: self.status.clone(),
                    view: &Fake,
                    safe_mode: false,
                    resyncing: self.resyncing,
                    now: self.now,
                },
            )
        }
    }

    fn cc(cc: u8, value: u8) -> MidiMsg {
        MidiMsg::Cc { ch: 1, cc, value }
    }

    /// A CC value whose fader position is at (about) the fake's 0.5.
    fn near_half() -> u8 {
        (0..=127u8).min_by(|a, b| {
            let d = |v: &u8| (cc_to_fader(*v, None) - 0.5).abs();
            d(a).total_cmp(&d(b))
        })
        .unwrap()
    }

    fn fader_binding() -> Vec<Binding> {
        vec![Binding::new(1, Source::Cc(7), Target::StripFader(CH1))]
    }

    #[test]
    fn resyncing_drops_levels_but_not_buttons() {
        let mut r = Rig::new(vec![
            Binding::new(1, Source::Cc(7), Target::StripFader(CH1)),
            Binding::new(1, Source::Cc(20), Target::StripMute(CH1)),
        ]);
        r.resyncing = true;
        assert!(r.feed(vec![cc(7, near_half())]).actions.is_empty());
        assert_eq!(r.feed(vec![cc(20, 127)]).actions.len(), 1);
        r.resyncing = false;
        assert_eq!(r.feed(vec![cc(7, near_half())]).actions.len(), 1);
    }

    #[test]
    fn learn_binds_then_holds_the_control_until_released() {
        let mut r = Rig::new(vec![]);
        r.learn.learning = Some(LearnReq { target: Target::StripMute(CH1), replace: None });
        let fed = r.feed(vec![cc(20, 127)]);
        assert!(fed.bindings_changed && fed.actions.is_empty());
        assert_eq!(r.prefs.bindings.len(), 1);
        assert!(r.learn.learning.is_none());
        assert!(r.learn.note.as_deref().unwrap().starts_with("Bound"));
        assert_eq!(r.learn.last_msg, Some(cc(20, 127)));
        // the learning gesture itself must not fire the new binding
        assert!(r.feed(vec![cc(20, 127)]).actions.is_empty());
        assert!(r.feed(vec![cc(20, 0)]).actions.is_empty()); // release
        assert_eq!(r.feed(vec![cc(20, 127)]).actions.len(), 1);
    }

    #[test]
    fn learning_a_taken_control_keeps_waiting() {
        let mut r = Rig::new(vec![Binding::new(1, Source::Cc(20), Target::StripMute(CH1))]);
        r.learn.learning = Some(LearnReq { target: Target::StripSolo(CH1), replace: None });
        let fed = r.feed(vec![cc(20, 127)]);
        assert!(fed.actions.is_empty() && !fed.bindings_changed);
        assert!(r.learn.learning.is_some());
        assert!(r.learn.note.as_deref().unwrap().contains("already bound"));
    }

    #[test]
    fn changing_device_or_bindings_rearms_soft_takeover() {
        let mut r = Rig::new(fader_binding());
        let far = if near_half() < 64 { 127 } else { 0 };
        assert!(r.feed(vec![cc(7, far)]).actions.is_empty(), "far from the fader: not engaged");
        assert_eq!(r.feed(vec![cc(7, near_half())]).actions.len(), 1, "engaged at the fader");
        r.prefs.device = Some("other".into());
        assert!(r.feed(vec![cc(7, far)]).actions.is_empty(), "device changed: re-armed");
    }

    #[test]
    fn a_scene_recall_stops_the_batch_and_rearms() {
        let mut r = Rig::new(vec![
            Binding::new(1, Source::Note(37), Target::RecallScene(3)),
            Binding::new(1, Source::Note(36), Target::StripMute(CH1)),
        ]);
        let fed = r.feed(vec![
            MidiMsg::NoteOn { ch: 1, note: 37, vel: 100 },
            MidiMsg::NoteOn { ch: 1, note: 36, vel: 100 },
        ]);
        assert_eq!(fed.actions, vec![Action::RecallScene(3)], "nothing after the recall acts on the old scene");
    }
}
