use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub socket_path: PathBuf,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
}

impl RuntimePaths {
    pub fn resolve() -> Self {
        Self::resolve_from_env(|key| env::var_os(key).map(PathBuf::from))
    }

    pub fn resolve_from_env<F>(mut get_var: F) -> Self
    where
        F: FnMut(&str) -> Option<PathBuf>,
    {
        if let Some(socket_path) = get_var("ADMUX_SOCKET") {
            let config_path =
                get_var("ADMUX_CONFIG").unwrap_or_else(|| PathBuf::from("config.toml"));
            let state_path = get_var("ADMUX_STATE").unwrap_or_else(|| PathBuf::from("state.json"));
            return Self {
                socket_path,
                config_path,
                state_path,
            };
        }

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
            state_path: config_root.join("admux").join("state.json"),
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
        assert_eq!(
            paths.state_path,
            PathBuf::from("/home/test/.config/admux/state.json")
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
        assert_eq!(
            paths.state_path,
            PathBuf::from("/home/tester/.config/admux/state.json")
        );
    }

    #[test]
    fn explicit_socket_override_wins() {
        let env = HashMap::from([("ADMUX_SOCKET", PathBuf::from("/tmp/custom-admux.sock"))]);

        let paths = RuntimePaths::resolve_from_env(|key| env.get(key).cloned());

        assert_eq!(paths.socket_path, PathBuf::from("/tmp/custom-admux.sock"));
        assert_eq!(paths.config_path, PathBuf::from("config.toml"));
        assert_eq!(paths.state_path, PathBuf::from("state.json"));
    }
}
