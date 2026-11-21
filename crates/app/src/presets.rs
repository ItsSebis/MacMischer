//! Processing presets: a strip's HPF/EQ/gate/comp saved as a named TOML file and loadable onto
//! any strip. Values are keyed by the strip-relative path suffix (e.g. `/eq/2/f`), so a preset
//! can be applied to every strip that has the same parameter.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xair_proto::param::proc_params;
use xair_proto::{EqField, Param, Proc, StripId, Value};

pub const MAX_NAME: usize = 48;
const MAX_FILE: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    /// Strip family the preset was saved from ("channel", "bus", "main", ...).
    #[serde(default)]
    pub kind: String,
    /// Suffix -> raw (normalised) mixer value; integers for switch/enum parameters.
    pub values: BTreeMap<String, f64>,
}

pub fn kind_of(s: StripId) -> &'static str {
    match s {
        StripId::Ch(_) => "channel",
        StripId::Bus(_) => "bus",
        StripId::Main => "main",
        StripId::AuxIn => "auxin",
        StripId::FxRtn(_) => "fxreturn",
        StripId::FxSend(_) => "fxsend",
        StripId::Dca(_) => "dca",
    }
}

/// Key-source selections point at strip-specific signals and polarity belongs to the source,
/// so presets leave them alone.
fn portable(p: Proc) -> bool {
    !matches!(p, Proc::GateKeySrc | Proc::DynKeySrc | Proc::Invert)
}

/// Highest valid value of an integer (switch / enum) parameter.
fn int_max(p: Proc) -> i32 {
    match p {
        Proc::Eq(_, EqField::Type) => 5,
        Proc::GateMode => 4,
        Proc::DynRatio => 11,
        Proc::GateFilterType | Proc::DynFilterType => 8,
        _ => 1,
    }
}

/// A value that is safe to send for `p`: finite, in range, on a hardware step.
fn clean_value(p: Proc, x: f64) -> Option<Value> {
    if !x.is_finite() {
        return None;
    }
    Some(if p.is_int() {
        Value::Int(x.round().clamp(0.0, f64::from(int_max(p))) as i32)
    } else {
        let f = (x as f32).clamp(0.0, 1.0);
        Value::Float(p.steps().map_or(f, |n| xair_proto::scale::steps_quantize(f, n)))
    })
}

/// The parameters of strip `s` that go into a preset.
pub fn saved_params(s: StripId) -> Vec<Proc> {
    proc_params(s).into_iter().filter(|p| portable(*p)).collect()
}

/// Build a preset from the strip's live values; `None` if some value is not known yet.
pub fn capture(s: StripId, get: impl Fn(Param) -> Option<Value>) -> Option<Preset> {
    let mut values = BTreeMap::new();
    for p in saved_params(s) {
        let v = get(Param::Proc(s, p))?.as_f32()?;
        values.insert(p.suffix(), (f64::from(v) * 1e6).round() / 1e6);
    }
    (!values.is_empty()).then(|| Preset { kind: kind_of(s).into(), values })
}

/// What loading a preset onto a strip would write.
#[derive(Debug, PartialEq)]
pub struct Plan {
    pub writes: Vec<(Param, Value)>,
    /// Preset entries that do not exist on the target strip (or are unknown).
    pub skipped: usize,
}

/// Loading needs an explicit confirmation when the preset comes from another strip family
/// or not everything in it fits the target.
pub fn needs_confirm(preset: &Preset, plan: &Plan, s: StripId) -> bool {
    preset.kind != kind_of(s) || plan.skipped > 0
}

pub fn plan(preset: &Preset, s: StripId) -> Plan {
    let mut writes = Vec::new();
    let mut skipped = 0;
    for (suffix, v) in &preset.values {
        let p = Proc::parse_suffix(suffix).filter(|p| p.valid_for(s) && portable(*p));
        match p.and_then(|p| clean_value(p, *v).map(|v| (p, v))) {
            Some((p, v)) => writes.push((Param::Proc(s, p), v)),
            None => skipped += 1,
        }
    }
    Plan { writes, skipped }
}

fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn is_reserved(name: &str) -> bool {
    let u = name.to_uppercase();
    matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| u.strip_prefix(p).is_some_and(|d| matches!(d.as_bytes(), [b'1'..=b'9'])))
}

/// Turn a user-typed name into a safe file stem, or say why it is not acceptable.
pub fn sanitize_name(raw: &str) -> Result<String, String> {
    let name: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || is_combining(c) || matches!(c, ' ' | '-' | '_' | '(' | ')' | '+' | '#') { c } else { '_' })
        .collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Preset name is empty".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("Preset name too long (max {MAX_NAME})"));
    }
    if name.chars().all(|c| c == '_' || c == ' ') {
        return Err("Preset name has no usable characters".into());
    }
    if is_reserved(&name) {
        return Err("Preset name is reserved by the operating system".into());
    }
    Ok(name)
}

/// The saved preset whose name matches `name` ignoring case, if any.
pub fn find_existing<'a>(names: &'a [String], name: &str) -> Option<&'a String> {
    let n = sanitize_name(name).ok()?.to_lowercase();
    names.iter().find(|x| x.to_lowercase() == n)
}

pub fn dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "MacMischer")
        .map(|d| d.config_dir().join("presets"))
        .unwrap_or_else(|| PathBuf::from("./presets"))
}

fn file(dir: &Path, name: &str) -> Result<(String, PathBuf), String> {
    let name = sanitize_name(name)?;
    let p = dir.join(format!("{name}.toml"));
    Ok((name, p))
}

/// Save (overwriting a preset of the same name); returns the sanitised name.
pub fn save(dir: &Path, name: &str, preset: &Preset) -> Result<String, String> {
    let (mut name, _) = file(dir, name)?;
    if let Some(existing) = find_existing(&list(dir), &name) {
        name = existing.clone(); // keep the existing file's casing
    }
    let path = dir.join(format!("{name}.toml"));
    let text = toml::to_string_pretty(preset).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(name)
}

pub fn load(dir: &Path, name: &str) -> Result<Preset, String> {
    let (_, path) = file(dir, name)?;
    let f = std::fs::File::open(&path).map_err(|e| format!("Cannot read preset: {e}"))?;
    let mut text = String::new();
    f.take(MAX_FILE + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("Cannot read preset: {e}"))?;
    if text.len() as u64 > MAX_FILE {
        return Err("Preset file is too large".into());
    }
    toml::from_str(&text).map_err(|e| format!("Invalid preset: {e}"))
}

pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
    let (_, path) = file(dir, name)?;
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

/// Preset names, sorted case-insensitively. Missing directory = no presets.
pub fn list(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "toml").then(|| p.file_stem()?.to_str().map(String::from))?
        })
        .filter(|n| sanitize_name(n).is_ok_and(|s| s == *n))
        .collect();
    v.sort_by_key(|n| n.to_lowercase());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use xair_proto::EqField;

    fn live(s: StripId) -> HashMap<Param, Value> {
        proc_params(s)
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let v = if p.is_int() {
                    Value::Int((i % 2) as i32)
                } else {
                    let x = i as f32 / 100.0;
                    Value::Float(p.steps().map_or(x, |n| xair_proto::scale::steps_quantize(x, n)))
                };
                (Param::Proc(s, p), v)
            })
            .collect()
    }

    fn cap(s: StripId) -> Preset {
        let m = live(s);
        capture(s, |p| m.get(&p).cloned()).unwrap()
    }

    #[test]
    fn capture_excludes_key_sources_and_needs_all_values() {
        let s = StripId::Ch(1);
        let p = cap(s);
        assert_eq!(p.kind, "channel");
        assert!(!p.values.contains_key("/preamp/invert"));
        assert!(!p.values.contains_key("/gate/keysrc") && !p.values.contains_key("/dyn/keysrc"));
        assert!(p.values.contains_key("/eq/4/g") && p.values.contains_key("/preamp/hpf"));
        assert!(capture(s, |_| None).is_none());
    }

    #[test]
    fn toml_round_trip() {
        let p = cap(StripId::Ch(1));
        let text = toml::to_string_pretty(&p).unwrap();
        assert_eq!(toml::from_str::<Preset>(&text).unwrap(), p);
    }

    #[test]
    fn same_strip_round_trips_values() {
        let s = StripId::Ch(3);
        let m = live(s);
        let pl = plan(&cap(s), s);
        assert_eq!(pl.skipped, 0);
        for (p, v) in &pl.writes {
            let orig = m.get(p).unwrap();
            assert_eq!(orig.as_f32().map(|x| (x * 1e5).round()), v.as_f32().map(|x| (x * 1e5).round()));
            assert_eq!(matches!(orig, Value::Int(_)), matches!(v, Value::Int(_)));
        }
    }

    #[test]
    fn channel_preset_onto_bus_skips_gate_and_hpf() {
        let p = cap(StripId::Ch(1));
        let bus = StripId::Bus(2);
        let pl = plan(&p, bus);
        assert!(pl.skipped > 0);
        assert_eq!(pl.writes.len() + pl.skipped, p.values.len());
        assert!(pl.writes.iter().all(|(q, _)| matches!(q, Param::Proc(s, pr) if *s == bus && pr.valid_for(bus))));
        // 4 EQ bands * 4 fields + EqOn are all applied; bands 5/6 untouched
        let eq = pl.writes.iter().filter(|(q, _)| matches!(q, Param::Proc(_, Proc::Eq(..)))).count();
        assert_eq!(eq, 16);
        assert!(!pl.writes.iter().any(|(q, _)| matches!(q, Param::Proc(_, Proc::Eq(5..=6, _)))));
        assert!(!pl.writes.iter().any(|(q, _)| matches!(q, Param::Proc(_, Proc::Hpf | Proc::GateThr))));
    }

    #[test]
    fn bus_preset_onto_channel_drops_extra_bands() {
        let p = cap(StripId::Bus(1));
        let pl = plan(&p, StripId::Ch(1));
        assert_eq!(pl.skipped, 8); // bands 5 and 6
        assert!(pl.writes.iter().any(|(q, _)| *q == Param::Proc(StripId::Ch(1), Proc::Eq(4, EqField::F))));
    }

    #[test]
    fn preset_onto_strip_without_processing_writes_nothing() {
        let pl = plan(&cap(StripId::Ch(1)), StripId::Dca(1));
        assert!(pl.writes.is_empty());
        assert_eq!(pl.skipped, cap(StripId::Ch(1)).values.len());
    }

    #[test]
    fn unknown_suffix_is_skipped() {
        let p = Preset { kind: "channel".into(), values: BTreeMap::from([("/bogus/x".to_string(), 1.0), ("/eq/on".to_string(), 1.0)]) };
        let pl = plan(&p, StripId::Ch(1));
        assert_eq!(pl.skipped, 1);
        assert_eq!(pl.writes, vec![(Param::Proc(StripId::Ch(1), Proc::EqOn), Value::Int(1))]);
    }

    fn one(suffix: &str, v: f64) -> Plan {
        let p = Preset { kind: "channel".into(), values: BTreeMap::from([(suffix.to_string(), v)]) };
        plan(&p, StripId::Ch(1))
    }

    fn written(pl: &Plan) -> Value {
        assert_eq!(pl.writes.len(), 1, "{pl:?}");
        pl.writes[0].1.clone()
    }

    #[test]
    fn values_are_validated() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let pl = one("/eq/1/g", bad);
            assert!(pl.writes.is_empty() && pl.skipped == 1);
            assert_eq!(one("/eq/on", bad).skipped, 1);
        }
        assert_eq!(written(&one("/preamp/hpf", -3.0)), Value::Float(0.0));
        assert_eq!(written(&one("/preamp/hpf", 7.5)), Value::Float(1.0));
        assert_eq!(written(&one("/dyn/thr", 2.0)), Value::Float(1.0));
        // ints clamp per parameter
        assert_eq!(written(&one("/eq/on", 5.0)), Value::Int(1));
        assert_eq!(written(&one("/eq/on", -5.0)), Value::Int(0));
        assert_eq!(written(&one("/eq/2/type", 99.0)), Value::Int(5));
        assert_eq!(written(&one("/gate/mode", 99.0)), Value::Int(4));
        assert_eq!(written(&one("/dyn/ratio", 99.0)), Value::Int(11));
        assert_eq!(written(&one("/dyn/filter/type", 99.0)), Value::Int(8));
        assert_eq!(written(&one("/gate/filter/type", 9.0)), Value::Int(8));
        for s in ["/dyn/mode", "/dyn/det", "/dyn/env"] {
            assert_eq!(written(&one(s, 3.0)), Value::Int(1));
        }
        // stepped floats snap to the hardware grid
        assert_eq!(written(&one("/eq/1/f", 0.1551)), Value::Float(xair_proto::scale::steps_quantize(0.1551, 201)));
        assert_eq!(written(&one("/eq/1/q", 0.4507)), Value::Float(32.0 / 71.0));
        assert_eq!(written(&one("/eq/1/g", 3.0)), Value::Float(1.0));
    }

    #[test]
    fn invert_is_not_applied() {
        let pl = one("/preamp/invert", 1.0);
        assert!(pl.writes.is_empty() && pl.skipped == 1);
    }

    #[test]
    fn confirmation_rules() {
        let ch = cap(StripId::Ch(1));
        let ok = plan(&ch, StripId::Ch(2));
        assert!(!needs_confirm(&ch, &ok, StripId::Ch(2)));
        let to_bus = plan(&ch, StripId::Bus(1));
        assert!(needs_confirm(&ch, &to_bus, StripId::Bus(1)));
        // same kind but something skipped
        let mut odd = ch.clone();
        odd.values.insert("/bogus".into(), 0.0);
        assert!(needs_confirm(&odd, &plan(&odd, StripId::Ch(2)), StripId::Ch(2)));
        // missing kind (old / hand-written file) counts as a mismatch
        let nokind: Preset = toml::from_str("[values]\n\"/eq/on\" = 1\n").unwrap();
        assert_eq!(nokind.kind, "");
        assert!(needs_confirm(&nokind, &plan(&nokind, StripId::Ch(1)), StripId::Ch(1)));
    }

    #[test]
    fn names_unicode_reserved_and_case() {
        let nfd = "Cafe\u{301} Vox";
        assert_eq!(sanitize_name(nfd).unwrap(), nfd);
        for r in ["CON", "nul", "Com1", "lpt9", "aux"] {
            assert!(sanitize_name(r).is_err(), "{r}");
        }
        assert!(sanitize_name("COM0").is_ok() && sanitize_name("Console").is_ok());
        let names = vec!["Vocal".to_string(), "bass".to_string()];
        assert_eq!(find_existing(&names, " VOCAL ").map(String::as_str), Some("Vocal"));
        assert_eq!(find_existing(&names, "Drums"), None);
    }

    #[test]
    fn save_over_existing_keeps_casing_and_big_files_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let p = cap(StripId::Ch(1));
        save(tmp.path(), "Vocal", &p).unwrap();
        assert_eq!(save(tmp.path(), "VOCAL", &p).unwrap(), "Vocal");
        assert_eq!(list(tmp.path()), vec!["Vocal".to_string()]);
        std::fs::write(tmp.path().join("big.toml"), format!("kind = \"x\"\n#{}", "a".repeat(70_000))).unwrap();
        assert!(load(tmp.path(), "big").unwrap_err().contains("too large"));
        let nfd = "Cafe\u{301}";
        save(tmp.path(), nfd, &p).unwrap();
        assert!(list(tmp.path()).contains(&nfd.to_string()));
    }

    #[test]
    fn name_sanitising() {
        assert_eq!(sanitize_name("  Vocal (lead) ").unwrap(), "Vocal (lead)");
        assert_eq!(sanitize_name("../../etc/passwd").unwrap(), ".._.._etc_passwd".replace('.', "_"));
        assert_eq!(sanitize_name("a/b\\c:d").unwrap(), "a_b_c_d");
        assert!(sanitize_name("").is_err());
        assert!(sanitize_name("   ").is_err());
        assert!(sanitize_name("../..").is_err());
        assert!(sanitize_name(&"x".repeat(MAX_NAME + 1)).is_err());
        assert!(sanitize_name(&"x".repeat(MAX_NAME)).is_ok());
        for n in ["../x", "a/b", "..", "."] {
            let s = sanitize_name(n).unwrap_or_default();
            assert!(!s.contains('/') && !s.contains('.'));
        }
    }

    #[test]
    fn file_io_in_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("presets");
        assert!(list(&dir).is_empty());
        let p = cap(StripId::Ch(1));
        assert_eq!(save(&dir, "Zeta", &p).unwrap(), "Zeta");
        assert_eq!(save(&dir, "alpha/../x", &p).unwrap(), "alpha____x");
        assert_eq!(list(&dir), vec!["alpha____x".to_string(), "Zeta".to_string()]);
        assert_eq!(load(&dir, "Zeta").unwrap(), p);
        assert!(load(&dir, "missing").is_err());
        assert!(save(&dir, "../..", &p).is_err());
        std::fs::write(dir.join("bad.toml"), "garbage [").unwrap();
        assert!(load(&dir, "bad").is_err());
        delete(&dir, "Zeta").unwrap();
        assert!(delete(&dir, "Zeta").is_err());
        assert_eq!(list(&dir), vec!["alpha____x".to_string(), "bad".to_string()]);
        // nothing escaped the directory
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}
