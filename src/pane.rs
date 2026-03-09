#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PaneId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSnapshot {
    pub id: PaneId,
    pub title: String,
    pub preview: String,
}
