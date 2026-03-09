use crate::{
    ipc::ScrollDirection,
    layout::LayoutTree,
    pane::{PaneId, PaneSnapshot},
    pty::PaneProcess,
};
use anyhow::Result;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionName(pub String);

pub struct Session {
    pub name: String,
    pub cwd: Option<PathBuf>,
    pub command: Vec<String>,
    pub layout: LayoutTree,
    pub panes: BTreeMap<PaneId, PaneRuntime>,
}

pub struct PaneRuntime {
    pub id: PaneId,
    pub title: String,
    pub process: PaneProcess,
}

impl Session {
    pub fn new(
        name: String,
        cwd: Option<PathBuf>,
        command: Vec<String>,
        pane_id: PaneId,
    ) -> Result<Self> {
        let process = PaneProcess::spawn(&command, cwd.as_deref())?;
        let pane = PaneRuntime {
            id: pane_id,
            title: name.clone(),
            process,
        };
        let mut panes = BTreeMap::new();
        panes.insert(pane_id, pane);

        Ok(Self {
            name,
            cwd,
            command,
            layout: LayoutTree::new(pane_id),
            panes,
        })
    }

    pub fn active_pane_preview(&self) -> String {
        self.panes
            .get(&self.layout.active)
            .map(|pane| pane.process.preview())
            .unwrap_or_default()
    }

    pub fn active_pane_formatted_preview(&self) -> String {
        self.panes
            .get(&self.layout.active)
            .map(|pane| pane.process.formatted_preview())
            .unwrap_or_default()
    }

    pub fn active_pane_formatted_cursor(&self) -> String {
        self.panes
            .get(&self.layout.active)
            .map(|pane| pane.process.formatted_cursor())
            .unwrap_or_default()
    }

    pub fn active_pane_snapshot(&self) -> Option<PaneSnapshot> {
        self.panes
            .get(&self.layout.active)
            .map(|pane| PaneSnapshot {
                id: pane.id,
                title: pane.title.clone(),
                preview: pane.process.preview(),
            })
    }

    pub fn send_keys(&self, keys: &[String]) -> Result<()> {
        if let Some(pane) = self.panes.get(&self.layout.active) {
            pane.process.send_keys(keys)?;
        }
        Ok(())
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        if let Some(pane) = self.panes.get(&self.layout.active) {
            pane.process.resize(rows, cols)?;
        }
        Ok(())
    }

    pub fn handle_mouse_scroll(
        &self,
        direction: ScrollDirection,
        row: u16,
        col: u16,
    ) -> Result<()> {
        if let Some(pane) = self.panes.get(&self.layout.active) {
            pane.process.handle_mouse_scroll(direction, row, col)?;
        }
        Ok(())
    }

    pub fn kill(self) -> Result<()> {
        for pane in self.panes.into_values() {
            pane.process.kill()?;
        }
        Ok(())
    }
}
