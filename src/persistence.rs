use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    buffer::PasteBuffer,
    pane::{PaneId, WindowId},
    session::{PaneRuntime, Session, WindowRuntime},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PersistedState {
    pub last_session: Option<String>,
    pub next_window_id: u64,
    #[serde(default)]
    pub buffers: Vec<PasteBuffer>,
    pub sessions: BTreeMap<String, PersistedSession>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSession {
    pub name: String,
    pub cwd: Option<PathBuf>,
    pub command: Vec<String>,
    pub rows: u16,
    pub cols: u16,
    pub window_order: Vec<WindowId>,
    pub active_window: WindowId,
    #[serde(default)]
    pub last_window: Option<WindowId>,
    pub windows: BTreeMap<WindowId, PersistedWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedWindow {
    pub id: WindowId,
    pub name: String,
    pub layout: crate::layout::LayoutTree,
    #[serde(default)]
    pub next_pane_id: u64,
    pub panes: BTreeMap<PaneId, PersistedPane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedPane {
    pub id: PaneId,
    pub title: String,
    #[serde(default)]
    pub socket_path: Option<PathBuf>,
}

impl PersistedSession {
    pub fn from_live(session: &Session) -> Self {
        Self {
            name: session.name.clone(),
            cwd: session.cwd.clone(),
            command: session.command.clone(),
            rows: session.rows,
            cols: session.cols,
            window_order: session.window_order.clone(),
            active_window: session.active_window,
            last_window: session.last_window,
            windows: session
                .windows
                .iter()
                .map(|(id, window)| (*id, PersistedWindow::from_live(window)))
                .collect(),
        }
    }
}

impl PersistedWindow {
    pub fn from_live(window: &WindowRuntime) -> Self {
        Self {
            id: window.id,
            name: window.name.clone(),
            layout: window.layout.clone(),
            next_pane_id: window.next_pane_id,
            panes: window
                .panes
                .iter()
                .map(|(id, pane)| (*id, PersistedPane::from_live(pane)))
                .collect(),
        }
    }
}

impl PersistedPane {
    pub fn from_live(pane: &PaneRuntime) -> Self {
        Self {
            id: pane.id,
            title: pane.title.clone(),
            socket_path: Some(pane.process.socket_path().to_path_buf()),
        }
    }
}

pub fn load_state(path: &Path) -> Result<PersistedState> {
    if !path.exists() {
        return Ok(PersistedState::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read state file {}", path.display()))?;
    let state = serde_json::from_str(&raw)
        .with_context(|| format!("failed to decode state file {}", path.display()))?;
    Ok(state)
}

pub fn save_state(path: &Path, state: &PersistedState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create state directory {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let raw = serde_json::to_vec_pretty(state).context("failed to encode state file")?;
    fs::write(&tmp, raw).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrips_state_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        let mut state = PersistedState {
            last_session: Some("work".into()),
            next_window_id: 2,
            buffers: vec![PasteBuffer {
                name: "buffer0001".into(),
                data: "hello".into(),
                explicit_name: false,
                created_seq: 1,
            }],
            sessions: BTreeMap::new(),
        };
        state.sessions.insert(
            "work".into(),
            PersistedSession {
                name: "work".into(),
                cwd: None,
                command: vec!["sh".into()],
                rows: 24,
                cols: 80,
                window_order: vec![WindowId(1)],
                active_window: WindowId(1),
                last_window: None,
                windows: BTreeMap::new(),
            },
        );

        save_state(&path, &state).expect("save");
        let loaded = load_state(&path).expect("load");

        assert_eq!(loaded, state);
    }
}
