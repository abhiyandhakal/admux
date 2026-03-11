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
            let paths = admux::paths::RuntimePaths::resolve();
            let socket = args.socket.unwrap_or(paths.socket_path);
            let state = args.state.unwrap_or(paths.state_path);
            let config = args.config.unwrap_or(paths.config_path);
            server::serve(&socket, &state, &config)
        }
    }
}
