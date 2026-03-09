use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRuntime {
    pub root: PathBuf,
}
