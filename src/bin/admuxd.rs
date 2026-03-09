use admux::{
    cli::{AdmuxdCli, DaemonCommand},
    server,
};
use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let cli = AdmuxdCli::parse();
    match cli.command {
        DaemonCommand::Serve(args) => {
            let socket = args
                .socket
                .unwrap_or_else(|| admux::paths::RuntimePaths::resolve().socket_path);
            server::serve(&socket)
        }
    }
}
