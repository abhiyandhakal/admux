use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, Value};
use sha2::{Digest, Sha256};

use crate::{
    layout::{LayoutNode, SplitAxis},
    numbering::Numbering,
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
    #[serde(default)]
    pub manifest_digest_algorithm: String,
    pub manifest_digest: String,
    pub session_name: String,
    pub active_window: usize,
    pub windows: Vec<WorkspaceWindowSnapshot>,
}

const MANIFEST_DIGEST_ALGORITHM: &str = "sha256";
const MAX_SNAPSHOT_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SNAPSHOT_VT_BYTES: usize = 1024 * 1024;
const MAX_SNAPSHOT_TITLE_BYTES: usize = 1024;
const MAX_SNAPSHOT_ROWS: u16 = 1_000;
const MAX_SNAPSHOT_COLS: u16 = 1_000;
static WORKSPACE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

pub fn load_workspace(path: &Path, numbering: Numbering) -> Result<WorkspaceLoad> {
    load_workspace_with_snapshot(path, numbering, true)
}

pub fn load_workspace_with_snapshot(
    path: &Path,
    numbering: Numbering,
    load_snapshot: bool,
) -> Result<WorkspaceLoad> {
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
    let mut workspace = resolve_workspace(manifest_path.clone(), manifest_dir, manifest, numbering)?;
    workspace.manifest_digest = manifest_digest.clone();
    if load_snapshot {
        let snapshot = load_snapshot_sidecar(&manifest_path, &manifest_digest)?;
        if let Some(snapshot) = snapshot {
            validate_snapshot(&snapshot, &workspace.spec, numbering)?;
            workspace.snapshot = Some(snapshot);
        }
    }
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
    let mut snapshot = export_snapshot(session, &path, "", snapshot_lines)?;
    let manifest = export_workspace(session)?;
    let raw = merge_workspace_manifest(&path, &manifest)?;
    snapshot.manifest_digest = manifest_digest(&raw);
    let snapshot_path = prepare_snapshot_sidecar(&path)?;
    let manifest_tmp = stage_workspace_file(&path, raw.as_bytes())?;
    let snapshot_raw = serde_json::to_vec_pretty(&snapshot)
        .context("failed to encode workspace snapshot")?;
    let snapshot_tmp = match stage_workspace_file(&snapshot_path, &snapshot_raw) {
        Ok(path) => path,
        Err(error) => {
            let _ = fs::remove_file(&manifest_tmp);
            return Err(error);
        }
    };
    commit_workspace_pair(&manifest_tmp, &path, &snapshot_tmp, &snapshot_path)?;
    Ok(path)
}

fn merge_workspace_manifest(path: &Path, manifest: &WorkspaceManifestOut) -> Result<String> {
    let generated_raw =
        toml::to_string_pretty(manifest).context("failed to encode workspace manifest")?;
    let Ok(existing_raw) = fs::read_to_string(path) else {
        return Ok(generated_raw);
    };
    let mut existing = existing_raw
        .parse::<DocumentMut>()
        .with_context(|| format!("failed to parse existing workspace manifest {}", path.display()))?;
    let generated = generated_raw
        .parse::<DocumentMut>()
        .context("failed to parse generated workspace manifest")?;

    merge_manifest_table(existing.as_table_mut(), generated.as_table());
    Ok(existing.to_string())
}

fn merge_manifest_table(existing: &mut Table, generated: &Table) {
    merge_known_table_item(existing, generated, "version");
    merge_workspace_table(existing, generated);
    merge_windows(existing, generated);
}

fn merge_workspace_table(existing: &mut Table, generated: &Table) {
    let Some(generated_workspace) = generated.get("workspace") else {
        existing.remove("workspace");
        return;
    };
    let Some(generated_workspace) = generated_workspace.as_table() else {
        existing.insert("workspace", generated_workspace.clone());
        return;
    };
    match existing.get_mut("workspace").and_then(Item::as_table_mut) {
        Some(existing_workspace) => {
            for key in ["name", "cwd", "active_window"] {
                merge_known_table_item(existing_workspace, generated_workspace, key);
            }
        }
        None => {
            existing.insert("workspace", Item::Table(generated_workspace.clone()));
        }
    }
}

fn merge_windows(existing: &mut Table, generated: &Table) {
    let Some(generated_windows) = generated
        .get("windows")
        .and_then(Item::as_array_of_tables)
    else {
        existing.remove("windows");
        return;
    };
    let Some(existing_windows) = existing
        .get_mut("windows")
        .and_then(Item::as_array_of_tables_mut)
    else {
        existing.insert("windows", Item::ArrayOfTables(generated_windows.clone()));
        return;
    };

    for index in 0..existing_windows.len().min(generated_windows.len()) {
        let existing_window = existing_windows.get_mut(index).expect("window index exists");
        let generated_window = generated_windows.get(index).expect("window index exists");
        merge_window_table(existing_window, generated_window);
    }
    while existing_windows.len() > generated_windows.len() {
        existing_windows.remove(existing_windows.len() - 1);
    }
    for index in existing_windows.len()..generated_windows.len() {
        existing_windows.push(generated_windows.get(index).expect("window index exists").clone());
    }
}

fn merge_window_table(existing: &mut Table, generated: &Table) {
    for key in ["name", "cwd", "active_pane"] {
        merge_known_table_item(existing, generated, key);
    }
    merge_pane_spec(existing, generated, "root");
    merge_splits(existing, generated);
}

fn merge_splits(existing: &mut Table, generated: &Table) {
    let Some(generated_splits) = generated
        .get("splits")
        .and_then(Item::as_array_of_tables)
    else {
        existing.remove("splits");
        return;
    };
    let Some(existing_splits) = existing
        .get_mut("splits")
        .and_then(Item::as_array_of_tables_mut)
    else {
        existing.insert("splits", Item::ArrayOfTables(generated_splits.clone()));
        return;
    };
    for index in 0..existing_splits.len().min(generated_splits.len()) {
        let existing_split = existing_splits.get_mut(index).expect("split index exists");
        let generated_split = generated_splits.get(index).expect("split index exists");
        for key in ["target", "direction", "size", "cwd", "command"] {
            merge_known_table_item(existing_split, generated_split, key);
        }
    }
    while existing_splits.len() > generated_splits.len() {
        existing_splits.remove(existing_splits.len() - 1);
    }
    for index in existing_splits.len()..generated_splits.len() {
        existing_splits.push(generated_splits.get(index).expect("split index exists").clone());
    }
}

fn merge_pane_spec(existing: &mut Table, generated: &Table, key: &str) {
    let Some(generated_item) = generated.get(key) else {
        existing.remove(key);
        return;
    };
    if let Some(generated_table) = generated_item.as_table() {
        match existing.get_mut(key) {
            Some(existing_item) => {
                if let Some(existing_table) = existing_item.as_table_mut() {
                    for field in ["cwd", "command"] {
                        merge_known_table_item(existing_table, generated_table, field);
                    }
                } else if let Some(existing_inline) = existing_item
                    .as_value_mut()
                    .and_then(Value::as_inline_table_mut)
                {
                    for field in ["cwd", "command"] {
                        match generated_table.get(field).and_then(Item::as_value) {
                            Some(generated_value) => match existing_inline.get_mut(field) {
                                Some(existing_value) => {
                                    replace_value_preserving_decor(existing_value, generated_value);
                                }
                                None => {
                                    existing_inline.insert(field, generated_value.clone());
                                }
                            },
                            None => {
                                existing_inline.remove(field);
                            }
                        }
                    }
                } else {
                    *existing_item = generated_item.clone();
                }
            }
            None => {
                existing.insert(key, generated_item.clone());
            }
        }
        return;
    }
    let Some(generated_inline) = generated_item
        .as_value()
        .and_then(Value::as_inline_table)
    else {
        existing.insert(key, generated_item.clone());
        return;
    };
    let Some(existing_inline) = existing
        .get_mut(key)
        .and_then(Item::as_value_mut)
        .and_then(Value::as_inline_table_mut)
    else {
        existing.insert(key, generated_item.clone());
        return;
    };
    for field in ["cwd", "command"] {
        match generated_inline.get(field) {
            Some(generated_value) => match existing_inline.get_mut(field) {
                Some(existing_value) => replace_value_preserving_decor(existing_value, generated_value),
                None => {
                    existing_inline.insert(field, generated_value.clone());
                }
            },
            None => {
                existing_inline.remove(field);
            }
        }
    }
}

fn merge_known_table_item(existing: &mut Table, generated: &Table, key: &str) {
    match generated.get(key) {
        Some(generated_item) => match existing.get_mut(key) {
            Some(existing_item) => match (existing_item.as_value_mut(), generated_item.as_value()) {
                (Some(existing_value), Some(generated_value)) => {
                    replace_value_preserving_decor(existing_value, generated_value);
                }
                _ => *existing_item = generated_item.clone(),
            },
            None => {
                existing.insert(key, generated_item.clone());
            }
        },
        None => {
            existing.remove(key);
        }
    }
}

fn replace_value_preserving_decor(existing: &mut Value, generated: &Value) {
    let decor = existing.decor().clone();
    *existing = generated.clone();
    *existing.decor_mut() = decor;
}

fn resolve_workspace(
    manifest_path: PathBuf,
    manifest_dir: PathBuf,
    manifest: RawWorkspaceManifest,
    numbering: Numbering,
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
    let cwd = resolve_cwd(workspace.cwd.as_ref(), &manifest_dir)?;
    let active_window_public = workspace.active_window.unwrap_or(numbering.window_base as usize);
    let active_window = numbering
        .parse_public_window_number(active_window_public as u64)?
        ;
    if active_window >= manifest.windows.len() {
        bail!(
            "workspace active_window {} is out of range for {} windows",
            active_window_public,
            manifest.windows.len()
        );
    }

    let mut windows = Vec::with_capacity(manifest.windows.len());
    for (window_index, window) in manifest.windows.into_iter().enumerate() {
        let window_cwd = resolve_cwd(window.cwd.as_ref(), &cwd)?;
        let root = resolve_pane_spec(window.root, &window_cwd)?;
        let mut known_panes = 1u64;
        let mut splits = Vec::with_capacity(window.splits.len());
        for split in window.splits {
            let target = numbering.parse_public_pane_number(split.target)?.0;
            if target >= known_panes {
                bail!(
                    "window {} split target {} does not exist yet",
                    window_index,
                    split.target
                );
            }
            let ratio = resolve_ratio(split.size)?;
            splits.push(WorkspaceSplitSpec {
                target,
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

        let active_pane_public = window.active_pane.unwrap_or(numbering.pane_base);
        let active_pane = numbering.parse_public_pane_number(active_pane_public)?.0;
        if active_pane >= known_panes {
            bail!(
                "window {} active_pane {} is out of range for {} panes",
                window_index,
                active_pane_public,
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
        .numbering
        .public_window_number(
            session
                .window_order
                .iter()
                .position(|window_id| *window_id == session.active_window)
                .unwrap_or(0),
        )? as usize;

    let windows = session
        .window_order
        .iter()
        .map(|window_id| {
            let window = session
                .windows
                .get(window_id)
                .ok_or_else(|| anyhow!("missing window {}", window_id.0))?;
            export_window(
                window,
                &session_cwd,
                session.numbering,
            )
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

fn export_window(
    window: &WindowRuntime,
    session_cwd: &Path,
    numbering: Numbering,
) -> Result<WindowSpecOut> {
    let window_base = window.cwd.as_deref().unwrap_or(session_cwd);
    let pane_map = manifest_pane_map(window, numbering);
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
        numbering,
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
    numbering: Numbering,
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
            collect_splits(first, window, window_base, numbering, pane_map, splits)?;
            collect_splits(second, window, window_base, numbering, pane_map, splits)?;
            Ok(target)
        }
    }
}

fn manifest_pane_map(window: &WindowRuntime, numbering: Numbering) -> BTreeMap<PaneId, u64> {
    let mut map = BTreeMap::new();
    map.insert(origin_pane(&window.layout.root), numbering.pane_base);
    let mut next = numbering.pane_base + 1;
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
    let command = pane.command.clone();
    if command.is_empty() {
        bail!("pane {} does not have a stored command", pane.id.0);
    }
    Ok(PaneSpecOut {
        cwd: relativize(pane.cwd.as_deref().unwrap_or(base), base),
        command,
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

fn prepare_snapshot_sidecar(manifest_path: &Path) -> Result<PathBuf> {
    let state_dir = workspace_state_dir(manifest_path);
    fs::create_dir_all(&state_dir)
        .with_context(|| format!("failed to create {}", state_dir.display()))?;
    let gitignore = state_dir.join(".gitignore");
    ensure_snapshot_gitignore(&gitignore)?;
    Ok(workspace_snapshot_path(manifest_path))
}

fn stage_workspace_file(path: &Path, contents: &[u8]) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("workspace file {} has no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("workspace file {} has no valid file name", path.display()))?;
    let counter = WORKSPACE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{file_name}.{}.{}.tmp", process::id(), counter));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;
    file.write_all(contents)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", temporary.display()))?;
    Ok(temporary)
}

fn commit_workspace_file(temporary: &Path, path: &Path) -> Result<()> {
    fs::rename(temporary, path)
        .with_context(|| format!("failed to rename {} to {}", temporary.display(), path.display()))?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("workspace file {} has no parent directory", path.display()))?;
    fs::File::open(parent)
        .with_context(|| format!("failed to open workspace directory {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync workspace directory {}", parent.display()))
}

fn commit_workspace_pair(
    manifest_tmp: &Path,
    manifest_path: &Path,
    snapshot_tmp: &Path,
    snapshot_path: &Path,
) -> Result<()> {
    commit_workspace_pair_with(
        manifest_tmp,
        manifest_path,
        snapshot_tmp,
        snapshot_path,
        commit_workspace_file,
    )
}

fn commit_workspace_pair_with<F>(
    manifest_tmp: &Path,
    manifest_path: &Path,
    snapshot_tmp: &Path,
    snapshot_path: &Path,
    mut commit: F,
) -> Result<()>
where
    F: FnMut(&Path, &Path) -> Result<()>,
{
    if let Err(error) = commit(manifest_tmp, manifest_path) {
        let _ = fs::remove_file(manifest_tmp);
        let _ = fs::remove_file(snapshot_tmp);
        return Err(error);
    }
    if let Err(error) = commit(snapshot_tmp, snapshot_path) {
        let _ = fs::remove_file(snapshot_tmp);
        return Err(error)
            .context("workspace manifest was saved but its snapshot sidecar was not updated");
    }
    Ok(())
}

fn ensure_snapshot_gitignore(gitignore: &Path) -> Result<()> {
    const REQUIRED_RULES: &str = "*\n!.gitignore\n";

    let existing = if gitignore.exists() {
        fs::read_to_string(gitignore)
            .with_context(|| format!("failed to read {}", gitignore.display()))?
    } else {
        String::new()
    };
    let rules = existing.lines().map(str::trim).collect::<Vec<_>>();
    if rules.contains(&"*") && rules.contains(&"!.gitignore") {
        return Ok(());
    }

    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::write(gitignore, format!("{existing}{separator}{REQUIRED_RULES}"))
        .with_context(|| format!("failed to write {}", gitignore.display()))
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
        let pane_map = manifest_pane_map(window, session.numbering);
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
                rows: persistent.rows,
                cols: persistent.cols,
                vt: persistent.vt,
            });
        }
        panes.sort_by_key(|pane| pane.pane_id);
        windows.push(WorkspaceWindowSnapshot {
            window_index: session.numbering.public_window_number(window_index)? as usize,
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
        manifest_digest_algorithm: MANIFEST_DIGEST_ALGORITHM.to_string(),
        manifest_digest: digest.to_string(),
        session_name: session.name.clone(),
        active_window: session.numbering.public_window_number(
            session
                .window_order
                .iter()
                .position(|window_id| *window_id == session.active_window)
                .unwrap_or(0),
        )? as usize,
        windows,
    })
}

fn load_snapshot_sidecar(manifest_path: &Path, digest: &str) -> Result<Option<WorkspaceSnapshot>> {
    let snapshot_path = workspace_snapshot_path(manifest_path);
    if !snapshot_path.exists() {
        return Ok(None);
    }
    let length = fs::metadata(&snapshot_path)
        .with_context(|| format!("failed to inspect {}", snapshot_path.display()))?
        .len();
    if length > MAX_SNAPSHOT_FILE_BYTES {
        bail!(
            "workspace snapshot {} exceeds the {} byte limit",
            snapshot_path.display(),
            MAX_SNAPSHOT_FILE_BYTES
        );
    }
    let raw = fs::read_to_string(&snapshot_path)
        .with_context(|| format!("failed to read {}", snapshot_path.display()))?;
    let snapshot: WorkspaceSnapshot = serde_json::from_str(&raw)
        .with_context(|| format!("failed to decode {}", snapshot_path.display()))?;
    if snapshot.version != 1 {
        return Ok(None);
    }
    if snapshot.manifest_digest_algorithm != MANIFEST_DIGEST_ALGORITHM {
        return Ok(None);
    }
    if snapshot.manifest_digest != digest {
        return Ok(None);
    }
    Ok(Some(snapshot))
}

fn validate_snapshot(
    snapshot: &WorkspaceSnapshot,
    spec: &WorkspaceSpec,
    numbering: Numbering,
) -> Result<()> {
    let expected_windows = spec
        .windows
        .iter()
        .enumerate()
        .map(|(index, window)| {
            let window_index = numbering.public_window_number(index)? as usize;
            let pane_ids = (0..=window.splits.len())
                .map(|pane| numbering.public_pane_number(PaneId(pane as u64)))
                .collect::<Result<Vec<_>>>()?;
            Ok((window_index, pane_ids))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    if snapshot.windows.len() != expected_windows.len() {
        bail!(
            "workspace snapshot has {} windows but manifest has {}",
            snapshot.windows.len(),
            expected_windows.len()
        );
    }
    if !expected_windows.contains_key(&snapshot.active_window) {
        bail!(
            "workspace snapshot active window {} is not in the manifest",
            snapshot.active_window
        );
    }

    let mut seen_windows = BTreeMap::new();
    for window in &snapshot.windows {
        let expected_panes = expected_windows.get(&window.window_index).ok_or_else(|| {
            anyhow!(
                "workspace snapshot window {} is not in the manifest",
                window.window_index
            )
        })?;
        if seen_windows.insert(window.window_index, ()).is_some() {
            bail!("workspace snapshot repeats window {}", window.window_index);
        }
        if window.panes.len() != expected_panes.len() {
            bail!(
                "workspace snapshot window {} has {} panes but manifest has {}",
                window.window_index,
                window.panes.len(),
                expected_panes.len()
            );
        }

        let mut seen_panes = BTreeMap::new();
        for pane in &window.panes {
            if !expected_panes.contains(&pane.pane_id) {
                bail!(
                    "workspace snapshot pane {} is not in manifest window {}",
                    pane.pane_id,
                    window.window_index
                );
            }
            if seen_panes.insert(pane.pane_id, ()).is_some() {
                bail!(
                    "workspace snapshot repeats pane {} in window {}",
                    pane.pane_id,
                    window.window_index
                );
            }
            validate_snapshot_pane(pane)?;
        }
        if !seen_panes.contains_key(&window.active_pane) {
            bail!(
                "workspace snapshot active pane {} is not in window {}",
                window.active_pane,
                window.window_index
            );
        }
    }
    Ok(())
}

fn validate_snapshot_pane(pane: &WorkspacePaneSnapshot) -> Result<()> {
    if pane.rows == 0 || pane.rows > MAX_SNAPSHOT_ROWS || pane.cols == 0 || pane.cols > MAX_SNAPSHOT_COLS {
        bail!(
            "workspace snapshot pane {} has invalid dimensions {}x{}",
            pane.pane_id,
            pane.cols,
            pane.rows
        );
    }
    if pane.vt.len() > MAX_SNAPSHOT_VT_BYTES {
        bail!("workspace snapshot pane {} VT data is too large", pane.pane_id);
    }
    if pane.title.len() > MAX_SNAPSHOT_TITLE_BYTES {
        bail!("workspace snapshot pane {} title is too large", pane.pane_id);
    }
    Ok(())
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
    format!("{:x}", Sha256::digest(raw.as_bytes()))
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
        cwd: resolve_cwd(raw.cwd.as_ref(), base)?,
        command: raw.command,
    })
}

fn resolve_cwd(path: Option<&PathBuf>, base: &Path) -> Result<PathBuf> {
    let candidate = match path {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => base.join(path),
        None => base.to_path_buf(),
    };
    let metadata = fs::metadata(&candidate)
        .with_context(|| format!("workspace cwd {} does not exist", candidate.display()))?;
    if !metadata.is_dir() {
        bail!("workspace cwd {} is not a directory", candidate.display());
    }
    candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace cwd {}", candidate.display()))
}

fn resolve_ratio(value: Option<f32>) -> Result<u16> {
    let value = value.unwrap_or(0.5);
    if !(0.1..=0.9).contains(&value) {
        bail!("workspace split size must be between 0.1 and 0.9");
    }
    let ratio = (value * 1000.0).round() as u16;
    Ok(ratio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::WindowDefaults,
        layout::SplitAxis,
        numbering::Numbering,
        pane::WindowId,
        session::Session,
    };
    use std::{thread, time::Duration};
    use tempfile::{TempDir, tempdir as make_tempdir};

    fn tempdir() -> TempDir {
        make_tempdir().expect("tempdir")
    }

    fn load(raw: &str) -> Result<WorkspaceLoad> {
        let dir = tempdir();
        let path = dir.path().join("admux.toml");
        fs::write(&path, raw).expect("write manifest");
        load_workspace(
            &path,
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
        )
    }

    fn wait_for_preview(session: &Session, needle: &str) {
        for _ in 0..50 {
            let pane = session
                .active_window()
                .and_then(|window| window.panes.get(&window.layout.active))
                .expect("active pane");
            let preview = pane
                .process
                .render(80, 24)
                .expect("render active pane")
                .preview;
            if preview.contains(needle) {
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
        let dir = tempdir();
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
        fs::create_dir_all(dir.path().join("repo/frontend/src")).expect("create root cwd");
        fs::create_dir_all(dir.path().join("repo/frontend/tests")).expect("create split cwd");

        let workspace = load_workspace(
            &path,
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
        )
        .expect("workspace");
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
    fn rejects_workspace_cwds_that_are_missing_or_not_directories() {
        let missing = load(
            r#"
version = 1
[workspace]
cwd = "missing"
[[windows]]
name = "editor"
root = { command = ["nvim"] }
"#,
        )
        .expect_err("missing cwd rejected");
        assert!(missing.to_string().contains("does not exist"));

        let dir = tempdir();
        let file = dir.path().join("not-a-directory");
        fs::write(&file, "file").expect("write file");
        let error = resolve_cwd(Some(&file), dir.path()).expect_err("file cwd rejected");
        assert!(error.to_string().contains("not a directory"));
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

        assert!(error.to_string().contains("between 0.1 and 0.9"));
    }

    #[test]
    fn rejects_ratios_that_would_be_silently_clamped() {
        let error = load(
            r#"
version = 1

[[windows]]
name = "editor"
root = { command = ["nvim"] }

[[windows.splits]]
target = 0
direction = "vertical"
size = 0.01
command = ["cargo", "test"]
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("between 0.1 and 0.9"));
    }

    #[test]
    fn existing_snapshot_gitignore_is_amended_to_protect_snapshot_contents() {
        let dir = tempdir();
        let gitignore = dir.path().join(".gitignore");
        fs::write(&gitignore, "# retain this comment\n!.keep")
            .expect("write existing gitignore");

        ensure_snapshot_gitignore(&gitignore).expect("amend gitignore");

        assert_eq!(
            fs::read_to_string(&gitignore).expect("read gitignore"),
            "# retain this comment\n!.keep\n*\n!.gitignore\n"
        );
    }

    #[test]
    fn manifest_digest_uses_identified_sha256() {
        assert_eq!(
            manifest_digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn save_writes_snapshot_sidecar_and_loads_it_back() {
        let dir = tempdir();
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
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
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
        let mut snapshot_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&snapshot_path).expect("read snapshot"),
        )
        .expect("decode snapshot");
        assert_eq!(
            snapshot_json["manifest_digest_algorithm"],
            MANIFEST_DIGEST_ALGORITHM
        );

        let loaded = load_workspace(
            &manifest_path,
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
        )
        .expect("reload workspace");
        let snapshot = loaded.snapshot.expect("snapshot");
        assert_eq!(snapshot.session_name, "workspace");
        assert_eq!(snapshot.windows.len(), 1);
        assert!(snapshot.windows[0].panes[0].vt.contains("saved-pane"));

        snapshot_json["windows"][0]["panes"][0]["rows"] = serde_json::json!(0);
        fs::write(
            &snapshot_path,
            serde_json::to_vec(&snapshot_json).expect("encode invalid snapshot"),
        )
        .expect("write invalid snapshot");
        let invalid = load_workspace(
            &manifest_path,
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
        )
        .expect_err("invalid snapshot should be rejected before restore");
        assert!(invalid.to_string().contains("invalid dimensions"));

        let _ = session.kill();
    }

    #[test]
    fn save_preserves_existing_comments_formatting_and_unknown_manifest_fields() {
        let dir = tempdir();
        let session_dir = dir.path().join("project");
        fs::create_dir_all(&session_dir).expect("session dir");
        let manifest_path = session_dir.join("admux.toml");
        fs::write(
            &manifest_path,
            r#"# retained document comment
version = 1 # retained version comment
custom_root = "keep-root"

[workspace]
# retained workspace comment
name = "old-name" # retained workspace value comment
custom_workspace = "keep-workspace"

[[windows]]
# retained window comment
name = "old-window"
custom_window = "keep-window"
root = { command = ["old"], custom_root_pane = "keep-pane" }
"#,
        )
        .expect("write existing manifest");
        let session = Session::new(
            "workspace".into(),
            None,
            Some(session_dir),
            vec!["sh".into()],
            WindowId(1),
            None,
            10_000,
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
            WindowDefaults::default(),
            dir.path().join("helpers"),
        )
        .expect("session");

        let saved_path = save_workspace(&session, 500).expect("save workspace");
        assert_eq!(saved_path, manifest_path);
        let saved = fs::read_to_string(&saved_path).expect("read saved manifest");
        for retained in [
            "# retained document comment",
            "version = 1 # retained version comment",
            "custom_root = \"keep-root\"",
            "# retained workspace comment",
            "custom_workspace = \"keep-workspace\"",
            "# retained window comment",
            "custom_window = \"keep-window\"",
            "custom_root_pane = \"keep-pane\"",
        ] {
            assert!(saved.contains(retained), "missing retained manifest content: {retained}");
        }
        let parsed: toml::Value = toml::from_str(&saved).expect("saved manifest stays valid TOML");
        assert_eq!(parsed["custom_root"].as_str(), Some("keep-root"));
        assert_eq!(
            parsed["workspace"]["custom_workspace"].as_str(),
            Some("keep-workspace")
        );
        assert_eq!(
            parsed["windows"][0]["root"]["custom_root_pane"].as_str(),
            Some("keep-pane")
        );

        session.kill().expect("clean up session");
    }

    #[test]
    fn ignores_stale_snapshot_when_manifest_changes() {
        let dir = tempdir();
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
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
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

        let loaded = load_workspace(
            &manifest_path,
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
        )
        .expect("load changed workspace");
        assert!(
            loaded.snapshot.is_none(),
            "stale snapshot should be ignored"
        );

        let _ = session.kill();
    }

    #[test]
    fn save_preserves_declared_command_without_foreground_arguments() {
        let dir = tempdir();
        let session_dir = dir.path().join("project");
        fs::create_dir_all(&session_dir).expect("session dir");
        let session = Session::new(
            "workspace".into(),
            None,
            Some(session_dir.clone()),
            vec!["sh".into(), "-lc".into(), "exec sleep 1".into()],
            WindowId(1),
            None,
            10_000,
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
            WindowDefaults::default(),
            dir.path().join("helpers"),
        )
        .expect("session");

        let manifest_path = save_workspace(&session, 500).expect("save workspace");
        let raw = fs::read_to_string(manifest_path).expect("read manifest");
        assert!(
            raw.contains("\"exec sleep 1\""),
            "saved manifest should retain the declared command"
        );
        assert!(
            !fs::read_to_string(workspace_snapshot_path(&session_dir.join("admux.toml")))
                .expect("read snapshot")
                .contains("command"),
            "snapshot should not persist helper-reported commands"
        );

        let _ = session.kill();
    }

    #[test]
    fn save_failure_before_snapshot_staging_preserves_existing_manifest() {
        let dir = tempdir();
        let session_dir = dir.path().join("project");
        fs::create_dir_all(&session_dir).expect("session dir");
        let session = Session::new(
            "workspace".into(),
            None,
            Some(session_dir.clone()),
            vec!["sh".into(), "-lc".into(), "sleep 1".into()],
            WindowId(1),
            None,
            10_000,
            Numbering {
                window_base: 0,
                pane_base: 0,
            },
            WindowDefaults::default(),
            dir.path().join("helpers"),
        )
        .expect("session");
        let manifest_path = session_dir.join("admux.toml");
        let original = "# user-maintained manifest\nversion = 1\n";
        fs::write(&manifest_path, original).expect("write manifest");
        fs::write(session_dir.join(".admux"), "not a directory")
            .expect("block snapshot directory");

        let error = save_workspace(&session, 500).expect_err("save should fail");

        assert!(error.to_string().contains("failed to create"));
        assert_eq!(fs::read_to_string(&manifest_path).expect("read manifest"), original);

        let _ = session.kill();
    }

    #[test]
    fn failed_manifest_commit_preserves_the_previous_snapshot() {
        let dir = tempdir();
        let manifest_path = dir.path().join("admux.toml");
        let snapshot_path = dir.path().join("snapshot.json");
        fs::write(&manifest_path, "version = 1\n").expect("write manifest");
        fs::write(&snapshot_path, "previous snapshot").expect("write snapshot");
        let manifest_tmp = stage_workspace_file(&manifest_path, b"version = 2\n")
            .expect("stage manifest");
        let snapshot_tmp = stage_workspace_file(&snapshot_path, b"new snapshot")
            .expect("stage snapshot");

        let error = commit_workspace_pair_with(
            &manifest_tmp,
            &manifest_path,
            &snapshot_tmp,
            &snapshot_path,
            |_, path| {
                if path == manifest_path {
                    bail!("injected manifest commit failure");
                }
                Ok(())
            },
        )
        .expect_err("manifest commit should fail");

        assert!(error.to_string().contains("injected manifest"));
        assert_eq!(fs::read_to_string(&snapshot_path).expect("read snapshot"), "previous snapshot");
        assert!(!manifest_tmp.exists());
        assert!(!snapshot_tmp.exists());
    }

    #[test]
    fn save_and_load_workspace_uses_configured_public_numbering() {
        let dir = tempdir();
        let session_dir = dir.path().join("project");
        fs::create_dir_all(&session_dir).expect("session dir");
        let numbering = Numbering {
            window_base: 1,
            pane_base: 2,
        };
        let mut session = Session::new(
            "workspace".into(),
            None,
            Some(session_dir.clone()),
            vec!["sh".into()],
            WindowId(1),
            None,
            10_000,
            numbering,
            WindowDefaults::default(),
            dir.path().join("helpers"),
        )
        .expect("session");
        session
            .split_active_pane(SplitAxis::Vertical, &["sh".into()])
            .expect("split");

        let manifest_path = save_workspace(&session, 50).expect("save workspace");
        let raw = fs::read_to_string(&manifest_path).expect("read manifest");

        assert!(raw.contains("active_window = 1"));
        assert!(raw.contains("active_pane = 3"));
        assert!(raw.contains("target = 2"));

        let loaded = load_workspace(&manifest_path, numbering).expect("load workspace");
        assert_eq!(loaded.spec.active_window, 0);
        assert_eq!(loaded.spec.windows[0].active_pane, 1);
        assert_eq!(loaded.spec.windows[0].splits[0].target, 0);

        let _ = session.kill();
    }
}
