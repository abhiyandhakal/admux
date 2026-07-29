use std::{
    collections::BTreeMap,
    env, fs,
    fs::OpenOptions,
    os::unix::{fs::{OpenOptionsExt, PermissionsExt}, io::AsRawFd},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{numbering::Numbering, workspace::load_workspace};

static ALIAS_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        let _writer_lock = lock_alias_writer(path)?;
        self.save_unlocked(path)
    }

    pub fn update<T>(path: &Path, update: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let _writer_lock = lock_alias_writer(path)?;
        let mut registry = Self::load(path)?;
        let result = update(&mut registry)?;
        registry.save_unlocked(path)?;
        Ok(result)
    }

    fn save_unlocked(&self, path: &Path) -> Result<()> {
        let tmp = alias_temporary_path(path)?;
        let raw = serde_json::to_vec_pretty(self).context("failed to encode alias file")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        std::io::Write::write_all(&mut file, &raw)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict permissions on {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", tmp.display()))?;
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

fn alias_temporary_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("alias file {} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create alias directory {}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("alias file {} has no valid file name", path.display()))?;
    let counter = ALIAS_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(".{file_name}.{}.{}.tmp", process::id(), counter)))
}

fn lock_alias_writer(path: &Path) -> Result<fs::File> {
    let lock_path = path.with_extension("json.lock");
    let parent = lock_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("alias lock {} has no parent directory", lock_path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create alias directory {}", parent.display()))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("failed to open alias lock {}", lock_path.display()))?;
    lock.set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to restrict permissions on {}", lock_path.display()))?;
    loop {
        // SAFETY: `lock` stays open while the exclusive advisory lock is held.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(lock);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error)
                .with_context(|| format!("failed to lock alias file {}", path.display()));
        }
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
    use std::{sync::{Arc, Barrier}, thread};
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
    fn alias_temp_paths_are_unique() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("aliases.json");

        let first = alias_temporary_path(&path).expect("first temp path");
        let second = alias_temporary_path(&path).expect("second temp path");

        assert_ne!(first, second);
    }

    #[test]
    fn concurrent_updates_preserve_both_aliases() {
        let dir = tempdir().expect("tempdir");
        let path = Arc::new(dir.path().join("aliases.json"));
        let start = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();

        for name in ["one", "two"] {
            let path = Arc::clone(&path);
            let start = Arc::clone(&start);
            workers.push(thread::spawn(move || {
                start.wait();
                AliasRegistry::update(&path, |registry| {
                    registry.aliases.insert(
                        name.into(),
                        PathBuf::from(format!("/tmp/{name}/admux.toml")),
                    );
                    Ok(())
                })
            }));
        }
        start.wait();
        for worker in workers {
            worker.join().expect("worker panicked").expect("update aliases");
        }

        let registry = AliasRegistry::load(&path).expect("load aliases");
        assert_eq!(registry.aliases.len(), 2);
        assert!(registry.aliases.contains_key("one"));
        assert!(registry.aliases.contains_key("two"));
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
