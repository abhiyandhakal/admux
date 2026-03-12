use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::layout::SplitAxis;

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
    pub spec: WorkspaceSpec,
}

#[derive(Debug, Deserialize)]
struct RawWorkspaceManifest {
    version: u16,
    workspace: Option<RawWorkspaceSettings>,
    #[serde(default)]
    windows: Vec<RawWindowSpec>,
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
    let manifest: RawWorkspaceManifest = toml::from_str(&raw)
        .with_context(|| format!("failed to parse workspace file {}", manifest_path.display()))?;
    resolve_workspace(manifest_path, manifest_dir, manifest)
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
        spec: WorkspaceSpec {
            manifest_path,
            manifest_dir,
            name,
            cwd,
            active_window,
            windows,
        },
    })
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
    use tempfile::tempdir;

    fn load(raw: &str) -> Result<WorkspaceLoad> {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("admux.toml");
        fs::write(&path, raw).expect("write manifest");
        load_workspace(&path)
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
}
