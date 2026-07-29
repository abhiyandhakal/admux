use admux::pty::{PaneHelperArgs, run_helper};
use anyhow::{Context, Result};
use std::fs;

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let first = arguments.next().context("missing pane helper payload")?;
    let payload = if first == "--args-file" {
        let path = arguments
            .next()
            .context("missing pane helper args-file path")?;
        let payload = fs::read_to_string(&path)
            .with_context(|| format!("failed to read pane helper args file {path}"));
        let cleanup = fs::remove_file(&path)
            .with_context(|| format!("failed to remove pane helper args file {path}"));
        let payload = payload?;
        cleanup?;
        payload
    } else {
        first
    };
    if arguments.next().is_some() {
        anyhow::bail!("unexpected pane helper arguments");
    }
    let args: PaneHelperArgs =
        serde_json::from_str(&payload).context("failed to decode pane helper payload")?;
    run_helper(args)
}
