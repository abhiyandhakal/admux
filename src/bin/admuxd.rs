use admux::cli::AdmuxdCli;
use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let _ = AdmuxdCli::parse();
    Ok(())
}
