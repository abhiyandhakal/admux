use admux::pty::{PaneHelperArgs, run_helper};
use anyhow::{Context, Result};

fn main() -> Result<()> {
    let payload = std::env::args()
        .nth(1)
        .context("missing pane helper payload")?;
    let args: PaneHelperArgs =
        serde_json::from_str(&payload).context("failed to decode pane helper payload")?;
    run_helper(args)
}
