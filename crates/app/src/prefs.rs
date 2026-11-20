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
}
