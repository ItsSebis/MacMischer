//! Remote configuration (stored in the show file under `[remote]`).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use xair_proto::{SendTo, StripId};

pub const DEFAULT_PORT: u16 = 8018;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    pub enabled: bool,
    pub port: u16,
    /// Empty = no PIN.
    pub pin: String,
    pub pages: Vec<PageConfig>,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        RemoteConfig { enabled: false, port: DEFAULT_PORT, pin: String::new(), pages: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageConfig {
    pub name: String,
    #[serde(serialize_with = "ser_strips", deserialize_with = "de_strips", default)]
    pub strips: Vec<StripId>,
    /// Faders control the sends to this bus/FX (personal monitor mix).
    #[serde(
        serialize_with = "ser_bus",
        deserialize_with = "de_bus",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bus: Option<SendTo>,
    /// Level cap in dB (fader law). `None` = no cap (+10 dB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_db: Option<f32>,
    #[serde(default = "yes")]
    pub meters: bool,
}

fn yes() -> bool {
    true
}

impl PageConfig {
    pub fn new(name: &str, strips: Vec<StripId>, bus: Option<SendTo>) -> PageConfig {
        PageConfig { name: name.into(), strips, bus, max_db: None, meters: true }
    }

    /// Cap as raw fader position 0..1.
    pub fn max_value(&self) -> f32 {
        self.max_db.map_or(1.0, |db| xair_proto::fader::quantize(xair_proto::fader::from_db(db)))
    }
}

/// "B1".."B6" / "FX1".."FX4" (same as bank files).
pub fn sendto_to_str(t: SendTo) -> String {
    match t {
        SendTo::Bus(n) => format!("B{n}"),
        SendTo::Fx(n) => format!("FX{n}"),
    }
}

pub fn sendto_from_str(s: &str) -> Option<SendTo> {
    let s = s.trim().to_ascii_uppercase();
    let t = if let Some(n) = s.strip_prefix("FX") {
        SendTo::Fx(n.parse().ok()?)
    } else {
        SendTo::Bus(s.strip_prefix('B')?.parse().ok()?)
    };
    SendTo::all().any(|x| x == t).then_some(t)
}

fn ser_strips<S: Serializer>(v: &[StripId], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(v.iter().map(|x| x.short()))
}

fn de_strips<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<StripId>, D::Error> {
    let v: Vec<String> = Vec::deserialize(d)?;
    Ok(v.iter().filter_map(|s| StripId::from_short(s)).collect())
}

fn ser_bus<S: Serializer>(v: &Option<SendTo>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(t) => s.serialize_str(&sendto_to_str(*t)),
        None => s.serialize_none(),
    }
}

fn de_bus<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SendTo>, D::Error> {
    let v: Option<String> = Option::deserialize(d)?;
    Ok(v.as_deref().and_then(sendto_from_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_toml() {
        let cfg = RemoteConfig {
            enabled: true,
            port: 8018,
            pin: "1234".into(),
            pages: vec![PageConfig {
                max_db: Some(4.0),
                ..PageConfig::new("Drums", vec![StripId::Ch(1), StripId::FxRtn(1)], Some(SendTo::Bus(2)))
            }],
        };
        let s = toml::to_string(&cfg).unwrap();
        assert!(s.contains("strips = [\"1\", \"FX1\"]"), "{s}");
        assert!(s.contains("bus = \"B2\""));
        let back: RemoteConfig = toml::from_str(&s).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn lenient_parsing() {
        let c: RemoteConfig = toml::from_str("[[pages]]\nname='x'\nstrips=['1','nope']\nbus='Q'\n").unwrap();
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(c.pages[0].strips, vec![StripId::Ch(1)]);
        assert_eq!(c.pages[0].bus, None);
        assert!(c.pages[0].meters);
        assert_eq!(c.pages[0].max_value(), 1.0);
    }
}
