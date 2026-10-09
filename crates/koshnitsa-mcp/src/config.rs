use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

/// `~/.config/koshnitsa/config.toml`. Absent means defaults, and the defaults cannot spend money.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ordering: Ordering,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Ordering {
    pub enabled: bool,
    /// Orders above this total, delivery and tip included, are refused.
    pub max_total_eur: f64,
    /// Payment methods `place_order` may use, as eBag names them.
    pub allowed_payment_methods: Vec<String>,
    /// Ask the user through MCP elicitation before placing. Turn off only for a client that
    /// cannot show a prompt and only if you accept the agent ordering on its own word.
    pub require_confirmation: bool,
}

impl Default for Ordering {
    fn default() -> Self {
        Self {
            enabled: false,
            max_total_eur: 80.0,
            allowed_payment_methods: vec!["cash".into()],
            require_confirmation: true,
        }
    }
}

pub fn dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default()
        .join("koshnitsa")
}

pub fn load() -> Result<Config> {
    let path = dir().join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cannot_order() {
        let c: Config = toml::from_str("").unwrap();
        assert!(!c.ordering.enabled);
        assert!(c.ordering.require_confirmation);
    }

    #[test]
    fn partial_ordering_section_keeps_other_defaults() {
        let c: Config = toml::from_str("[ordering]\nenabled = true\n").unwrap();
        assert!(c.ordering.enabled);
        assert_eq!(c.ordering.max_total_eur, 80.0);
        assert_eq!(c.ordering.allowed_payment_methods, ["cash"]);
    }
}
