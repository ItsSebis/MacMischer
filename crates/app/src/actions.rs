//! Every user intent (mouse, keyboard, later MIDI) becomes an [`Action`], applied in one place.

use xair_proto::{Param, StripId, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Set(Param, Value),
    Select(StripId),
    SelectBank(usize),
}
