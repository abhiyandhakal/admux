use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    layout::{LayoutNode, SplitAxis},
    pane::PaneId,
    session::{PaneRuntime, Session, WindowRuntime},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSpec {
    pub manifest_path: PathBuf,
    pub manifest_dir: PathBuf,
    pub name: String,
    pub cwd: PathBuf,
    pub active_window: usize,
    pub windows: Vec<WorkspaceWindowSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWindowSpec {
    pub name: String,
    pub cwd: PathBuf,
    pub active_pane: u64,
    pub root: WorkspacePaneSpec,
    pub splits: Vec<WorkspaceSplitSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePaneSpec {
    pub cwd: PathBuf,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSplitSpec {
    pub target: u64,
    pub direction: SplitAxis,
    pub ratio: u16,
    pub pane: WorkspacePaneSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLoad {
    pub manifest_key: String,
    pub manifest_digest: String,
    pub spec: WorkspaceSpec,
    pub snapshot: Option<WorkspaceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub version: u16,
    pub saved_at_unix: u64,
    pub manifest_path: String,
    pub manifest_digest: String,
    pub session_name: String,
    pub active_window: usize,
    pub windows: Vec<WorkspaceWindowSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceWindowSnapshot {
    pub window_index: usize,
    pub active_pane: u64,
    pub panes: Vec<WorkspacePaneSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePaneSnapshot {
    pub pane_id: u64,
    pub title: String,
    pub cwd: PathBuf,
    pub command: Vec<String>,
    pub rows: u16,
    pub cols: u16,
    pub vt: String,
}

impl WorkspaceSnapshot {
    pub fn pane(&self, window_index: usize, pane_id: u64) -> Option<&WorkspacePaneSnapshot> {
        self.windows
            .iter()
            .find(|window| window.window_index == window_index)
            .and_then(|window| window.panes.iter().find(|pane| pane.pane_id == pane_id))
    }

    pub fn active_pane(&self, window_index: usize) -> Option<u64> {
        self.windows
            .iter()
            .find(|window| window.window_index == window_index)
            .map(|window| window.active_pane)
    }
}

#[derive(Debug, Deserialize)]
struct RawWorkspaceManifest {
    version: u16,
    workspace: Option<RawWorkspaceSettings>,
    #[serde(default)]
    windows: Vec<RawWindowSpec>,
}

#[derive(Debug, Serialize)]
struct WorkspaceManifestOut {
    version: u16,
    workspace: WorkspaceSettingsOut,
    windows: Vec<WindowSpecOut>,
}

#[derive(Debug, Serialize)]
struct WorkspaceSettingsOut {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    active_window: usize,
}

#[derive(Debug, Serialize)]
struct WindowSpecOut {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "is_zero_u64")]
    active_pane: u64,
    root: PaneSpecOut,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    splits: Vec<SplitSpecOut>,
}

#[derive(Debug, Serialize)]
struct PaneSpecOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    command: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SplitSpecOut {
    target: u64,
    direction: SplitAxisOut,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    command: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SplitAxisOut {
    Horizontal,
    Vertical,
}

#[derive(Debug, Default, Deserialize)]
struct RawWorkspaceSettings {
    name: Option<String>,
    cwd: Option<PathBuf>,
    active_window: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RawWindowSpec {
    name: String,
    cwd: Option<PathBuf>,
    active_pane: Option<u64>,
    root: RawPaneSpec,
    #[serde(default)]
    splits: Vec<RawSplitSpec>,
}

#[derive(Debug, Deserialize)]
struct RawPaneSpec {
    cwd: Option<PathBuf>,
    command: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawSplitSpec {
    target: u64,
    direction: RawSplitDirection,
    size: Option<f32>,
    cwd: Option<PathBuf>,
    command: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawSplitDirection {
    Horizontal,
    Vertical,
}

impl From<RawSplitDirection> for SplitAxis {
    fn from(value: RawSplitDirection) -> Self {
        match value {
            RawSplitDirection::Horizontal => SplitAxis::Horizontal,
            RawSplitDirection::Vertical => SplitAxis::Vertical,
        }
    }
}

pub fn load_workspace(path: &Path) -> Result<WorkspaceLoad> {
    let manifest_path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace file {}", path.display()))?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| {
            anyhow!(
                "workspace file {} has no parent directory",
                manifest_path.display()
            )
        })?
        .to_path_buf();
    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read workspace file {}", manifest_path.display()))?;
    let manifest_digest = manifest_digest(&raw);
    let manifest: RawWorkspaceManifest = toml::from_str(&raw)
        .with_context(|| format!("failed to parse workspace file {}", manifest_path.display()))?;
    let mut workspace = resolve_workspace(manifest_path.clone(), manifest_dir, manifest)?;
    workspace.manifest_digest = manifest_digest.clone();
    workspace.snapshot = load_snapshot_sidecar(&manifest_path, &manifest_digest)?;
    Ok(workspace)
}

pub fn save_workspace(session: &Session, snapshot_lines: usize) -> Result<PathBuf> {
    let session_dir = session.cwd.clone().ok_or_else(|| {
        anyhow!(
            "session {} does not have a workspace directory",
            session.name
        )
    })?;
    fs::create_dir_all(&session_dir).with_context(|| {
        format!(
            "failed to create session directory {}",
            session_dir.display()
        )
    })?;
    let path = session_dir.join("admux.toml");
    let manifest = export_workspace(session)?;
    let raw = toml::to_string_pretty(&manifest).context("failed to encode workspace manifest")?;
    fs::write(&path, raw)
        .with_context(|| format!("failed to write workspace manifest {}", path.display()))?;
    let digest = manifest_digest(
        &fs::read_to_string(&path)
            .with_context(|| format!("failed to reread workspace manifest {}", path.display()))?,
    );
    write_snapshot_sidecar(session, &path, &digest, snapshot_lines)?;
    Ok(path)
}

fn resolve_workspace(
    manifest_path: PathBuf,
    manifest_dir: PathBuf,
    manifest: RawWorkspaceManifest,
) -> Result<WorkspaceLoad> {
    if manifest.version != 1 {
        bail!(
            "unsupported workspace manifest version {}; expected 1",
            manifest.version
        );
    }
    if manifest.windows.is_empty() {
        bail!("workspace manifest must define at least one window");
    }

    let workspace = manifest.workspace.unwrap_or_default();
    let name = workspace
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            manifest_dir
                .file_name()
                .and_then(|part| part.to_str())
                .filter(|part| !part.is_empty())
                .unwrap_or("workspace")
                .to_string()
        });
    let cwd = resolve_cwd(workspace.cwd.as_ref(), &manifest_dir);
    let active_window = workspace.active_window.unwrap_or(0);
    if active_window >= manifest.windows.len() {
        bail!(
            "workspace active_window {} is out of range for {} windows",
            active_window,
            manifest.windows.len()
        );
    }

    let mut windows = Vec::with_capacity(manifest.windows.len());
    for (window_index, window) in manifest.windows.into_iter().enumerate() {
        let window_cwd = resolve_cwd(window.cwd.as_ref(), &cwd);
        let root = resolve_pane_spec(window.root, &window_cwd)?;
        let mut known_panes = 1u64;
        let mut splits = Vec::with_capacity(window.splits.len());
        for split in window.splits {
            if split.target >= known_panes {
                bail!(
                    "window {} split target {} does not exist yet",
                    window_index,
                    split.target
                );
            }
            let ratio = resolve_ratio(split.size)?;
            splits.push(WorkspaceSplitSpec {
                target: split.target,
                direction: split.direction.into(),
                ratio,
                pane: resolve_pane_spec(
                    RawPaneSpec {
                        cwd: split.cwd,
                        command: split.command,
                    },
                    &window_cwd,
                )?,
            });
            known_panes += 1;
        }

        let active_pane = window.active_pane.unwrap_or(0);
        if active_pane >= known_panes {
            bail!(
                "window {} active_pane {} is out of range for {} panes",
                window_index,
                active_pane,
                known_panes
            );
        }

        windows.push(WorkspaceWindowSpec {
            name: window.name,
            cwd: window_cwd,
            active_pane,
            root,
            splits,
        });
    }

    Ok(WorkspaceLoad {
        manifest_key: manifest_path.display().to_string(),
        manifest_digest: String::new(),
        spec: WorkspaceSpec {
            manifest_path,
            manifest_dir,
            name,
            cwd,
            active_window,
            windows,
        },
        snapshot: None,
    })
}

fn export_workspace(session: &Session) -> Result<WorkspaceManifestOut> {
    let session_cwd = session.cwd.clone().ok_or_else(|| {
        anyhow!(
            "session {} does not have a workspace directory",
            session.name
        )
    })?;
    let active_window = session
        .window_order
        .iter()
        .position(|window_id| *window_id == session.active_window)
        .unwrap_or(0);

    let windows = session
        .window_order
        .iter()
        .map(|window_id| {
            let window = session
                .windows
                .get(window_id)
                .ok_or_else(|| anyhow!("missing window {}", window_id.0))?;
            export_window(window, &session_cwd)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(WorkspaceManifestOut {
        version: 1,
        workspace: WorkspaceSettingsOut {
            name: session.name.clone(),
            cwd: Some(PathBuf::from(".")),
            active_window,
        },
        windows,
    })
}

fn export_window(window: &WindowRuntime, session_cwd: &Path) -> Result<WindowSpecOut> {
    let window_base = window.cwd.as_deref().unwrap_or(session_cwd);
    let pane_map = manifest_pane_map(window);
    let root_pane_id = origin_pane(&window.layout.root);
    let root = export_pane(
        window
            .panes
            .get(&root_pane_id)
            .ok_or_else(|| anyhow!("missing root pane {}", root_pane_id.0))?,
        window_base,
    )?;
    let mut splits = Vec::new();
    collect_splits(
        &window.layout.root,
        window,
        window_base,
        &pane_map,
        &mut splits,
    )?;
    Ok(WindowSpecOut {
        name: window.name.clone(),
        cwd: relativize(window_base, session_cwd),
        active_pane: *pane_map
            .get(&window.layout.active)
            .ok_or_else(|| anyhow!("missing active pane {}", window.layout.active.0))?,
        root,
        splits,
    })
}

fn collect_splits(
    node: &LayoutNode,
    window: &WindowRuntime,
    window_base: &Path,
    pane_map: &BTreeMap<PaneId, u64>,
    splits: &mut Vec<SplitSpecOut>,
) -> Result<PaneId> {
    match node {
        LayoutNode::Pane(pane_id) => Ok(*pane_id),
        LayoutNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let target = origin_pane(first);
            let new_pane = origin_pane(second);
            let pane = window
                .panes
                .get(&new_pane)
                .ok_or_else(|| anyhow!("missing pane {}", new_pane.0))?;
            splits.push(SplitSpecOut {
                target: *pane_map
                    .get(&target)
                    .ok_or_else(|| anyhow!("missing mapped pane {}", target.0))?,
                direction: match axis {
                    SplitAxis::Horizontal => SplitAxisOut::Horizontal,
                    SplitAxis::Vertical => SplitAxisOut::Vertical,
                },
                size: if *ratio == 500 {
                    None
                } else {
                    Some((*ratio as f32) / 1000.0)
                },
                cwd: relativize(pane.cwd.as_deref().unwrap_or(window_base), window_base),
                command: pane.command.clone(),
            });
            collect_splits(first, window, window_base, pane_map, splits)?;
            collect_splits(second, window, window_base, pane_map, splits)?;
            Ok(target)
        }
    }
}

fn manifest_pane_map(window: &WindowRuntime) -> BTreeMap<PaneId, u64> {
    let mut map = BTreeMap::new();
    map.insert(origin_pane(&window.layout.root), 0);
    let mut next = 1u64;
    assign_manifest_pane_ids(&window.layout.root, &mut map, &mut next);
    map
}

fn assign_manifest_pane_ids(node: &LayoutNode, map: &mut BTreeMap<PaneId, u64>, next: &mut u64) {
    match node {
        LayoutNode::Pane(_) => {}
        LayoutNode::Split { first, second, .. } => {
            let new_pane = origin_pane(second);
            map.entry(new_pane).or_insert_with(|| {
                let current = *next;
                *next += 1;
                current
            });
            assign_manifest_pane_ids(first, map, next);
            assign_manifest_pane_ids(second, map, next);
        }
    }
}

fn export_pane(pane: &PaneRuntime, base: &Path) -> Result<PaneSpecOut> {
    if pane.command.is_empty() {
        bail!("pane {} does not have a stored command", pane.id.0);
    }
    Ok(PaneSpecOut {
        cwd: relativize(pane.cwd.as_deref().unwrap_or(base), base),
        command: pane.command.clone(),
    })
}

fn origin_pane(node: &LayoutNode) -> PaneId {
    match node {
        LayoutNode::Pane(pane_id) => *pane_id,
        LayoutNode::Split { first, .. } => origin_pane(first),
    }
}

fn relativize(path: &Path, base: &Path) -> Option<PathBuf> {
    if path == base {
        return None;
    }
    path.strip_prefix(base)
        .map(PathBuf::from)
        .ok()
        .or_else(|| Some(path.to_path_buf()))
}

fn write_snapshot_sidecar(
    session: &Session,
    manifest_path: &Path,
    digest: &str,
    snapshot_lines: usize,
) -> Result<()> {
    let snapshot = export_snapshot(session, manifest_path, digest, snapshot_lines)?;
    let state_dir = workspace_state_dir(manifest_path);
    fs::create_dir_all(&state_dir)
        .with_context(|| format!("failed to create {}", state_dir.display()))?;
    let gitignore = state_dir.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, "*\n!.gitignore\n")
            .with_context(|| format!("failed to write {}", gitignore.display()))?;
    }
    let snapshot_path = workspace_snapshot_path(manifest_path);
    let raw =
        serde_json::to_vec_pretty(&snapshot).context("failed to encode workspace snapshot")?;
    fs::write(&snapshot_path, raw)
        .with_context(|| format!("failed to write {}", snapshot_path.display()))?;
    Ok(())
}

fn export_snapshot(
    session: &Session,
    manifest_path: &Path,
    digest: &str,
    snapshot_lines: usize,
) -> Result<WorkspaceSnapshot> {
    let mut windows = Vec::new();
    for (window_index, window_id) in session.window_order.iter().enumerate() {
        let window = session
            .windows
            .get(window_id)
            .ok_or_else(|| anyhow!("missing window {}", window_id.0))?;
        let pane_map = manifest_pane_map(window);
        let mut panes = Vec::new();
        for pane_id in window.layout.panes() {
            let pane = window
                .panes
                .get(&pane_id)
                .ok_or_else(|| anyhow!("missing pane {}", pane_id.0))?;
            let manifest_pane_id = *pane_map
                .get(&pane_id)
                .ok_or_else(|| anyhow!("missing mapped pane {}", pane_id.0))?;
            let persistent =
                session.pane_persistent_snapshot(*window_id, pane_id, snapshot_lines)?;
            panes.push(WorkspacePaneSnapshot {
                pane_id: manifest_pane_id,
                title: pane.title.clone(),
                cwd: pane
                    .cwd
                    .clone()
                    .or_else(|| window.cwd.clone())
                    .or_else(|| session.cwd.clone())
                    .ok_or_else(|| anyhow!("pane {} does not have a cwd", pane.id.0))?,
                command: pane.command.clone(),
                rows: persistent.rows,
                cols: persistent.cols,
                vt: persistent.vt,
            });
        }
        panes.sort_by_key(|pane| pane.pane_id);
        windows.push(WorkspaceWindowSnapshot {
            window_index,
            active_pane: *pane_map
                .get(&window.layout.active)
                .ok_or_else(|| anyhow!("missing active pane {}", window.layout.active.0))?,
            panes,
        });
    }
    Ok(WorkspaceSnapshot {
        version: 1,
        saved_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        manifest_path: manifest_path.display().to_string(),
        manifest_digest: digest.to_string(),
        session_name: session.name.clone(),
        active_window: session
            .window_order
            .iter()
            .position(|window_id| *window_id == session.active_window)
            .unwrap_or(0),
        windows,
    })
}

fn load_snapshot_sidecar(manifest_path: &Path, digest: &str) -> Result<Option<WorkspaceSnapshot>> {
    let snapshot_path = workspace_snapshot_path(manifest_path);
    if !snapshot_path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&snapshot_path)
        .with_context(|| format!("failed to read {}", snapshot_path.display()))?;
    let snapshot: WorkspaceSnapshot = serde_json::from_str(&raw)
        .with_context(|| format!("failed to decode {}", snapshot_path.display()))?;
    if snapshot.version != 1 {
        return Ok(None);
    }
    if snapshot.manifest_digest != digest {
        return Ok(None);
    }
    Ok(Some(snapshot))
}

pub fn workspace_state_dir(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .map(|parent| parent.join(".admux"))
        .unwrap_or_else(|| PathBuf::from(".admux"))
}

fn workspace_snapshot_path(manifest_path: &Path) -> PathBuf {
    workspace_state_dir(manifest_path).join("snapshot.json")
}

fn manifest_digest(raw: &str) -> String {
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn resolve_pane_spec(raw: RawPaneSpec, base: &Path) -> Result<WorkspacePaneSpec> {
    if raw.command.is_empty() {
        bail!("workspace pane command cannot be empty");
    }
    Ok(WorkspacePaneSpec {
        cwd: resolve_cwd(raw.cwd.as_ref(), base),
        command: raw.command,
    })
}

fn resolve_cwd(path: Option<&PathBuf>, base: &Path) -> PathBuf {
    match path {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => base.join(path),
        None => base.to_path_buf(),
    }
}

fn resolve_ratio(value: Option<f32>) -> Result<u16> {
    let value = value.unwrap_or(0.5);
    if !(0.0 < value && value < 1.0) {
        bail!("workspace split size must be between 0 and 1");
    }
    let ratio = (value * 1000.0).round() as u16;
    Ok(ratio.clamp(100, 900))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::WindowDefaults, pane::WindowId, session::Session};
    use std::{thread, time::Duration};
    use tempfile::tempdir;

    fn load(raw: &str) -> Result<WorkspaceLoad> {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("admux.toml");
        fs::write(&path, raw).expect("write manifest");
        load_workspace(&path)
    }

    fn wait_for_preview(session: &Session, needle: &str) {
        for _ in 0..50 {
            if session.active_pane_preview().contains(needle) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn parses_minimal_workspace_manifest() {
        let workspace = load(
            r#"
version = 1

[[windows]]
name = "editor"
root = { command = ["nvim"] }
"#,
        )
        .expect("load workspace");

        assert!(!workspace.spec.name.is_empty());
        assert_eq!(workspace.spec.windows.len(), 1);
        assert_eq!(workspace.spec.windows[0].name, "editor");
        assert_eq!(workspace.spec.windows[0].root.command, vec!["nvim"]);
        assert_eq!(workspace.spec.windows[0].active_pane, 0);
    }

    #[test]
    fn resolves_relative_paths_against_manifest_dir() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("admux.toml");
        fs::write(
            &path,
            r#"
version = 1

[workspace]
cwd = "repo"

[[windows]]
name = "editor"
cwd = "frontend"
root = { cwd = "src", command = ["nvim"] }

[[windows.splits]]
target = 0
direction = "vertical"
cwd = "tests"
command = ["cargo", "test"]
"#,
        )
        .expect("write");

        let workspace = load_workspace(&path).expect("workspace");
        assert_eq!(workspace.spec.cwd, dir.path().join("repo"));
        assert_eq!(
            workspace.spec.windows[0].cwd,
            dir.path().join("repo").join("frontend")
        );
        assert_eq!(
            workspace.spec.windows[0].root.cwd,
            dir.path().join("repo").join("frontend").join("src")
        );
        assert_eq!(
            workspace.spec.windows[0].splits[0].pane.cwd,
            dir.path().join("repo").join("frontend").join("tests")
        );
    }

    #[test]
    fn rejects_invalid_split_target() {
        let error = load(
            r#"
version = 1

[[windows]]
name = "editor"
root = { command = ["nvim"] }

[[windows.splits]]
target = 1
direction = "vertical"
command = ["cargo", "test"]
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("split target 1"));
    }

    #[test]
    fn rejects_invalid_active_pane() {
        let error = load(
            r#"
version = 1

[[windows]]
name = "editor"
active_pane = 2
root = { command = ["nvim"] }
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("active_pane 2"));
    }

    #[test]
    fn rejects_invalid_ratio() {
        let error = load(
            r#"
version = 1

[[windows]]
name = "editor"
root = { command = ["nvim"] }

[[windows.splits]]
target = 0
direction = "vertical"
size = 1.0
command = ["cargo", "test"]
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("between 0 and 1"));
    }

    #[test]
    fn save_writes_snapshot_sidecar_and_loads_it_back() {
        let dir = tempdir().expect("tempdir");
        let session_dir = dir.path().join("project");
        fs::create_dir_all(&session_dir).expect("session dir");
        let session = Session::new(
            "workspace".into(),
            None,
            Some(session_dir.clone()),
            vec!["sh".into(), "-lc".into(), "printf saved-pane".into()],
            WindowId(1),
            None,
            10_000,
            WindowDefaults::default(),
            dir.path().join("helpers"),
        )
        .expect("session");
        wait_for_preview(&session, "saved-pane");

        let manifest_path = save_workspace(&session, 500).expect("save workspace");
        let snapshot_path = workspace_snapshot_path(&manifest_path);
        let gitignore_path = workspace_state_dir(&manifest_path).join(".gitignore");

        assert!(snapshot_path.exists(), "snapshot sidecar should exist");
        assert!(gitignore_path.exists(), "workspace .gitignore should exist");

        let loaded = load_workspace(&manifest_path).expect("reload workspace");
        let snapshot = loaded.snapshot.expect("snapshot");
        assert_eq!(snapshot.session_name, "workspace");
        assert_eq!(snapshot.windows.len(), 1);
        assert!(snapshot.windows[0].panes[0].vt.contains("saved-pane"));

        let _ = session.kill();
    }

    #[test]
    fn ignores_stale_snapshot_when_manifest_changes() {
        let dir = tempdir().expect("tempdir");
        let session_dir = dir.path().join("project");
        fs::create_dir_all(&session_dir).expect("session dir");
        let session = Session::new(
            "workspace".into(),
            None,
            Some(session_dir.clone()),
            vec!["sh".into(), "-lc".into(), "printf stale-pane".into()],
            WindowId(1),
            None,
            10_000,
            WindowDefaults::default(),
            dir.path().join("helpers"),
        )
        .expect("session");
        wait_for_preview(&session, "stale-pane");
        let manifest_path = save_workspace(&session, 500).expect("save workspace");

        fs::write(
            &manifest_path,
            r#"
version = 1

[workspace]
name = "workspace"

[[windows]]
name = "fresh"
root = { command = ["sh"] }
"#,
        )
        .expect("overwrite manifest");

        let loaded = load_workspace(&manifest_path).expect("load changed workspace");
        assert!(
            loaded.snapshot.is_none(),
            "stale snapshot should be ignored"
        );

        let _ = session.kill();
    }
}
