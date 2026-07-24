use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub socket_path: PathBuf,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
    pub aliases_path: PathBuf,
}

impl RuntimePaths {
    pub fn resolve() -> Self {
        Self::resolve_from_env(|key| env::var_os(key).map(PathBuf::from))
    }

    pub fn resolve_from_env<F>(mut get_var: F) -> Self
    where
        F: FnMut(&str) -> Option<PathBuf>,
    {
        let runtime_root = get_var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|| std::env::temp_dir().join(format!("admux-{}", effective_uid())));
        let config_root = get_var("XDG_CONFIG_HOME")
            .or_else(|| get_var("HOME").map(|home| home.join(".config")))
            // Do not put state into an arbitrary caller's working directory.
            // This is an ephemeral fallback when neither XDG nor HOME exists.
            .unwrap_or_else(|| runtime_root.join("config"));

        let defaults = Self {
            socket_path: runtime_root.join("admux").join("socket"),
            config_path: config_root.join("admux").join("config.toml"),
            state_path: config_root.join("admux").join("state.json"),
            aliases_path: config_root.join("admux").join("aliases.json"),
        };

        Self {
            socket_path: get_var("ADMUX_SOCKET").unwrap_or(defaults.socket_path),
            config_path: get_var("ADMUX_CONFIG").unwrap_or(defaults.config_path),
            state_path: get_var("ADMUX_STATE").unwrap_or(defaults.state_path),
            aliases_path: get_var("ADMUX_ALIASES").unwrap_or(defaults.aliases_path),
        }
    }

    pub fn socket_dir(&self) -> &Path {
        self.socket_path
            .parent()
            .expect("socket path should always have a parent")
    }
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn effective_uid() -> u32 {
    0
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
        assert_eq!(
            paths.state_path,
            PathBuf::from("/home/test/.config/admux/state.json")
        );
        assert_eq!(
            paths.aliases_path,
            PathBuf::from("/home/test/.config/admux/aliases.json")
        );
    }

    #[test]
    fn falls_back_to_home_and_tmp_when_xdg_is_missing() {
        let env = HashMap::from([("HOME", PathBuf::from("/home/tester"))]);

        let paths = RuntimePaths::resolve_from_env(|key| env.get(key).cloned());

        assert_eq!(
            paths.socket_path,
            std::env::temp_dir()
                .join(format!("admux-{}", effective_uid()))
                .join("admux/socket")
        );
        assert_eq!(
            paths.config_path,
            PathBuf::from("/home/tester/.config/admux/config.toml")
        );
        assert_eq!(
            paths.state_path,
            PathBuf::from("/home/tester/.config/admux/state.json")
        );
        assert_eq!(
            paths.aliases_path,
            PathBuf::from("/home/tester/.config/admux/aliases.json")
        );
    }

    #[test]
    fn overrides_are_independent_of_one_another() {
        let env = HashMap::from([
            ("XDG_RUNTIME_DIR", PathBuf::from("/run/user/1000")),
            ("XDG_CONFIG_HOME", PathBuf::from("/home/test/.config")),
            ("ADMUX_SOCKET", PathBuf::from("/tmp/custom-admux.sock")),
            ("ADMUX_CONFIG", PathBuf::from("/tmp/custom-config.toml")),
        ]);

        let paths = RuntimePaths::resolve_from_env(|key| env.get(key).cloned());

        assert_eq!(paths.socket_path, PathBuf::from("/tmp/custom-admux.sock"));
        assert_eq!(paths.config_path, PathBuf::from("/tmp/custom-config.toml"));
        assert_eq!(
            paths.state_path,
            PathBuf::from("/home/test/.config/admux/state.json")
        );
        assert_eq!(
            paths.aliases_path,
            PathBuf::from("/home/test/.config/admux/aliases.json")
        );
    }

    #[test]
    fn config_and_state_overrides_work_without_socket_override() {
        let env = HashMap::from([
            ("XDG_RUNTIME_DIR", PathBuf::from("/run/user/1000")),
            ("XDG_CONFIG_HOME", PathBuf::from("/home/test/.config")),
            ("ADMUX_CONFIG", PathBuf::from("/tmp/custom-config.toml")),
            ("ADMUX_STATE", PathBuf::from("/tmp/custom-state.json")),
        ]);

        let paths = RuntimePaths::resolve_from_env(|key| env.get(key).cloned());

        assert_eq!(
            paths.socket_path,
            PathBuf::from("/run/user/1000/admux/socket")
        );
        assert_eq!(paths.config_path, PathBuf::from("/tmp/custom-config.toml"));
        assert_eq!(paths.state_path, PathBuf::from("/tmp/custom-state.json"));
    }

    #[test]
    fn missing_xdg_and_home_never_uses_the_current_directory_for_state() {
        let paths = RuntimePaths::resolve_from_env(|_| None);
        let cwd = std::env::current_dir().expect("current directory");

        assert!(!paths.state_path.starts_with(&cwd));
        assert!(!paths.config_path.starts_with(&cwd));
        assert_eq!(
            paths.config_path,
            std::env::temp_dir()
                .join(format!("admux-{}", effective_uid()))
                .join("config/admux/config.toml")
        );
    }
}
