use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub keys: KeyConfig,
    pub mouse: MouseConfig,
    pub behavior: BehaviorConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub status_position: StatusPosition,
    pub show_pane_labels: bool,
    pub status_clock: bool,
    pub status_show_pane: bool,
    pub status_show_window_list: bool,
    pub status_style: StatusStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyConfig {
    pub leader: String,
    pub bindings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    pub scrollback_lines: usize,
    pub default_shell: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StatusPosition {
    Top,
    #[default]
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StatusStyle {
    #[default]
    TmuxPlus,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ui: UiConfig::default(),
            keys: KeyConfig::default(),
            mouse: MouseConfig::default(),
            behavior: BehaviorConfig::default(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            status_position: StatusPosition::Bottom,
            show_pane_labels: true,
            status_clock: true,
            status_show_pane: true,
            status_show_window_list: true,
            status_style: StatusStyle::TmuxPlus,
        }
    }
}

impl Default for KeyConfig {
    fn default() -> Self {
        let mut bindings = BTreeMap::new();
        bindings.insert("split-right".into(), "\"".into());
        bindings.insert("split-down".into(), "%".into());
        bindings.insert("detach".into(), "d".into());
        Self {
            leader: "Ctrl-b".into(),
            bindings,
        }
    }
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            default_shell: None,
        }
    }
}

impl Config {
    pub fn from_toml(input: &str) -> Result<Self> {
        let config = toml::from_str(input).context("failed to parse config TOML")?;
        Ok(config)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file at {}", path.display()))?;
        Self::from_toml(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn empty_config_uses_defaults() {
        let config = Config::from_toml("").expect("default config");
        assert_eq!(config.keys.leader, "Ctrl-b");
        assert_eq!(config.ui.status_position, StatusPosition::Bottom);
        assert!(config.ui.status_clock);
        assert!(config.ui.status_show_pane);
        assert!(config.ui.status_show_window_list);
        assert_eq!(config.ui.status_style, StatusStyle::TmuxPlus);
        assert!(config.mouse.enabled);
        assert_eq!(config.behavior.scrollback_lines, 10_000);
    }

    #[test]
    fn partial_config_overrides_defaults() {
        let config = Config::from_toml(
            r#"
                [ui]
                status_position = "top"

                [behavior]
                scrollback_lines = 2048
            "#,
        )
        .expect("partial config");
        assert_eq!(config.ui.status_position, StatusPosition::Top);
        assert!(config.ui.show_pane_labels);
        assert!(config.ui.status_clock);
        assert_eq!(config.behavior.scrollback_lines, 2048);
        assert_eq!(config.keys.leader, "Ctrl-b");
    }

    #[test]
    fn loading_config_from_path_reads_toml() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("config.toml");
        fs::write(&path, "[mouse]\nenabled = false\n").expect("write config");

        let config = Config::load_from_path(&path).expect("load config");

        assert!(!config.mouse.enabled);
    }
}
