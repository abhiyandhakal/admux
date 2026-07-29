use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ClipboardBackend {
    #[default]
    Osc52,
    ExternalCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipboardConfig {
    pub backend: ClipboardBackend,
    pub command: Vec<String>,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            backend: ClipboardBackend::Osc52,
            command: Vec::new(),
        }
    }
}
