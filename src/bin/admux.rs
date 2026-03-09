use anyhow::Result;

fn main() -> Result<()> {
    admux::client::run_from_env()
}
