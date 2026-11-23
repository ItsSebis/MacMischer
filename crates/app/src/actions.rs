//! Every user intent (mouse, keyboard, later MIDI) becomes an [`Action`], applied in one place.

use xair_proto::{Param, StripId, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Set(Param, Value),
    Select(StripId),
    /// Select the n-th (1-based) strip of the current bank.
    SelectPosition(u8),
    SelectBank(usize),
    /// Move the selected fader (or bus send in a monitor bank) by dB.
    Nudge(f32),
    /// Fader / send to 0 dB.
    Unity,
    PanNudge(f32),
    ToggleMute,
    ToggleSolo,
    /// Solo on/off for a strip (and its linked partner).
    SetSolo(StripId, bool),
    BankStep(i32),
    /// Select next/previous strip within the current bank.
    StripStep(i32),
    RecallScene(u8),
    /// Save the current mix into a slot with this name.
    SaveScene(u8, String),
    RenameScene(u8, String),
    ToggleLock,
    OpenScenes,
    OpenRouting,
    OpenFx,
    Undo,
    Redo,
    OpenRemote,
    OpenSettings,
    OpenMidi,
    /// A/B compare: toggle the selected strip between its two processing snapshots.
    AbToggle,
    /// A/B compare: switch the selected strip to a given slot.
    AbSelect(crate::ab::Slot),
    /// Save the selected strip's processing as a named preset.
    PresetSave(String),
    /// Load a named preset onto the selected strip.
    PresetLoad(String),
    PresetDelete(String),
    /// Load a preset after the user confirmed the compatibility / Safe-mode prompt.
    PresetLoadConfirmed(String),
    /// Re-read the preset directory (menu opened).
    PresetRefresh,
    /// Restore the strip values overwritten by the last preset load.
    PresetUndo,
    SetView(crate::views::DetailView),
}

/// Fader position after moving by `db`, always at least one mixer step in that direction.
pub fn nudge_fader(f: f32, db: f32) -> f32 {
    use xair_proto::fader;
    let cur = fader::to_db(f);
    let target = if cur < -89.9 {
        if db > 0.0 { -60.0 } else { return 0.0 }
    } else {
        cur + db
    };
    let mut q = if target < -89.9 { 0.0 } else { fader::quantize(fader::from_db(target)) };
    let step = 1.0 / 1023.0;
    if (q - f).abs() < step / 2.0 && q > 0.0 {
        q = fader::quantize(f + step * db.signum());
    }
    q.clamp(0.0, 1.0)
}

impl Action {
    /// Allowed while locked: navigation, mutes/solo, the main fader. Exhaustive on purpose: a new
    /// Action must be classified here before it compiles.
    pub fn allowed_when_locked(&self, selected: StripId) -> bool {
        match self {
            Action::Select(_)
            | Action::SelectPosition(_)
            | Action::SelectBank(_)
            | Action::BankStep(_)
            | Action::StripStep(_)
            | Action::ToggleMute
            | Action::ToggleSolo
            | Action::SetSolo(..)
            | Action::ToggleLock
            | Action::OpenScenes
            | Action::OpenRouting
            | Action::OpenFx
            | Action::OpenRemote
            | Action::OpenSettings
            | Action::OpenMidi
            | Action::PresetRefresh
            | Action::SetView(_) => true,
            Action::Set(p, _) => p.allowed_when_locked(),
            Action::Nudge(_) | Action::Unity => selected == StripId::Main,
            Action::PanNudge(_)
            | Action::RecallScene(_)
            | Action::SaveScene(..)
            | Action::RenameScene(..)
            | Action::Undo
            | Action::Redo
            | Action::AbToggle
            | Action::AbSelect(_)
            | Action::PresetSave(_)
            | Action::PresetLoad(_)
            | Action::PresetDelete(_)
            | Action::PresetLoadConfirmed(_)
            | Action::PresetUndo => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xair_proto::fader::to_db;

    #[test]
    fn lock_filter() {
        use xair_proto::Value;
        let ch = StripId::Ch(1);
        assert!(Action::ToggleMute.allowed_when_locked(ch));
        assert!(Action::Set(Param::On(ch), Value::Int(0)).allowed_when_locked(ch));
        assert!(Action::Set(Param::Fader(StripId::Main), Value::Float(0.5)).allowed_when_locked(ch));
        assert!(!Action::Set(Param::Fader(ch), Value::Float(0.5)).allowed_when_locked(ch));
        assert!(!Action::Nudge(1.0).allowed_when_locked(ch));
        assert!(Action::Nudge(1.0).allowed_when_locked(StripId::Main));
        assert!(!Action::RecallScene(2).allowed_when_locked(ch));
        assert!(!Action::PanNudge(0.1).allowed_when_locked(ch));
        assert!(!Action::AbToggle.allowed_when_locked(ch));
        assert!(!Action::PresetSave("x".into()).allowed_when_locked(ch));
        assert!(!Action::PresetLoad("x".into()).allowed_when_locked(StripId::Main));
        assert!(!Action::PresetDelete("x".into()).allowed_when_locked(ch));
        assert!(!Action::PresetLoadConfirmed("x".into()).allowed_when_locked(ch));
        assert!(Action::PresetRefresh.allowed_when_locked(ch));
        assert!(!Action::PresetUndo.allowed_when_locked(ch));
        assert!(!Action::AbSelect(crate::ab::Slot::B).allowed_when_locked(ch));
    }

    #[test]
    fn nudges() {
        let unity = 767.0 / 1023.0;
        assert!((to_db(nudge_fader(unity, 1.0)) - 1.0).abs() < 0.06);
        assert!((to_db(nudge_fader(unity, -3.0)) + 3.0).abs() < 0.06);
        assert!((to_db(nudge_fader(0.0, 1.0)) + 60.0).abs() < 0.3);
        assert_eq!(nudge_fader(0.0, -1.0), 0.0);
        assert_eq!(nudge_fader(1.0, 1.0), 1.0);
        // fine steps never get stuck in coarse regions
        let low = xair_proto::fader::quantize(xair_proto::fader::from_db(-50.0));
        assert!(nudge_fader(low, 0.1) > low);
        assert!(nudge_fader(low, -0.1) < low);
    }
}
