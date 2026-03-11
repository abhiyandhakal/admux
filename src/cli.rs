use clap::{ArgGroup, Args, Parser, Subcommand};
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
    ListWindows(SessionArgs),
    ListPanes(TargetArgs),
    Kill(KillArgs),
    KillWindow(TargetStringArgs),
    KillPane(TargetStringArgs),
    SendKeys(SendKeysArgs),
    SplitPane(SplitPaneArgs),
    NewWindow(NewWindowArgs),
    SelectPane(SelectPaneArgs),
    SelectWindow(TargetStringArgs),
    NextWindow(SessionArgs),
    PrevWindow(SessionArgs),
    ResizePane(ResizePaneArgs),
    ReloadConfig,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct NewArgs {
    #[arg(short = 'd', long)]
    pub detach: bool,
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
pub struct SessionArgs {
    pub session: String,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct TargetArgs {
    pub target: String,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct TargetStringArgs {
    pub target: String,
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

#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(group(
    ArgGroup::new("split-axis")
        .required(true)
        .args(["horizontal", "vertical"])
))]
pub struct SplitPaneArgs {
    pub target: String,
    #[arg(long)]
    pub horizontal: bool,
    #[arg(long)]
    pub vertical: bool,
    #[arg(trailing_var_arg = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct NewWindowArgs {
    pub session: String,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(trailing_var_arg = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(group(
    ArgGroup::new("select-pane-mode")
        .required(true)
        .args(["target", "left", "right", "up", "down"])
))]
pub struct SelectPaneArgs {
    pub target: Option<String>,
    #[arg(long)]
    pub left: bool,
    #[arg(long)]
    pub right: bool,
    #[arg(long)]
    pub up: bool,
    #[arg(long)]
    pub down: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(group(
    ArgGroup::new("resize-direction")
        .required(true)
        .args(["left", "right", "up", "down"])
))]
pub struct ResizePaneArgs {
    pub target: String,
    #[arg(long)]
    pub left: bool,
    #[arg(long)]
    pub right: bool,
    #[arg(long)]
    pub up: bool,
    #[arg(long)]
    pub down: bool,
    pub amount: u16,
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
    #[arg(long)]
    pub state: Option<PathBuf>,
    #[arg(long)]
    pub config: Option<PathBuf>,
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
                detach: false,
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
    fn parses_split_pane_command() {
        let cli = AdmuxCli::parse_from(["admux", "split-pane", "work", "--vertical"]);
        assert_eq!(
            cli.command,
            ClientCommand::SplitPane(SplitPaneArgs {
                target: "work".into(),
                horizontal: false,
                vertical: true,
                command: Vec::new(),
            })
        );
    }

    #[test]
    fn parses_daemon_socket_override() {
        let cli = AdmuxdCli::parse_from(["admuxd", "serve", "--socket", "/tmp/admux.sock"]);
        assert_eq!(
            cli.command,
            DaemonCommand::Serve(ServeArgs {
                socket: Some(PathBuf::from("/tmp/admux.sock")),
                state: None,
                config: None,
            })
        );
    }
}
