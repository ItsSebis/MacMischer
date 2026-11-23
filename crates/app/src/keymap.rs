//! Key bindings → actions. Digits/prefix letters are handled by [`crate::input`] first.

use crate::actions::Action;
use crate::views::DetailView;
use eframe::egui::{Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    /// Named key with exact modifiers (`Modifiers::NONE` = plain).
    Key(Key, Mods),
    /// Typed character (after keyboard layout), e.g. `[` or `m`.
    Char(char),
}

/// Modifier subset that matters for bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub cmd: bool,
}

impl From<Modifiers> for Mods {
    fn from(m: Modifiers) -> Self {
        Mods { shift: m.shift, alt: m.alt, cmd: m.command }
    }
}

const NONE: Mods = Mods { shift: false, alt: false, cmd: false };
const SHIFT: Mods = Mods { shift: true, alt: false, cmd: false };
const ALT: Mods = Mods { shift: false, alt: true, cmd: false };
const CMD: Mods = Mods { shift: false, alt: false, cmd: true };

pub struct Keymap {
    bindings: Vec<(Binding, Action)>,
}

impl Default for Keymap {
    fn default() -> Self {
        use Action::*;
        let mut b = vec![
            (Binding::Key(Key::ArrowUp, NONE), Nudge(1.0)),
            (Binding::Key(Key::ArrowDown, NONE), Nudge(-1.0)),
            (Binding::Key(Key::ArrowUp, SHIFT), Nudge(3.0)),
            (Binding::Key(Key::ArrowDown, SHIFT), Nudge(-3.0)),
            (Binding::Key(Key::ArrowUp, ALT), Nudge(0.1)),
            (Binding::Key(Key::ArrowDown, ALT), Nudge(-0.1)),
            (Binding::Key(Key::ArrowLeft, NONE), StripStep(-1)),
            (Binding::Key(Key::ArrowRight, NONE), StripStep(1)),
            (Binding::Key(Key::Tab, NONE), StripStep(1)),
            (Binding::Key(Key::Tab, SHIFT), StripStep(-1)),
            (Binding::Char(','), PanNudge(-0.05)),
            (Binding::Char('.'), PanNudge(0.05)),
            (Binding::Char('<'), PanNudge(-0.25)),
            (Binding::Char('>'), PanNudge(0.25)),
            (Binding::Char('v'), SetView(DetailView::Mix)),
            (Binding::Char('e'), SetView(DetailView::Eq)),
            (Binding::Char('c'), SetView(DetailView::Dynamics)),
            (Binding::Char('m'), ToggleMute),
            (Binding::Char('s'), ToggleSolo),
            (Binding::Char('b'), AbToggle),
            (Binding::Key(Key::L, CMD), ToggleLock),
            (Binding::Key(Key::K, CMD), OpenScenes),
            (Binding::Key(Key::R, CMD), OpenRouting),
            (Binding::Key(Key::F, CMD), OpenFx),
            (Binding::Key(Key::Z, CMD), Undo),
            (Binding::Key(Key::Y, CMD), Redo),
            (Binding::Key(Key::P, CMD), OpenRemote),
            (Binding::Key(Key::Comma, CMD), OpenSettings),
            (Binding::Key(Key::M, CMD), OpenMidi),
            (Binding::Char('['), BankStep(-1)),
            (Binding::Char(']'), BankStep(1)),
        ];
        let fkeys = [
            Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6,
            Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12,
        ];
        for (i, k) in fkeys.into_iter().enumerate() {
            b.push((Binding::Key(k, NONE), SelectBank(i)));
        }
        Keymap { bindings: b }
    }
}

impl Keymap {
    pub fn lookup(&self, b: Binding) -> Option<Action> {
        let b = match b {
            Binding::Char(c) => Binding::Char(c.to_ascii_lowercase()),
            k => k,
        };
        self.bindings.iter().find(|(k, _)| *k == b).map(|(_, a)| a.clone())
    }

    /// Human-readable cheat sheet rows.
    pub fn help() -> &'static [(&'static str, &'static str)] {
        &[
            ("1 ... 16", "select n-th strip of the bank (or mixer channel, if set for the bank)"),
            ("a1-a6", "select bus"),
            ("f1-f4 / x1-x4", "FX return / FX send"),
            ("d1-d4", "select DCA"),
            ("r / l", "aux in / main LR"),
            ("Up / Down", "fader +/-1 dB  (Shift 3 dB, Alt 0.1 dB)"),
            ("Left / Right", "previous / next strip in bank"),
            (", .  (< >)", "pan left / right (coarse)"),
            ("0 0", "fader to 0 dB"),
            ("m / s", "mute / solo"),
            ("b", "A/B compare of the selected strip's EQ + dynamics (first press stores A and B)"),
            ("v / e / c", "detail: mix + sends / EQ / gate + compressor"),
            ("Tab / Shift+Tab", "next / previous strip in bank"),
            ("[ ]  F1-F12", "previous / next bank, jump to bank"),
            ("Ctrl/Cmd+K", "scenes"),
            ("Ctrl/Cmd+R", "routing"),
            ("Ctrl/Cmd+P", "web remote settings (phones / tablets)"),
            ("Ctrl/Cmd+,", "settings (remote, safety)"),
            ("Ctrl/Cmd+M", "MIDI controller settings (bindings, learn)"),
            ("Ctrl/Cmd+L", "lock / unlock (only mutes + main fader work)"),
            ("Esc", "cancel number entry"),
            ("?", "show / hide this help"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        let k = Keymap::default();
        assert_eq!(k.lookup(Binding::Key(Key::ArrowUp, SHIFT)), Some(Action::Nudge(3.0)));
        assert_eq!(k.lookup(Binding::Char('M')), Some(Action::ToggleMute));
        assert_eq!(k.lookup(Binding::Key(Key::F3, NONE)), Some(Action::SelectBank(2)));
        assert_eq!(k.lookup(Binding::Char('q')), None);
    }
}
