use anyhow::{Result, anyhow, bail};

use crate::pane::{PaneId, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Numbering {
    pub window_base: u64,
    pub pane_base: u64,
}

impl Numbering {
    pub fn public_window_number(self, index: usize) -> Result<u64> {
        self.window_base
            .checked_add(index as u64)
            .ok_or_else(|| anyhow!("window number overflow"))
    }

    pub fn parse_public_window_number(self, public: u64) -> Result<usize> {
        if public < self.window_base {
            bail!("window number {public} is below configured window_base {}", self.window_base);
        }
        let offset = public - self.window_base;
        usize::try_from(offset).map_err(|_| anyhow!("window number {public} is too large"))
    }

    pub fn public_pane_number(self, pane_id: PaneId) -> Result<u64> {
        self.pane_base
            .checked_add(pane_id.0)
            .ok_or_else(|| anyhow!("pane number overflow"))
    }

    pub fn parse_public_pane_number(self, public: u64) -> Result<PaneId> {
        if public < self.pane_base {
            bail!("pane number {public} is below configured pane_base {}", self.pane_base);
        }
        Ok(PaneId(public - self.pane_base))
    }

    pub fn public_window_id(self, id: WindowId, window_order: &[WindowId]) -> Result<u64> {
        let index = window_order
            .iter()
            .position(|current| *current == id)
            .ok_or_else(|| anyhow!("unknown window {}", id.0))?;
        self.public_window_number(index)
    }

    pub fn parse_public_window_id(self, public: u64, window_order: &[WindowId]) -> Result<WindowId> {
        let index = self.parse_public_window_number(public)?;
        window_order
            .get(index)
            .copied()
            .ok_or_else(|| anyhow!("unknown window {public}"))
    }
}
