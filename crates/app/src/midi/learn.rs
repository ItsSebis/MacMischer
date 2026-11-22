//! Learn: bind the next CC / note to a target.

use super::*;

// ---------------------------------------------------------------------------------------------
// Learn

/// A pending "Learn": the next CC / note-on is bound to `target`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LearnReq {
    pub target: Target,
    /// Re-learn an existing row (keeps its cap) instead of adding one.
    pub replace: Option<usize>,
}

#[derive(Debug, PartialEq)]
pub enum LearnOutcome {
    /// Not usable for learning (note-off, note for a CC-only target): handle it normally.
    Ignored,
    Bound(usize),
    /// Source already bound elsewhere: message is consumed, keep waiting for another control.
    Taken(String),
}

pub fn learn(bindings: &mut Vec<Binding>, req: &LearnReq, msg: MidiMsg) -> LearnOutcome {
    let (channel, source) = match msg {
        MidiMsg::Cc { .. } => msg.key(),
        MidiMsg::NoteOn { .. } if !req.target.needs_cc() => msg.key(),
        _ => return LearnOutcome::Ignored,
    };
    if let Some(i) = bindings.iter().position(|b| b.key() == (channel, source)) {
        if Some(i) != req.replace {
            return LearnOutcome::Taken(format!(
                "{} is already bound to {} - move another control",
                bindings[i].source_label(),
                bindings[i].target
            ));
        }
    }
    match req.replace.filter(|&i| i < bindings.len()) {
        Some(i) => {
            bindings[i].channel = channel;
            bindings[i].source = source;
            LearnOutcome::Bound(i)
        }
        None => {
            bindings.push(Binding::new(channel, source, req.target));
            LearnOutcome::Bound(bindings.len() - 1)
        }
    }
}

