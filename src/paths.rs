use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub socket_path: PathBuf,
    pub config_path: PathBuf,
}
