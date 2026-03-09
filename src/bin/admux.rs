use admux::cli::AdmuxCli;
use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let _ = AdmuxCli::parse();
    Ok(())
}
