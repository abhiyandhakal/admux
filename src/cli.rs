use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "admux", about = "Opinionated terminal multiplexer client")]
pub struct AdmuxCli {
    #[command(subcommand)]
    pub command: ClientCommand,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum ClientCommand {
    New(NewArgs),
    Attach(AttachArgs),
    Ls,
    Kill(KillArgs),
    SendKeys(SendKeysArgs),
    ReloadConfig,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct NewArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[arg(trailing_var_arg = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct AttachArgs {
    pub session: Option<String>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct KillArgs {
    pub session: String,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SendKeysArgs {
    pub target: String,
    #[arg(required = true)]
    pub keys: Vec<String>,
}

#[derive(Debug, Parser)]
#[command(name = "admuxd", about = "Opinionated terminal multiplexer daemon")]
pub struct AdmuxdCli {
    #[command(subcommand)]
    pub command: DaemonCommand,
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum DaemonCommand {
    Serve(ServeArgs),
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ServeArgs {
    #[arg(long)]
    pub socket: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_new_command() {
        let cli = AdmuxCli::parse_from(["admux", "new", "--name", "work", "--", "bash"]);
        assert_eq!(
            cli.command,
            ClientCommand::New(NewArgs {
                name: Some("work".into()),
                cwd: None,
                command: vec!["bash".into()],
            })
        );
    }

    #[test]
    fn parses_attach_without_session() {
        let cli = AdmuxCli::parse_from(["admux", "attach"]);
        assert_eq!(
            cli.command,
            ClientCommand::Attach(AttachArgs { session: None })
        );
    }

    #[test]
    fn parses_daemon_socket_override() {
        let cli = AdmuxdCli::parse_from(["admuxd", "serve", "--socket", "/tmp/admux.sock"]);
        assert_eq!(
            cli.command,
            DaemonCommand::Serve(ServeArgs {
                socket: Some(PathBuf::from("/tmp/admux.sock")),
            })
        );
    }
}
