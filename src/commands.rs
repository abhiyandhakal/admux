#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractiveCommand {
    SplitWindow {
        horizontal: bool,
    },
    NewWindow,
    SelectWindow {
        target: String,
    },
    NextWindow,
    PreviousWindow,
    KillPane,
    KillWindow,
    AttachSession {
        target: String,
    },
    SwitchClient {
        target: String,
    },
    ListSessions,
    ListWindows,
    ListPanes,
    ListBuffers,
    ShowBuffer {
        buffer: Option<String>,
    },
    DeleteBuffer {
        buffer: Option<String>,
    },
    PasteBuffer {
        buffer: Option<String>,
        target: Option<String>,
    },
    SetBuffer {
        buffer: Option<String>,
        data: String,
    },
    SaveBuffer {
        buffer: Option<String>,
        path: String,
    },
    LoadBuffer {
        buffer: Option<String>,
        path: String,
    },
    ChooseBuffer,
    ChooseTree,
    DetachClient,
    RenameWindow {
        name: String,
    },
    SaveSession,
    SendKeys {
        keys: Vec<String>,
    },
    ReloadConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommandSpec {
    canonical: &'static str,
    aliases: &'static [&'static str],
}

const COMMAND_SPECS: &[CommandSpec] = &[
    CommandSpec {
        canonical: "attach-session",
        aliases: &["attach"],
    },
    CommandSpec {
        canonical: "choose-buffer",
        aliases: &[],
    },
    CommandSpec {
        canonical: "choose-tree",
        aliases: &[],
    },
    CommandSpec {
        canonical: "delete-buffer",
        aliases: &[],
    },
    CommandSpec {
        canonical: "detach-client",
        aliases: &["detach"],
    },
    CommandSpec {
        canonical: "kill-pane",
        aliases: &[],
    },
    CommandSpec {
        canonical: "kill-window",
        aliases: &[],
    },
    CommandSpec {
        canonical: "list-buffers",
        aliases: &[],
    },
    CommandSpec {
        canonical: "list-panes",
        aliases: &[],
    },
    CommandSpec {
        canonical: "list-sessions",
        aliases: &["ls"],
    },
    CommandSpec {
        canonical: "list-windows",
        aliases: &[],
    },
    CommandSpec {
        canonical: "load-buffer",
        aliases: &[],
    },
    CommandSpec {
        canonical: "new-window",
        aliases: &[],
    },
    CommandSpec {
        canonical: "next-window",
        aliases: &[],
    },
    CommandSpec {
        canonical: "paste-buffer",
        aliases: &[],
    },
    CommandSpec {
        canonical: "previous-window",
        aliases: &["prev-window"],
    },
    CommandSpec {
        canonical: "reload-config",
        aliases: &[],
    },
    CommandSpec {
        canonical: "save-buffer",
        aliases: &[],
    },
    CommandSpec {
        canonical: "save-session",
        aliases: &[],
    },
    CommandSpec {
        canonical: "rename-window",
        aliases: &[],
    },
    CommandSpec {
        canonical: "set-buffer",
        aliases: &[],
    },
    CommandSpec {
        canonical: "select-window",
        aliases: &[],
    },
    CommandSpec {
        canonical: "send-keys",
        aliases: &[],
    },
    CommandSpec {
        canonical: "show-buffer",
        aliases: &[],
    },
    CommandSpec {
        canonical: "split-window",
        aliases: &["split-pane"],
    },
    CommandSpec {
        canonical: "switch-client",
        aliases: &[],
    },
];

pub const COMMAND_NAMES: &[&str] = &[
    "attach-session",
    "choose-buffer",
    "choose-tree",
    "delete-buffer",
    "detach-client",
    "list-buffers",
    "kill-pane",
    "kill-window",
    "load-buffer",
    "list-panes",
    "list-sessions",
    "list-windows",
    "new-window",
    "next-window",
    "paste-buffer",
    "previous-window",
    "reload-config",
    "save-buffer",
    "save-session",
    "rename-window",
    "set-buffer",
    "select-window",
    "send-keys",
    "show-buffer",
    "split-window",
    "switch-client",
];

pub fn complete(prefix: &str) -> Vec<&'static str> {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return COMMAND_NAMES.to_vec();
    }

    COMMAND_SPECS
        .iter()
        .filter(|spec| {
            spec.canonical.starts_with(prefix)
                || spec.aliases.iter().any(|alias| alias.starts_with(prefix))
        })
        .map(|spec| spec.canonical)
        .collect()
}

pub fn parse(input: &str) -> Result<InteractiveCommand, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err("empty command".into());
    }

    let command = canonical_name(&tokens[0]).ok_or_else(|| {
        let known = COMMAND_NAMES.join(", ");
        format!("unknown command '{}'; known commands: {known}", tokens[0])
    })?;

    let args = &tokens[1..];
    match command {
        "split-window" => parse_split_window(args),
        "new-window" => parse_no_args(command, args).map(|_| InteractiveCommand::NewWindow),
        "select-window" => parse_required_target(command, args)
            .map(|target| InteractiveCommand::SelectWindow { target }),
        "next-window" => parse_no_args(command, args).map(|_| InteractiveCommand::NextWindow),
        "previous-window" => {
            parse_no_args(command, args).map(|_| InteractiveCommand::PreviousWindow)
        }
        "kill-pane" => parse_no_args(command, args).map(|_| InteractiveCommand::KillPane),
        "kill-window" => parse_no_args(command, args).map(|_| InteractiveCommand::KillWindow),
        "attach-session" => parse_required_target(command, args)
            .map(|target| InteractiveCommand::AttachSession { target }),
        "choose-buffer" => parse_no_args(command, args).map(|_| InteractiveCommand::ChooseBuffer),
        "switch-client" => parse_required_target(command, args)
            .map(|target| InteractiveCommand::SwitchClient { target }),
        "list-buffers" => parse_no_args(command, args).map(|_| InteractiveCommand::ListBuffers),
        "show-buffer" => parse_optional_buffer_arg(command, args)
            .map(|buffer| InteractiveCommand::ShowBuffer { buffer }),
        "delete-buffer" => parse_optional_buffer_arg(command, args)
            .map(|buffer| InteractiveCommand::DeleteBuffer { buffer }),
        "paste-buffer" => parse_paste_buffer(args),
        "set-buffer" => parse_set_buffer(args),
        "save-buffer" => parse_save_or_load_buffer(command, args, true),
        "load-buffer" => parse_save_or_load_buffer(command, args, false),
        "list-sessions" => parse_no_args(command, args).map(|_| InteractiveCommand::ListSessions),
        "list-windows" => parse_no_args(command, args).map(|_| InteractiveCommand::ListWindows),
        "list-panes" => parse_no_args(command, args).map(|_| InteractiveCommand::ListPanes),
        "choose-tree" => parse_no_args(command, args).map(|_| InteractiveCommand::ChooseTree),
        "detach-client" => parse_no_args(command, args).map(|_| InteractiveCommand::DetachClient),
        "reload-config" => parse_no_args(command, args).map(|_| InteractiveCommand::ReloadConfig),
        "save-session" => parse_no_args(command, args).map(|_| InteractiveCommand::SaveSession),
        "rename-window" => parse_rename_window(args),
        "send-keys" => parse_send_keys(args),
        _ => Err(format!("unsupported command '{command}'")),
    }
}

fn canonical_name(input: &str) -> Option<&'static str> {
    COMMAND_SPECS
        .iter()
        .find(|spec| spec.canonical == input || spec.aliases.contains(&input))
        .map(|spec| spec.canonical)
}

fn parse_no_args(command: &str, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("{command} does not accept arguments"))
    }
}

fn parse_split_window(args: &[String]) -> Result<InteractiveCommand, String> {
    let mut horizontal = false;
    let mut seen_h = false;
    let mut seen_v = false;

    for arg in args {
        match arg.as_str() {
            "-h" => {
                seen_h = true;
                horizontal = true;
            }
            "-v" => {
                seen_v = true;
                horizontal = false;
            }
            _ => {
                return Err(format!(
                    "split-window only supports -h or -v in interactive mode, found '{arg}'"
                ));
            }
        }
    }

    if seen_h && seen_v {
        return Err("split-window cannot combine -h and -v".into());
    }

    Ok(InteractiveCommand::SplitWindow { horizontal })
}

fn parse_required_target(command: &str, args: &[String]) -> Result<String, String> {
    match args {
        [] => Err(format!("{command} requires -t <target>")),
        [flag, target] if flag == "-t" => Ok(target.clone()),
        [flag] if flag == "-t" => Err(format!("{command} requires a value after -t")),
        [flag, ..] => Err(format!("{command} expected -t <target>, found '{flag}'")),
    }
}

fn parse_optional_buffer_arg(command: &str, args: &[String]) -> Result<Option<String>, String> {
    match args {
        [] => Ok(None),
        [flag, value] if flag == "-b" => Ok(Some(value.clone())),
        [flag] if flag == "-b" => Err(format!("{command} requires a value after -b")),
        [flag, ..] => Err(format!("{command} expected -b <buffer>, found '{flag}'")),
    }
}

fn parse_paste_buffer(args: &[String]) -> Result<InteractiveCommand, String> {
    let mut buffer = None;
    let mut target = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-b" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("paste-buffer requires a value after -b".into());
                };
                buffer = Some(value.clone());
            }
            "-t" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("paste-buffer requires a value after -t".into());
                };
                target = Some(value.clone());
            }
            other => {
                return Err(format!(
                    "paste-buffer expected -b <buffer> or -t <target>, found '{other}'"
                ));
            }
        }
        index += 1;
    }
    Ok(InteractiveCommand::PasteBuffer { buffer, target })
}

fn parse_set_buffer(args: &[String]) -> Result<InteractiveCommand, String> {
    if args.is_empty() {
        return Err("set-buffer requires text".into());
    }
    if args[0] == "-b" {
        if args.len() < 3 {
            return Err("set-buffer requires -b <buffer> <text>".into());
        }
        return Ok(InteractiveCommand::SetBuffer {
            buffer: Some(args[1].clone()),
            data: args[2..].join(" "),
        });
    }
    Ok(InteractiveCommand::SetBuffer {
        buffer: None,
        data: args.join(" "),
    })
}

fn parse_save_or_load_buffer(
    command: &str,
    args: &[String],
    save: bool,
) -> Result<InteractiveCommand, String> {
    let (buffer, path) = match args {
        [path] => (None, path.clone()),
        [flag, name, path] if flag == "-b" => (Some(name.clone()), path.clone()),
        _ => return Err(format!("{command} expects [ -b <buffer> ] <path>")),
    };
    if save {
        Ok(InteractiveCommand::SaveBuffer { buffer, path })
    } else {
        Ok(InteractiveCommand::LoadBuffer { buffer, path })
    }
}

fn parse_rename_window(args: &[String]) -> Result<InteractiveCommand, String> {
    if args.is_empty() {
        return Err("rename-window requires a name".into());
    }

    Ok(InteractiveCommand::RenameWindow {
        name: args.join(" "),
    })
}

fn parse_send_keys(args: &[String]) -> Result<InteractiveCommand, String> {
    if args.is_empty() {
        return Err("send-keys requires at least one key".into());
    }

    Ok(InteractiveCommand::SendKeys {
        keys: args.to_vec(),
    })
}

fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.trim().chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match quote {
            Some(active) => match ch {
                '\\' => {
                    let escaped = chars
                        .next()
                        .ok_or_else(|| "dangling escape at end of command".to_string())?;
                    current.push(escaped);
                }
                c if c == active => quote = None,
                _ => current.push(ch),
            },
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    let escaped = chars
                        .next()
                        .ok_or_else(|| "dangling escape at end of command".to_string())?;
                    current.push(escaped);
                }
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            },
        }
    }

    if quote.is_some() {
        return Err("unterminated quoted string".into());
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_canonical_names_by_prefix() {
        assert_eq!(complete("sp"), vec!["split-window"]);
    }

    #[test]
    fn completes_canonical_name_for_alias_prefix() {
        assert_eq!(complete("split-p"), vec!["split-window"]);
        assert_eq!(complete("att"), vec!["attach-session"]);
    }

    #[test]
    fn empty_prefix_lists_all_primary_commands() {
        assert_eq!(complete(""), COMMAND_NAMES.to_vec());
    }

    #[test]
    fn parses_split_window_alias() {
        assert_eq!(
            parse("split-pane -h").expect("parse"),
            InteractiveCommand::SplitWindow { horizontal: true }
        );
    }

    #[test]
    fn rejects_conflicting_split_flags() {
        assert_eq!(
            parse("split-window -h -v").unwrap_err(),
            "split-window cannot combine -h and -v"
        );
    }

    #[test]
    fn parses_select_window_target() {
        assert_eq!(
            parse("select-window -t 1").expect("parse"),
            InteractiveCommand::SelectWindow { target: "1".into() }
        );
    }

    #[test]
    fn parses_attach_alias() {
        assert_eq!(
            parse("attach -t work").expect("parse"),
            InteractiveCommand::AttachSession {
                target: "work".into(),
            }
        );
    }

    #[test]
    fn parses_list_sessions_alias() {
        assert_eq!(
            parse("ls").expect("parse"),
            InteractiveCommand::ListSessions
        );
    }

    #[test]
    fn parses_paste_buffer_with_target() {
        assert_eq!(
            parse("paste-buffer -b buffer0002 -t work:1.0").expect("parse"),
            InteractiveCommand::PasteBuffer {
                buffer: Some("buffer0002".into()),
                target: Some("work:1.0".into()),
            }
        );
    }

    #[test]
    fn parses_choose_buffer() {
        assert_eq!(
            parse("choose-buffer").expect("parse"),
            InteractiveCommand::ChooseBuffer
        );
    }

    #[test]
    fn parses_previous_window_alias() {
        assert_eq!(
            parse("prev-window").expect("parse"),
            InteractiveCommand::PreviousWindow
        );
    }

    #[test]
    fn parses_rename_window_with_quotes() {
        assert_eq!(
            parse("rename-window \"editor pane\"").expect("parse"),
            InteractiveCommand::RenameWindow {
                name: "editor pane".into(),
            }
        );
    }

    #[test]
    fn parses_send_keys_with_quoted_arguments() {
        assert_eq!(
            parse("send-keys C-l \"echo hello\" Enter").expect("parse"),
            InteractiveCommand::SendKeys {
                keys: vec!["C-l".into(), "echo hello".into(), "Enter".into()],
            }
        );
    }

    #[test]
    fn parses_reload_config() {
        assert_eq!(
            parse("reload-config").expect("parse"),
            InteractiveCommand::ReloadConfig
        );
    }

    #[test]
    fn parses_save_session() {
        assert_eq!(
            parse("save-session").expect("parse"),
            InteractiveCommand::SaveSession
        );
    }

    #[test]
    fn rejects_missing_target() {
        assert_eq!(
            parse("switch-client").unwrap_err(),
            "switch-client requires -t <target>"
        );
    }

    #[test]
    fn rejects_extra_args_for_no_arg_commands() {
        assert_eq!(
            parse("list-windows now").unwrap_err(),
            "list-windows does not accept arguments"
        );
    }

    #[test]
    fn rejects_unterminated_quotes() {
        assert_eq!(
            parse("rename-window \"oops").unwrap_err(),
            "unterminated quoted string"
        );
    }
}
