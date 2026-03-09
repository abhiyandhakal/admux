#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct PaneId(pub u64);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSnapshot {
    pub id: PaneId,
    pub title: String,
    pub preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    pub fn contains(self, row: u16, col: u16) -> bool {
        row >= self.y && row < self.bottom() && col >= self.x && col < self.right()
    }

    pub fn content(self) -> Rect {
        Rect {
            x: self.x.saturating_add(1),
            y: self.y.saturating_add(1),
            width: self.width.saturating_sub(2),
            height: self.height.saturating_sub(2),
        }
    }
}
