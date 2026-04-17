use crate::pane::WindowId;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowSummary {
    pub id: u64,
    pub index: u64,
    pub name: String,
    pub active: bool,
    #[serde(default)]
    pub last_selected: bool,
}

impl WindowSummary {
    pub fn new(
        id: WindowId,
        index: u64,
        name: String,
        active: bool,
        last_selected: bool,
    ) -> Self {
        Self {
            id: id.0,
            index,
            name,
            active,
            last_selected,
        }
    }
}
