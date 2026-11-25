//! App preferences (last mixer, last/recent shows), stored as TOML in the config dir.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// "host:port", e.g. "10.5.5.48:10024".
    pub last_mixer: Option<String>,
    pub last_show: Option<PathBuf>,
    pub recent_shows: Vec<PathBuf>,
    /// Safe mode: scene recall needs confirmation (Simple mode when false).
    pub safe_mode: bool,
    /// Window layout the user dragged to (remembered between launches).
    pub ui: UiPrefs,
    /// Ignore the scroll wheel / trackpad scroll on faders, bars and EQ bands (no accidental moves).
    pub disable_scroll: bool,
    /// Closing the window quits even while the web remote is on (default: keep running in the
    /// menu bar / tray so the remote keeps serving).
    pub quit_on_close: bool,
    /// MIDI input: enabled, device and bindings (not part of the show file).
    #[serde(deserialize_with = "crate::midi::lenient_prefs")]
    pub midi: crate::midi::MidiPrefs,
    /// Optional background image under the grey panels.
    pub background: crate::background::BackgroundPrefs,
    /// Launch update check and the skipped Release.
    pub updates: crate::updater::UpdatePrefs,
}

/// Layout sizes the user chose by dragging panel edges.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    /// Height of the meter bridge in points.
    pub bridge_h: Option<f32>,
    /// RTA shown beside the meter bridge instead of in its own window.
    pub rta_docked: bool,
    /// Width of the docked RTA in points.
    pub rta_dock_w: Option<f32>,
}

impl UiPrefs {
    /// Whether a panel height differs enough from the stored one to be worth saving
    /// (ignores sub-pixel jitter and non-finite values).
    pub fn moved(stored: Option<f32>, now: f32) -> bool {
        now.is_finite() && stored.is_none_or(|s| (s - now).abs() > 0.5) && now > 1.0
    }
}

impl Prefs {
    pub fn path() -> PathBuf {
        directories::ProjectDirs::from("", "", "MacMischer")
            .map(|d| d.config_dir().join("prefs.toml"))
            .unwrap_or_else(|| PathBuf::from("./prefs.toml"))
    }

    pub fn load() -> Prefs {
        Prefs::load_from(&Prefs::path())
    }

    pub fn load_from(p: &Path) -> Prefs {
        match std::fs::read_to_string(p) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("invalid prefs file {}: {e}", p.display());
                Prefs::default()
            }),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("cannot read prefs {}: {e}", p.display());
                }
                Prefs::default()
            }
        }
    }

    pub fn save_to(&self, p: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp_name = p.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp = p.with_file_name(tmp_name);
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, p).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    pub fn save(&self) {
        let p = Prefs::path();
        if let Err(e) = self.save_to(&p) {
            tracing::warn!("cannot save prefs {}: {e}", p.display());
        }
    }

    pub fn touch_show(&mut self, p: &Path) {
        self.last_show = Some(p.to_path_buf());
        self.recent_shows.retain(|x| x != p);
        self.recent_shows.insert(0, p.to_path_buf());
        self.recent_shows.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c/prefs.toml");
        assert_eq!(Prefs::load_from(&p), Prefs::default());
        let mut pr = Prefs { last_mixer: Some("10.5.5.48:10024".into()), ..Default::default() };
        pr.touch_show(Path::new("/a.toml"));
        pr.save_to(&p).unwrap();
        assert_eq!(Prefs::load_from(&p), pr);
        std::fs::write(&p, "garbage [").unwrap();
        assert_eq!(Prefs::load_from(&p), Prefs::default());
    }

    #[test]
    fn touch_show_order_and_limit() {
        let mut pr = Prefs::default();
        for i in 0..12 {
            pr.touch_show(Path::new(&format!("/{i}.toml")));
        }
        assert_eq!(pr.recent_shows.len(), 8);
        pr.touch_show(Path::new("/9.toml"));
        assert_eq!(pr.recent_shows[0], Path::new("/9.toml"));
        assert_eq!(pr.recent_shows.iter().filter(|p| *p == Path::new("/9.toml")).count(), 1);
        assert_eq!(pr.last_show.as_deref(), Some(Path::new("/9.toml")));
    }

    #[test]
    fn old_prefs_without_update_fields_load_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("prefs.toml");
        std::fs::write(&p, "last_mixer = \"10.5.5.48:10024\"\nsafe_mode = true\n").unwrap();
        let pr = Prefs::load_from(&p);
        assert_eq!(pr.last_mixer.as_deref(), Some("10.5.5.48:10024"));
        assert!(pr.updates.check_at_launch);
        assert_eq!(pr.updates.skipped_version, None);
        // A file with only part of the section also keeps the defaults.
        std::fs::write(&p, "[updates]\nskipped_version = \"0.2.0\"\n").unwrap();
        let pr = Prefs::load_from(&p);
        assert!(pr.updates.check_at_launch);
        assert_eq!(pr.updates.skipped_version.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn scroll_is_enabled_by_default_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("prefs.toml");
        std::fs::write(&p, "safe_mode = true\n").unwrap();
        assert!(!Prefs::load_from(&p).disable_scroll);
        let pr = Prefs { disable_scroll: true, ..Default::default() };
        pr.save_to(&p).unwrap();
        assert!(Prefs::load_from(&p).disable_scroll);
    }

    #[test]
    fn bridge_height_round_trips_and_old_files_load() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("prefs.toml");
        std::fs::write(&p, "safe_mode = true\n").unwrap();
        assert_eq!(Prefs::load_from(&p).ui.bridge_h, None);
        let pr = Prefs { ui: UiPrefs { bridge_h: Some(140.0), rta_docked: true, rta_dock_w: Some(500.0) }, ..Default::default() };
        pr.save_to(&p).unwrap();
        let back = Prefs::load_from(&p).ui;
        assert_eq!((back.bridge_h, back.rta_docked, back.rta_dock_w), (Some(140.0), true, Some(500.0)));
    }

    #[test]
    fn small_height_changes_are_not_saved() {
        assert!(UiPrefs::moved(None, 84.0));
        assert!(!UiPrefs::moved(Some(84.0), 84.3));
        assert!(UiPrefs::moved(Some(84.0), 90.0));
        assert!(!UiPrefs::moved(Some(84.0), f32::NAN));
    }
}
