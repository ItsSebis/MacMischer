//! Undo/redo of parameter edits made in the app (not changes made on the console or other clients).

use std::time::{Duration, Instant};
use xair_proto::{Param, Value};

const MAX: usize = 200;
/// Edits to the same parameter closer together than this form one undo step (a drag).
const COALESCE: Duration = Duration::from_millis(700);

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<(Param, Value)>,
    redo: Vec<(Param, Value)>,
    last: Option<(Param, Instant)>,
}

fn tracked(p: Param) -> bool {
    !matches!(p, Param::Solo(_) | Param::SnapIndex | Param::SnapName(_) | Param::MuteGroupOn(_))
}

impl History {
    /// Remember that `p` is about to change away from `old`.
    pub fn record(&mut self, p: Param, old: Option<Value>, now: Instant) {
        let Some(old) = old else { return };
        if !tracked(p) {
            return;
        }
        let same = matches!(self.last, Some((lp, t)) if lp == p && now.duration_since(t) < COALESCE);
        self.last = Some((p, now));
        if same {
            return;
        }
        self.redo.clear();
        self.undo.push((p, old));
        if self.undo.len() > MAX {
            self.undo.remove(0);
        }
    }

    /// Pop the last edit; `current` is the live value (pushed onto the redo stack). Returns what to write.
    pub fn undo(&mut self, current: impl Fn(Param) -> Option<Value>) -> Option<(Param, Value)> {
        let (p, old) = self.undo.pop()?;
        if let Some(cur) = current(p) {
            self.redo.push((p, cur));
        }
        self.last = None;
        Some((p, old))
    }

    pub fn redo(&mut self, current: impl Fn(Param) -> Option<Value>) -> Option<(Param, Value)> {
        let (p, v) = self.redo.pop()?;
        if let Some(cur) = current(p) {
            self.undo.push((p, cur));
        }
        self.last = None;
        Some((p, v))
    }

    pub fn clear(&mut self) {
        *self = History::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xair_proto::StripId;

    #[test]
    fn coalesces_drag_and_undoes_redoes() {
        let p = Param::Fader(StripId::Ch(1));
        let mut h = History::default();
        let t = Instant::now();
        h.record(p, Some(Value::Float(0.1)), t);
        h.record(p, Some(Value::Float(0.2)), t + Duration::from_millis(100));
        h.record(p, Some(Value::Float(0.3)), t + Duration::from_millis(200));
        assert_eq!(h.undo.len(), 1);
        let live = |_| Some(Value::Float(0.4));
        assert_eq!(h.undo(live), Some((p, Value::Float(0.1))));
        assert_eq!(h.undo(live), None);
        assert_eq!(h.redo(|_| Some(Value::Float(0.1))), Some((p, Value::Float(0.4))));
    }

    #[test]
    fn new_edit_clears_redo_and_untracked_ignored() {
        let p = Param::On(StripId::Ch(1));
        let mut h = History::default();
        let t = Instant::now();
        h.record(p, Some(Value::Int(1)), t);
        h.undo(|_| Some(Value::Int(0)));
        h.record(p, Some(Value::Int(1)), t + Duration::from_secs(5));
        assert!(h.redo.is_empty());
        h.record(Param::Solo(1), Some(Value::Int(0)), t + Duration::from_secs(9));
        assert_eq!(h.undo.len(), 1);
    }
}
