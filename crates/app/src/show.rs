//! Show files: named sets of fader banks persisted as human-editable TOML.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::banks::{self, Bank};

#[derive(Debug)]
pub enum ShowError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
}

impl fmt::Display for ShowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShowError::Io(e) => write!(f, "I/O error: {e}"),
            ShowError::Parse(e) => write!(f, "invalid show file: {e}"),
            ShowError::Serialize(e) => write!(f, "cannot serialize show: {e}"),
        }
    }
}

impl std::error::Error for ShowError {}

impl From<std::io::Error> for ShowError {
    fn from(e: std::io::Error) -> Self {
        ShowError::Io(e)
    }
}

fn default_version() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Show {
    pub name: String,
    #[serde(default)]
    pub current_bank: usize,
    #[serde(default)]
    pub banks: Vec<Bank>,
}

/// On-disk representation (adds `format_version`, keeps it first in the file).
#[derive(Serialize, Deserialize)]
struct ShowFile {
    #[serde(default = "default_version")]
    format_version: u32,
    #[serde(flatten)]
    show: Show,
}

impl Show {
    pub fn default_show() -> Show {
        Show { name: "Default".into(), banks: banks::defaults(), current_bank: 0 }
    }

    pub fn from_template(name: &str, template: &Show) -> Show {
        Show { name: name.into(), banks: template.banks.clone(), current_bank: 0 }
    }

    pub fn load(path: &Path) -> Result<Show, ShowError> {
        let text = std::fs::read_to_string(path)?;
        let mut show = toml::from_str::<ShowFile>(&text).map_err(ShowError::Parse)?.show;
        if show.banks.is_empty() {
            show.banks = banks::defaults();
        }
        show.current_bank = show.current_bank.min(show.banks.len() - 1);
        Ok(show)
    }

    pub fn save(&self, path: &Path) -> Result<(), ShowError> {
        let file = ShowFile { format_version: 1, show: self.clone() };
        let text = toml::to_string_pretty(&file).map_err(ShowError::Serialize)?;
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp = dir.join(tmp_name);
        std::fs::write(&tmp, text)?;
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    pub fn import_banks(&mut self, other: &Show) {
        for b in &other.banks {
            let mut bank = b.clone();
            if self.banks.iter().any(|x| x.name == bank.name) {
                let base = bank.name.clone();
                let mut n = 2;
                while self.banks.iter().any(|x| x.name == format!("{base} ({n})")) {
                    n += 1;
                }
                bank.name = format!("{base} ({n})");
            }
            self.banks.push(bank);
        }
    }
}

pub fn shows_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "MacMischer")
        .map(|d| d.data_dir().join("shows"))
        .unwrap_or_else(|| PathBuf::from("./shows"))
}

pub fn list_shows(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(String, PathBuf)> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| Show::load(&p).ok().map(|s| (s.name, p)))
        .collect();
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.1.cmp(&b.1)));
    out
}

pub fn path_for(dir: &Path, name: &str) -> PathBuf {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') { c } else { '_' })
        .collect();
    let clean = if clean.is_empty() { "show".to_string() } else { clean };
    dir.join(format!("{clean}.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xair_proto::{SendTo, StripId};

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut show = Show::default_show();
        show.name = "Sunday".into();
        let mut b = Bank::new("Band", [StripId::Ch(1), StripId::AuxIn, StripId::FxRtn(1), StripId::Bus(3), StripId::Dca(2), StripId::Main]);
        b.bus = Some(SendTo::Fx(2));
        b.positional = true;
        show.banks.push(b);
        show.current_bank = 5;
        let p = dir.path().join("sub/x.toml");
        show.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("format_version = 1"), "{text}");
        assert_eq!(Show::load(&p).unwrap(), show);
        assert_eq!(list_shows(&dir.path().join("sub")), vec![("Sunday".to_string(), p)]);
    }

    #[test]
    fn unknown_strip_skipped_and_clamp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.toml");
        std::fs::write(&p, "name='A'\ncurrent_bank=9\nfuture='x'\n[[banks]]\nname='B'\nstrips=['1','ZZ','99','B2']\nbus='Q9'\n").unwrap();
        let s = Show::load(&p).unwrap();
        assert_eq!(s.banks[0].strips, vec![StripId::Ch(1), StripId::Bus(2)]);
        assert_eq!(s.banks[0].bus, None);
        assert!(!s.banks[0].positional);
        assert_eq!(s.current_bank, 0);
    }

    #[test]
    fn empty_banks_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.toml");
        std::fs::write(&p, "name='A'\nbanks=[]\n").unwrap();
        assert_eq!(Show::load(&p).unwrap().banks, banks::defaults());
        std::fs::write(&p, "not toml [").unwrap();
        assert!(Show::load(&p).is_err());
    }

    #[test]
    fn import_renames() {
        let mut a = Show::default_show();
        let b = Show::default_show();
        let n = a.banks.len();
        a.import_banks(&b);
        a.import_banks(&b);
        assert_eq!(a.banks.len(), n * 3);
        assert_eq!(a.banks[n].name, format!("{} (2)", a.banks[0].name));
        assert_eq!(a.banks[2 * n].name, format!("{} (3)", a.banks[0].name));
    }

    #[test]
    fn path_sanitizing() {
        let d = Path::new("/x");
        assert_eq!(path_for(d, "Sun/day: 1-a_b"), d.join("Sun_day_ 1-a_b.toml"));
        assert_eq!(path_for(d, ""), d.join("show.toml"));
    }
}
