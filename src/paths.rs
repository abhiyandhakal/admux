use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub socket_path: PathBuf,
    pub config_path: PathBuf,
}

impl RuntimePaths {
    pub fn resolve() -> Self {
        Self::resolve_from_env(|key| env::var_os(key).map(PathBuf::from))
    }

    pub fn resolve_from_env<F>(mut get_var: F) -> Self
    where
        F: FnMut(&str) -> Option<PathBuf>,
    {
        let config_root = get_var("XDG_CONFIG_HOME")
            .or_else(|| get_var("HOME").map(|home| home.join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        let runtime_root = get_var("XDG_RUNTIME_DIR").unwrap_or_else(|| {
            let uid = get_var("UID")
                .and_then(|value| value.into_os_string().into_string().ok())
                .unwrap_or_else(|| "unknown".into());
            PathBuf::from(format!("/tmp/admux-{uid}"))
        });

        Self {
            socket_path: runtime_root.join("admux").join("socket"),
            config_path: config_root.join("admux").join("config.toml"),
        }
    }

    pub fn socket_dir(&self) -> &Path {
        self.socket_path
            .parent()
            .expect("socket path should always have a parent")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn prefers_xdg_paths_when_available() {
        let env = HashMap::from([
            ("XDG_RUNTIME_DIR", PathBuf::from("/run/user/1000")),
            ("XDG_CONFIG_HOME", PathBuf::from("/home/test/.config")),
        ]);

        let paths = RuntimePaths::resolve_from_env(|key| env.get(key).cloned());

        assert_eq!(
            paths.socket_path,
            PathBuf::from("/run/user/1000/admux/socket")
        );
        assert_eq!(
            paths.config_path,
            PathBuf::from("/home/test/.config/admux/config.toml")
        );
    }

    #[test]
    fn falls_back_to_home_and_tmp_when_xdg_is_missing() {
        let env = HashMap::from([
            ("HOME", PathBuf::from("/home/tester")),
            ("UID", PathBuf::from("1001")),
        ]);

        let paths = RuntimePaths::resolve_from_env(|key| env.get(key).cloned());

        assert_eq!(
            paths.socket_path,
            PathBuf::from("/tmp/admux-1001/admux/socket")
        );
        assert_eq!(
            paths.config_path,
            PathBuf::from("/home/tester/.config/admux/config.toml")
        );
    }
}
