//! Stand-in when built without the `midi` feature.

use super::*;

#[allow(dead_code)]
pub fn list_ports() -> Result<Vec<String>, String> {
    Err("built without MIDI support".into())
}

pub struct MidiService;

impl MidiService {
    pub fn new(_ctx: eframe::egui::Context) -> MidiService {
        MidiService
    }
    pub fn configure(&self, _enabled: bool, _device: Option<&str>, _watch: bool) {}
    pub fn drain(&self) -> Vec<MidiMsg> {
        Vec::new()
    }
    pub fn ports(&self) -> Vec<String> {
        Vec::new()
    }
    pub fn status(&self) -> MidiStatus {
        MidiStatus::Error("built without MIDI support (cargo feature `midi`)".into())
    }
    pub fn available() -> bool {
        false
    }
}
