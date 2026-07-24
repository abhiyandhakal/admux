use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    buffer::PasteBuffer,
    layout::LayoutNode,
    pane::{PaneId, WindowId},
    session::{PaneRuntime, Session, WindowRuntime},
};

pub const STATE_SCHEMA_VERSION: u32 = 1;
static STATE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PersistedState {
    #[serde(default)]
    pub schema_version: u32,
    pub last_session: Option<String>,
    pub next_window_id: u64,
    #[serde(default)]
    pub buffers: Vec<PasteBuffer>,
    #[serde(default)]
    pub workspaces: BTreeMap<String, String>,
    pub sessions: BTreeMap<String, PersistedSession>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSession {
    pub name: String,
    #[serde(default)]
    pub workspace_manifest: Option<String>,
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
    #[serde(default)]
    pub cwd: Option<PathBuf>,
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
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub socket_path: Option<PathBuf>,
}

impl PersistedSession {
    pub fn normalized(&self) -> Result<Self> {
        if self.rows == 0 || self.cols == 0 {
            bail!("persisted session {} has a zero-sized viewport", self.name);
        }
        if self.window_order.is_empty() {
            bail!("persisted session {} has no windows", self.name);
        }

        let ordered: std::collections::BTreeSet<_> = self.window_order.iter().copied().collect();
        if ordered.len() != self.window_order.len() || ordered.len() != self.windows.len() {
            bail!("persisted session {} has an invalid window order", self.name);
        }
        if ordered.iter().any(|id| !self.windows.contains_key(id)) {
            bail!("persisted session {} references an unknown window", self.name);
        }
        if !ordered.contains(&self.active_window) {
            bail!("persisted session {} has an unknown active window", self.name);
        }
        if self
            .last_window
            .is_some_and(|window_id| !ordered.contains(&window_id))
        {
            bail!("persisted session {} has an unknown last window", self.name);
        }

        let mut normalized = self.clone();
        for (window_id, window) in &mut normalized.windows {
            if *window_id != window.id {
                bail!("persisted session {} has a mismatched window id", self.name);
            }
            let mut layout_panes = Vec::new();
            collect_valid_layout_panes(&window.layout.root, &mut layout_panes)?;
            let layout_set: std::collections::BTreeSet<_> = layout_panes.iter().copied().collect();
            if layout_set.len() != layout_panes.len() || layout_set.len() != window.panes.len() {
                bail!("persisted window {} has inconsistent pane references", window_id.0);
            }
            if layout_set.iter().any(|pane_id| !window.panes.contains_key(pane_id)) {
                bail!("persisted window {} references an unknown pane", window_id.0);
            }
            if !layout_set.contains(&window.layout.active) {
                bail!("persisted window {} has an unknown active pane", window_id.0);
            }
            if window
                .panes
                .iter()
                .any(|(pane_id, pane)| *pane_id != pane.id)
            {
                bail!("persisted window {} has a mismatched pane id", window_id.0);
            }
            let max_pane = layout_set
                .iter()
                .map(|pane_id| pane_id.0)
                .max()
                .ok_or_else(|| anyhow!("persisted window {} has no panes", window_id.0))?;
            let minimum_next = max_pane
                .checked_add(1)
                .ok_or_else(|| anyhow!("persisted window {} exhausted pane ids", window_id.0))?;
            window.next_pane_id = window.next_pane_id.max(minimum_next);
        }
        Ok(normalized)
    }

    pub fn from_live(session: &Session) -> Self {
        Self {
            name: session.name.clone(),
            workspace_manifest: session.workspace_manifest.clone(),
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

fn collect_valid_layout_panes(node: &LayoutNode, panes: &mut Vec<PaneId>) -> Result<()> {
    match node {
        LayoutNode::Pane(pane_id) => panes.push(*pane_id),
        LayoutNode::Split {
            ratio,
            first,
            second,
            ..
        } => {
            if !(100..=900).contains(ratio) {
                bail!("persisted layout contains invalid split ratio {ratio}");
            }
            collect_valid_layout_panes(first, panes)?;
            collect_valid_layout_panes(second, panes)?;
        }
    }
    Ok(())
}

impl PersistedWindow {
    pub fn from_live(window: &WindowRuntime) -> Self {
        Self {
            id: window.id,
            name: window.name.clone(),
            cwd: window.cwd.clone(),
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
            cwd: pane.cwd.clone(),
            command: pane.command.clone(),
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
    let mut state: PersistedState = match serde_json::from_str(&raw) {
        Ok(state) => state,
        Err(error) => {
            let quarantined = quarantine_corrupt_state(path);
            match quarantined {
                Ok(quarantined_path) => eprintln!(
                    "admuxd: ignored corrupt state file {} (moved to {}): {error}",
                    path.display(),
                    quarantined_path.display()
                ),
                Err(quarantine_error) => eprintln!(
                    "admuxd: ignored corrupt state file {} (could not quarantine it: {quarantine_error}): {error}",
                    path.display()
                ),
            }
            return Ok(PersistedState::default());
        }
    };
    if state.schema_version > STATE_SCHEMA_VERSION {
        anyhow::bail!(
            "state file {} uses unsupported schema version {} (this admux supports {})",
            path.display(),
            state.schema_version,
            STATE_SCHEMA_VERSION
        );
    }
    // Version 0 is the pre-versioned format and is structurally compatible.
    state.schema_version = STATE_SCHEMA_VERSION;
    Ok(state)
}

fn quarantine_corrupt_state(path: &Path) -> Result<PathBuf> {
    let counter = STATE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let quarantined = path.with_extension(format!("json.corrupt.{}.{}", process::id(), counter));
    fs::rename(path, &quarantined).with_context(|| {
        format!(
            "failed to quarantine corrupt state file {} as {}",
            path.display(),
            quarantined.display()
        )
    })?;
    Ok(quarantined)
}

pub fn save_state(path: &Path, state: &PersistedState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create state directory {}", parent.display()))?;
    }
    let counter = STATE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.{}.{}.tmp", process::id(), counter));
    let mut state = state.clone();
    state.schema_version = STATE_SCHEMA_VERSION;
    let raw = serde_json::to_vec_pretty(&state).context("failed to encode state file")?;
    fs::write(&tmp, raw).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LayoutTree;
    use tempfile::tempdir;

    fn persisted_session() -> PersistedSession {
        PersistedSession {
            name: "work".into(),
            workspace_manifest: None,
            cwd: None,
            command: vec!["sh".into()],
            rows: 24,
            cols: 80,
            window_order: vec![WindowId(1)],
            active_window: WindowId(1),
            last_window: None,
            windows: BTreeMap::from([(
                WindowId(1),
                PersistedWindow {
                    id: WindowId(1),
                    name: "shell".into(),
                    cwd: None,
                    layout: LayoutTree::new(PaneId(0)),
                    next_pane_id: 0,
                    panes: BTreeMap::from([(
                        PaneId(0),
                        PersistedPane {
                            id: PaneId(0),
                            title: "shell".into(),
                            cwd: None,
                            command: vec!["sh".into()],
                            socket_path: None,
                        },
                    )]),
                },
            )]),
        }
    }

    #[test]
    fn roundtrips_state_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        let mut state = PersistedState {
            schema_version: STATE_SCHEMA_VERSION,
            last_session: Some("work".into()),
            next_window_id: 2,
            buffers: vec![PasteBuffer {
                name: "buffer0001".into(),
                data: "hello".into(),
                explicit_name: false,
                created_seq: 1,
            }],
            workspaces: BTreeMap::from([("/tmp/project/admux.toml".into(), "work".into())]),
            sessions: BTreeMap::new(),
        };
        state.sessions.insert(
            "work".into(),
            PersistedSession {
                name: "work".into(),
                workspace_manifest: None,
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

    #[test]
    fn corrupt_state_is_quarantined_and_does_not_block_startup() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        fs::write(&path, "not json").expect("write corrupt state");

        let state = load_state(&path).expect("recover corrupt state");

        assert_eq!(state, PersistedState::default());
        assert!(!path.exists());
        assert!(fs::read_dir(dir.path())
            .expect("read state directory")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with("state.json.corrupt.")));
    }

    #[test]
    fn future_state_schema_is_not_treated_as_corruption() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        let future_state = PersistedState {
            schema_version: STATE_SCHEMA_VERSION + 1,
            ..PersistedState::default()
        };
        fs::write(
            &path,
            serde_json::to_vec(&future_state).expect("encode future state"),
        )
        .expect("write future state");

        assert!(load_state(&path).is_err());
        assert!(path.exists());
    }

    #[test]
    fn normalizing_legacy_state_advances_pane_ids() {
        let normalized = persisted_session().normalized().expect("normalize state");
        assert_eq!(
            normalized.windows[&WindowId(1)].next_pane_id,
            1,
            "the next split must not overwrite pane zero"
        );
    }

    #[test]
    fn normalization_rejects_inconsistent_window_order() {
        let mut session = persisted_session();
        session.window_order.push(WindowId(1));
        assert!(session.normalized().is_err());
    }
}
