use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{numbering::Numbering, workspace::load_workspace};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AliasRegistry {
    #[serde(default)]
    pub aliases: BTreeMap<String, PathBuf>,
}

impl AliasRegistry {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read alias file {}", path.display()))?;
        let registry = serde_json::from_str(&raw)
            .with_context(|| format!("failed to decode alias file {}", path.display()))?;
        Ok(registry)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create alias directory {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let raw = serde_json::to_vec_pretty(self).context("failed to encode alias file")?;
        fs::write(&tmp, raw).with_context(|| format!("failed to write {}", tmp.display()))?;
        fs::rename(&tmp, path)
            .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))?;
        Ok(())
    }

    pub fn list(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.aliases
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_path()))
    }

    pub fn resolve(&self, name: &str) -> Option<&Path> {
        self.aliases.get(name).map(PathBuf::as_path)
    }

    pub fn add(
        &mut self,
        name: &str,
        manifest: Option<&Path>,
        numbering: Numbering,
        reserved_names: &[&str],
    ) -> Result<PathBuf> {
        validate_alias_name(name, reserved_names)?;
        if self.aliases.contains_key(name) {
            bail!("alias {name} already exists");
        }
        let manifest = resolve_manifest_path(manifest)?;
        load_workspace(&manifest, numbering)
            .with_context(|| format!("failed to validate workspace alias {}", manifest.display()))?;
        self.aliases.insert(name.to_string(), manifest.clone());
        Ok(manifest)
    }

    pub fn remove(&mut self, name: &str) -> Result<PathBuf> {
        self.aliases
            .remove(name)
            .ok_or_else(|| anyhow::anyhow!("alias {name} does not exist"))
    }
}

fn validate_alias_name(name: &str, reserved_names: &[&str]) -> Result<()> {
    if name.is_empty() {
        bail!("alias name cannot be empty");
    }
    if name.starts_with('-') {
        bail!("alias name cannot start with '-'");
    }
    if reserved_names.contains(&name) {
        bail!("alias name {name} is reserved");
    }
    Ok(())
}

fn resolve_manifest_path(path: Option<&Path>) -> Result<PathBuf> {
    let manifest = match path {
        Some(path) => path.to_path_buf(),
        None => env::current_dir()
            .context("failed to resolve current directory")?
            .join("admux.toml"),
    };
    if !manifest.exists() {
        bail!("workspace manifest {} does not exist", manifest.display());
    }
    manifest
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace manifest {}", manifest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn numbering() -> Numbering {
        Numbering {
            window_base: 1,
            pane_base: 1,
        }
    }

    fn manifest(dir: &Path) -> PathBuf {
        let path = dir.join("admux.toml");
        fs::write(
            &path,
            r#"
version = 1

[workspace]
name = "demo"

[[windows]]
name = "shell"
root = { command = ["sh"] }
"#,
        )
        .expect("write manifest");
        path
    }

    #[test]
    fn roundtrips_alias_registry() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("aliases.json");
        let mut registry = AliasRegistry::default();
        registry
            .aliases
            .insert("demo".into(), PathBuf::from("/tmp/demo/admux.toml"));

        registry.save(&path).expect("save aliases");
        let loaded = AliasRegistry::load(&path).expect("load aliases");

        assert_eq!(loaded, registry);
    }

    #[test]
    fn add_rejects_reserved_names() {
        let dir = tempdir().expect("tempdir");
        let path = manifest(dir.path());
        let mut registry = AliasRegistry::default();

        let error = registry
            .add("up", Some(&path), numbering(), &["up", "help"])
            .expect_err("reserved alias should fail");

        assert!(error.to_string().contains("reserved"));
    }

    #[test]
    fn add_validates_manifest_and_stores_absolute_path() {
        let dir = tempdir().expect("tempdir");
        let path = manifest(dir.path());
        let mut registry = AliasRegistry::default();

        let stored = registry
            .add("demo", Some(&path), numbering(), &["up", "help"])
            .expect("add alias");

        assert!(stored.is_absolute());
        assert_eq!(registry.resolve("demo"), Some(stored.as_path()));
    }

    #[test]
    fn remove_rejects_missing_alias() {
        let mut registry = AliasRegistry::default();
        let error = registry.remove("missing").expect_err("missing alias");
        assert!(error.to_string().contains("does not exist"));
    }

    #[test]
    fn add_without_path_uses_current_directory_manifest() {
        let dir = tempdir().expect("tempdir");
        let current = env::current_dir().expect("current dir");
        let path = manifest(dir.path());
        env::set_current_dir(dir.path()).expect("change dir");

        let mut registry = AliasRegistry::default();
        let stored = registry
            .add("demo", None, numbering(), &["up", "help"])
            .expect("add alias");

        env::set_current_dir(current).expect("restore current dir");
        assert_eq!(stored, path.canonicalize().expect("canonical path"));
    }
}
