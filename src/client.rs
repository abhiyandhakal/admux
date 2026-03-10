use std::{
    collections::BTreeSet,
    io::{self, IsTerminal, Read, Write},
    os::unix::net::UnixStream,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::Parser;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::{
    cli::{AdmuxCli, ClientCommand, NewWindowArgs, ResizePaneArgs, SelectPaneArgs, SplitPaneArgs},
    commands::{InteractiveCommand, complete as complete_commands, parse as parse_command},
    copy_mode::Selection,
    input::{InputAction, InputState},
    ipc::{
        CommandRequest, CommandResponse, CycleDirection, NavigationDirection, PaneCursor,
        PaneRender, RenderSnapshot, SwitchSource,
    },
    layout::SplitAxis,
    pane::Rect,
    paths::RuntimePaths,
    render::{
        BottomBar, PaneSelection, TerminalSize, TreeLine, render_choose_tree, render_help_overlay,
        render_session,
    },
    window::WindowSummary,
};

#[derive(Debug, Clone)]
struct PromptState {
    buffer: String,
    cursor: usize,
    completions: Vec<String>,
    selected: usize,
    history_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChooseItem {
    Session(String),
    Window {
        session: String,
        window_id: u64,
    },
    Pane {
        session: String,
        window_id: u64,
        pane_id: u64,
    },
}

#[derive(Debug, Clone)]
struct ChooseTreeState {
    items: Vec<ChooseItem>,
    lines: Vec<TreeLine>,
    selected: usize,
    expanded_sessions: BTreeSet<String>,
    expanded_windows: BTreeSet<(String, u64)>,
    attached_session: String,
    search_input: Option<String>,
    last_search: Option<String>,
}

#[derive(Debug, Clone)]
enum OverlayState {
    None,
    Prompt(PromptState),
    ChooseTree(ChooseTreeState),
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptResult {
    KeepOpen,
    Close,
    CloseAndClearSelection,
}

#[derive(Debug, Clone, Copy)]
struct SelectionAnchor {
    pane_id: u64,
    row: u16,
    col: u16,
}

#[derive(Debug, Clone, Copy)]
struct ResizeDrag {
    pane_id: u64,
    direction: NavigationDirection,
    last_row: u16,
    last_col: u16,
}

pub fn run_from_env() -> Result<()> {
    let cli = AdmuxCli::parse();
    run(cli)
}

pub fn run(cli: AdmuxCli) -> Result<()> {
    let paths = RuntimePaths::resolve();
    let request = match cli.command {
        ClientCommand::New(args) => {
            let requested_name = args.name.clone();
            let nested_switch = (!args.detach
                && io::stdout().is_terminal()
                && std::env::var_os("ADMUX_NONINTERACTIVE").is_none())
            .then(nested_switch_source)
            .flatten();
            let response = request_response(
                &paths,
                CommandRequest::NewSession {
                    name: args.name,
                    cwd: args.cwd,
                    command: args.command,
                    switch_from: nested_switch.clone(),
                },
            )?;
            let created_session = match &response {
                CommandResponse::SessionCreated { session, .. } => Some(session.clone()),
                _ => None,
            };
            if nested_switch.is_none() {
                print_response(&paths, response)?;
            }

            if !args.detach
                && io::stdout().is_terminal()
                && std::env::var_os("ADMUX_NONINTERACTIVE").is_none()
                && nested_switch.is_none()
            {
                let session = created_session.or(requested_name).ok_or_else(|| {
                    anyhow!("new session response did not include a session name")
                })?;
                attach_interactive(&paths, &session)?;
            }
            return Ok(());
        }
        ClientCommand::Attach(args) => CommandRequest::Attach {
            session: args.session,
        },
        ClientCommand::Ls => CommandRequest::ListSessions,
        ClientCommand::ListWindows(args) => CommandRequest::ListWindows {
            session: args.session,
        },
        ClientCommand::ListPanes(args) => CommandRequest::ListPanes {
            target: args.target,
        },
        ClientCommand::Kill(args) => CommandRequest::KillSession {
            session: args.session,
        },
        ClientCommand::KillWindow(args) => CommandRequest::KillWindow {
            target: args.target,
        },
        ClientCommand::KillPane(args) => CommandRequest::KillPane {
            target: args.target,
        },
        ClientCommand::SendKeys(args) => CommandRequest::SendKeys {
            target: args.target,
            keys: args.keys,
        },
        ClientCommand::SplitPane(args) => split_pane_request(args),
        ClientCommand::NewWindow(NewWindowArgs {
            session,
            name,
            command,
        }) => CommandRequest::NewWindow {
            session,
            name,
            command,
        },
        ClientCommand::SelectPane(args) => select_pane_request(args),
        ClientCommand::SelectWindow(args) => CommandRequest::SelectWindow {
            target: args.target,
        },
        ClientCommand::NextWindow(args) => CommandRequest::CycleWindow {
            session: args.session,
            direction: CycleDirection::Next,
        },
        ClientCommand::PrevWindow(args) => CommandRequest::CycleWindow {
            session: args.session,
            direction: CycleDirection::Prev,
        },
        ClientCommand::ResizePane(args) => resize_pane_request(args),
        ClientCommand::ReloadConfig => CommandRequest::ReloadConfig,
    };

    let response = request_response(&paths, request)?;
    print_response(&paths, response)
}

fn nested_switch_source() -> Option<SwitchSource> {
    let session = std::env::var("ADMUX_SESSION").ok()?;
    let pane_id = std::env::var("ADMUX_PANE").ok()?.parse().ok()?;
    Some(SwitchSource { session, pane_id })
}

pub fn request_response(paths: &RuntimePaths, request: CommandRequest) -> Result<CommandResponse> {
    let response = with_connection(paths, |stream| {
        write_message(stream, &request)?;
        read_message(stream)
    })?;
    Ok(response)
}

fn split_pane_request(args: SplitPaneArgs) -> CommandRequest {
    CommandRequest::SplitPane {
        target: args.target,
        axis: if args.vertical {
            SplitAxis::Vertical
        } else {
            SplitAxis::Horizontal
        },
        command: args.command,
    }
}

fn select_pane_request(args: SelectPaneArgs) -> CommandRequest {
    let direction = if args.left {
        Some(NavigationDirection::Left)
    } else if args.right {
        Some(NavigationDirection::Right)
    } else if args.up {
        Some(NavigationDirection::Up)
    } else if args.down {
        Some(NavigationDirection::Down)
    } else {
        None
    };
    CommandRequest::SelectPane {
        target: args.target,
        direction,
    }
}

fn resize_pane_request(args: ResizePaneArgs) -> CommandRequest {
    let direction = if args.left {
        NavigationDirection::Left
    } else if args.right {
        NavigationDirection::Right
    } else if args.up {
        NavigationDirection::Up
    } else {
        NavigationDirection::Down
    };
    CommandRequest::ResizePane {
        target: args.target,
        direction,
        amount: args.amount,
    }
}

fn with_connection<T>(
    paths: &RuntimePaths,
    mut f: impl FnMut(&mut UnixStream) -> Result<T>,
) -> Result<T> {
    match UnixStream::connect(&paths.socket_path) {
        Ok(mut stream) => f(&mut stream),
        Err(_) => {
            spawn_daemon(paths)?;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match UnixStream::connect(&paths.socket_path) {
                    Ok(mut stream) => return f(&mut stream),
                    Err(error) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(50));
                        let _ = error;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "failed to connect to admuxd at {} after autostart",
                                paths.socket_path.display()
                            )
                        });
                    }
                }
            }
        }
    }
}

fn spawn_daemon(paths: &RuntimePaths) -> Result<()> {
    let daemon_path = resolve_daemon_binary()?;
    let socket = paths.socket_path.display().to_string();
    Command::new(daemon_path)
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn admuxd")?;
    Ok(())
}

fn resolve_daemon_binary() -> Result<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("ADMUXD_BIN") {
        return Ok(path.into());
    }

    let current = std::env::current_exe().context("failed to resolve current executable path")?;
    let daemon = current.with_file_name("admuxd");
    if daemon.exists() {
        Ok(daemon)
    } else {
        bail!(
            "could not locate admuxd binary next to {}",
            current.display()
        )
    }
}

fn write_message(stream: &mut UnixStream, request: &CommandRequest) -> Result<()> {
    let payload = serde_json::to_vec(request).context("failed to encode request")?;
    stream
        .write_all(&payload)
        .context("failed to write request payload")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to finish request")?;
    Ok(())
}

fn read_message(stream: &mut UnixStream) -> Result<CommandResponse> {
    let mut payload = Vec::new();
    stream
        .read_to_end(&mut payload)
        .context("failed to read response payload")?;
    let response = serde_json::from_slice(&payload).context("failed to decode response")?;
    Ok(response)
}

fn print_response(paths: &RuntimePaths, response: CommandResponse) -> Result<()> {
    match response {
        CommandResponse::HelloAck { version } => {
            println!("protocol {}", version.0);
        }
        CommandResponse::SessionCreated { session, pane_id } => {
            println!("created {session} pane {pane_id}");
        }
        CommandResponse::WindowCreated {
            session,
            window_id,
            pane_id,
        } => {
            println!("created {session}:{window_id} pane {pane_id}");
        }
        CommandResponse::PaneSplit {
            session,
            window_id,
            pane_id,
        } => {
            println!("split {session}:{window_id} pane {pane_id}");
        }
        CommandResponse::Attached {
            session,
            preview,
            formatted_preview,
            snapshot,
            ..
        } => {
            if io::stdout().is_terminal() && std::env::var_os("ADMUX_NONINTERACTIVE").is_none() {
                attach_interactive(paths, &session)?;
            } else {
                println!("attached {session}");
                if io::stdout().is_terminal() {
                    if !formatted_preview.is_empty() {
                        print!("{formatted_preview}");
                    }
                } else if !preview.is_empty() {
                    print!("{preview}");
                } else if let Some(snapshot) = snapshot {
                    for pane in snapshot.panes {
                        for row in pane.rows_plain {
                            println!("{row}");
                        }
                    }
                }
            }
        }
        CommandResponse::SessionList { sessions } => {
            for session in sessions {
                println!("{session}");
            }
        }
        CommandResponse::WindowList { windows } => {
            for window in windows {
                let marker = if window.active { "*" } else { " " };
                println!("{marker} {} {}", window.id, window.name);
            }
        }
        CommandResponse::PaneList { panes } => {
            for pane in panes {
                let marker = if pane.active { "*" } else { " " };
                println!("{marker} {} {} ({})", pane.id, pane.title, pane.window_id);
            }
        }
        CommandResponse::SessionKilled { session } => println!("killed {session}"),
        CommandResponse::WindowKilled { session, window_id } => {
            println!("killed {session}:{window_id}");
        }
        CommandResponse::PaneKilled {
            session,
            window_id,
            pane_id,
        } => {
            println!("killed {session}:{window_id}.{pane_id}");
        }
        CommandResponse::KeysSent => println!("keys sent"),
        CommandResponse::SelectionCopied { .. }
        | CommandResponse::Scrolled
        | CommandResponse::Resized
        | CommandResponse::FocusChanged => {}
        CommandResponse::ConfigReloaded => println!("config reloaded"),
        CommandResponse::Error { message } => return Err(anyhow!(message)),
    }
    Ok(())
}

fn attach_interactive(paths: &RuntimePaths, session: &str) -> Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode().context("failed to enable raw mode")?;
    execute!(
        stdout,
        EnterAlternateScreen,
        Hide,
        EnableMouseCapture,
        EnableBracketedPaste
    )
    .context("failed to enter alternate screen")?;

    let result = run_attach_loop(paths, session.to_string(), &mut stdout);

    let _ = execute!(
        stdout,
        DisableBracketedPaste,
        DisableMouseCapture,
        Show,
        LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
    result
}

fn run_attach_loop(
    paths: &RuntimePaths,
    mut current_session: String,
    stdout: &mut impl Write,
) -> Result<()> {
    let mut state = InputState::default();
    let mut last_size = (0, 0);
    let mut selection_anchor: Option<SelectionAnchor> = None;
    let mut active_selection: Option<PaneSelection> = None;
    let mut resize_drag: Option<ResizeDrag> = None;
    let mut status_message: Option<String> = None;
    let mut prompt_history = Vec::<String>::new();
    let mut overlay = OverlayState::None;

    loop {
        let (width, height) = terminal::size().context("failed to read terminal size")?;
        let rows = height.max(1);
        let cols = width.max(1);
        if last_size != (rows, cols) {
            let _ = request_response(
                paths,
                CommandRequest::Resize {
                    session: current_session.clone(),
                    rows,
                    cols,
                },
            )?;
            last_size = (rows, cols);
        }

        let response = request_response(
            paths,
            CommandRequest::Attach {
                session: Some(current_session.clone()),
            },
        )?;
        let snapshot = match response {
            CommandResponse::Attached {
                preview, snapshot, ..
            } => snapshot.unwrap_or_else(|| fallback_snapshot(preview, width, height)),
            CommandResponse::Error { message } => return Err(anyhow!(message)),
            other => return Err(anyhow!("unexpected attach response: {other:?}")),
        };

        match &overlay {
            OverlayState::None => {
                render_session(
                    stdout,
                    &current_session,
                    &snapshot,
                    BottomBar::Status {
                        message: status_message.as_deref(),
                    },
                    active_selection,
                    TerminalSize { width, height },
                )?;
            }
            OverlayState::Prompt(prompt) => {
                render_session(
                    stdout,
                    &current_session,
                    &snapshot,
                    BottomBar::Prompt {
                        buffer: &prompt.buffer,
                        completions: &prompt.completions,
                        selected: prompt.selected,
                        cursor: prompt.cursor,
                    },
                    None,
                    TerminalSize { width, height },
                )?;
            }
            OverlayState::ChooseTree(tree) => {
                let (title, preview_snapshot) = chooser_preview(paths, tree)?;
                let chooser_status = choose_tree_status(tree);
                render_choose_tree(
                    stdout,
                    &current_session,
                    &snapshot,
                    &tree.lines,
                    &title,
                    &preview_snapshot,
                    &chooser_status,
                    TerminalSize { width, height },
                )?;
            }
            OverlayState::Help => {
                render_help_overlay(
                    stdout,
                    &current_session,
                    &snapshot,
                    &help_lines(),
                    TerminalSize { width, height },
                )?;
            }
        }
        status_message = None;

        if !event::poll(Duration::from_millis(50)).context("failed to poll terminal events")? {
            continue;
        }

        match event::read().context("failed to read terminal event")? {
            Event::Key(key) => {
                let current_overlay = std::mem::replace(&mut overlay, OverlayState::None);
                match current_overlay {
                    OverlayState::Prompt(mut prompt) => {
                        match handle_prompt_key(
                            paths,
                            &snapshot,
                            &mut current_session,
                            &mut prompt,
                            &mut prompt_history,
                            key,
                            &mut status_message,
                        )? {
                            PromptResult::KeepOpen => {
                                overlay = OverlayState::Prompt(prompt);
                            }
                            PromptResult::Close => {}
                            PromptResult::CloseAndClearSelection => {
                                selection_anchor = None;
                                active_selection = None;
                            }
                        }
                    }
                    OverlayState::ChooseTree(mut tree) => {
                        if handle_choose_tree_key(paths, &mut tree, key, &mut current_session)? {
                            overlay = OverlayState::ChooseTree(tree);
                        }
                    }
                    OverlayState::Help => match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {}
                        _ => overlay = OverlayState::Help,
                    },
                    OverlayState::None => match state.handle_key(key) {
                        InputAction::Noop => {}
                        InputAction::Detach => break,
                        InputAction::SendBytes(bytes) => {
                            send_input_bytes(paths, &current_session, &bytes)?
                        }
                        InputAction::SplitPane(axis) => {
                            let _ = request_response(
                                paths,
                                CommandRequest::SplitPane {
                                    target: current_session.clone(),
                                    axis,
                                    command: Vec::new(),
                                },
                            )?;
                        }
                        InputAction::SelectWindowIndex(index) => {
                            if let Some(window) = snapshot
                                .windows
                                .iter()
                                .find(|window| window.index == index as usize)
                            {
                                let _ = request_response(
                                    paths,
                                    CommandRequest::SelectWindow {
                                        target: format!("{}:{}", current_session, window.id),
                                    },
                                )?;
                            }
                        }
                        InputAction::OpenPrompt => {
                            overlay = OverlayState::Prompt(PromptState {
                                buffer: String::new(),
                                cursor: 0,
                                completions: command_completions(""),
                                selected: 0,
                                history_index: None,
                            });
                        }
                        InputAction::OpenSessions => {
                            overlay = OverlayState::ChooseTree(build_choose_tree(
                                paths,
                                &current_session,
                            )?);
                        }
                        InputAction::OpenHelp => {
                            overlay = OverlayState::Help;
                        }
                        InputAction::NewWindow => {
                            let _ = request_response(
                                paths,
                                CommandRequest::NewWindow {
                                    session: current_session.clone(),
                                    name: None,
                                    command: Vec::new(),
                                },
                            )?;
                        }
                        InputAction::NextWindow => {
                            let _ = request_response(
                                paths,
                                CommandRequest::CycleWindow {
                                    session: current_session.clone(),
                                    direction: CycleDirection::Next,
                                },
                            )?;
                        }
                        InputAction::PrevWindow => {
                            let _ = request_response(
                                paths,
                                CommandRequest::CycleWindow {
                                    session: current_session.clone(),
                                    direction: CycleDirection::Prev,
                                },
                            )?;
                        }
                        InputAction::FocusPane(direction) => {
                            let _ = request_response(
                                paths,
                                CommandRequest::SelectPane {
                                    target: None,
                                    direction: Some(direction),
                                },
                            )?;
                        }
                        InputAction::ResizePane(direction, amount) => {
                            let _ = request_response(
                                paths,
                                CommandRequest::ResizePane {
                                    target: current_session.clone(),
                                    direction,
                                    amount,
                                },
                            )?;
                        }
                        InputAction::KillPane => {
                            let _ = request_response(
                                paths,
                                CommandRequest::KillPane {
                                    target: current_session.clone(),
                                },
                            )?;
                        }
                    },
                }
            }
            Event::Paste(text) => {
                if matches!(overlay, OverlayState::None) {
                    send_input_bytes(paths, &current_session, text.as_bytes())?;
                }
            }
            Event::Mouse(mouse) => {
                if matches!(overlay, OverlayState::None) {
                    handle_mouse_event(
                        paths,
                        &current_session,
                        &snapshot,
                        mouse,
                        stdout,
                        &mut selection_anchor,
                        &mut active_selection,
                        &mut resize_drag,
                        &mut status_message,
                    )?;
                }
            }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_prompt_key(
    paths: &RuntimePaths,
    snapshot: &RenderSnapshot,
    current_session: &mut String,
    prompt: &mut PromptState,
    history: &mut Vec<String>,
    key: crossterm::event::KeyEvent,
    status_message: &mut Option<String>,
) -> Result<PromptResult> {
    match key.code {
        KeyCode::Esc => return Ok(PromptResult::Close),
        KeyCode::Enter => {
            let command = prompt.buffer.trim().to_string();
            if !command.is_empty() {
                history.push(command.clone());
                let result = execute_prompt_command(paths, snapshot, current_session, &command)?;
                *status_message = result;
            }
            return Ok(PromptResult::CloseAndClearSelection);
        }
        KeyCode::Tab => {
            if !prompt.completions.is_empty() {
                prompt.selected = (prompt.selected + 1) % prompt.completions.len();
                prompt.buffer = prompt.completions[prompt.selected].clone();
                prompt.cursor = prompt.buffer.len();
            }
        }
        KeyCode::Backspace => {
            if prompt.cursor > 0 {
                prompt.buffer.remove(prompt.cursor - 1);
                prompt.cursor -= 1;
            }
        }
        KeyCode::Delete => {
            if prompt.cursor < prompt.buffer.len() {
                prompt.buffer.remove(prompt.cursor);
            }
        }
        KeyCode::Left => prompt.cursor = prompt.cursor.saturating_sub(1),
        KeyCode::Right => prompt.cursor = (prompt.cursor + 1).min(prompt.buffer.len()),
        KeyCode::Home => prompt.cursor = 0,
        KeyCode::End => prompt.cursor = prompt.buffer.len(),
        KeyCode::Up => {
            if history.is_empty() {
                return Ok(PromptResult::KeepOpen);
            }
            let next = prompt
                .history_index
                .map(|index| index.saturating_sub(1))
                .unwrap_or(history.len().saturating_sub(1));
            prompt.history_index = Some(next);
            prompt.buffer = history[next].clone();
            prompt.cursor = prompt.buffer.len();
        }
        KeyCode::Down => {
            if let Some(index) = prompt.history_index {
                let next = (index + 1).min(history.len().saturating_sub(1));
                prompt.history_index = Some(next);
                prompt.buffer = history[next].clone();
                prompt.cursor = prompt.buffer.len();
            }
        }
        KeyCode::Char(ch) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            prompt.buffer.insert(prompt.cursor, ch);
            prompt.cursor += 1;
        }
        _ => {}
    }

    let prefix = prompt.buffer.split_whitespace().next().unwrap_or("");
    prompt.completions = command_completions(prefix);
    prompt.selected = 0;
    Ok(PromptResult::KeepOpen)
}

fn execute_prompt_command(
    paths: &RuntimePaths,
    snapshot: &RenderSnapshot,
    current_session: &mut String,
    input: &str,
) -> Result<Option<String>> {
    match parse_command(input).map_err(anyhow::Error::msg)? {
        InteractiveCommand::SplitWindow { horizontal } => {
            let _ = request_response(
                paths,
                CommandRequest::SplitPane {
                    target: current_session.clone(),
                    axis: if horizontal {
                        SplitAxis::Vertical
                    } else {
                        SplitAxis::Horizontal
                    },
                    command: Vec::new(),
                },
            )?;
            Ok(None)
        }
        InteractiveCommand::NewWindow => {
            let _ = request_response(
                paths,
                CommandRequest::NewWindow {
                    session: current_session.clone(),
                    name: None,
                    command: Vec::new(),
                },
            )?;
            Ok(None)
        }
        InteractiveCommand::SelectWindow { target } => {
            let target = resolve_window_target(snapshot, current_session, &target);
            let _ = request_response(paths, CommandRequest::SelectWindow { target })?;
            Ok(None)
        }
        InteractiveCommand::NextWindow => {
            let _ = request_response(
                paths,
                CommandRequest::CycleWindow {
                    session: current_session.clone(),
                    direction: CycleDirection::Next,
                },
            )?;
            Ok(None)
        }
        InteractiveCommand::PreviousWindow => {
            let _ = request_response(
                paths,
                CommandRequest::CycleWindow {
                    session: current_session.clone(),
                    direction: CycleDirection::Prev,
                },
            )?;
            Ok(None)
        }
        InteractiveCommand::KillPane => {
            let _ = request_response(
                paths,
                CommandRequest::KillPane {
                    target: current_session.clone(),
                },
            )?;
            Ok(None)
        }
        InteractiveCommand::KillWindow => {
            let target = format!("{}:{}", current_session, snapshot.active_window_id);
            let _ = request_response(paths, CommandRequest::KillWindow { target })?;
            Ok(None)
        }
        InteractiveCommand::AttachSession { target }
        | InteractiveCommand::SwitchClient { target } => {
            let _ = request_response(
                paths,
                CommandRequest::Attach {
                    session: Some(target.clone()),
                },
            )?;
            *current_session = target;
            Ok(None)
        }
        InteractiveCommand::ListSessions => {
            let response = request_response(paths, CommandRequest::ListSessions)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::ListWindows => {
            let response = request_response(
                paths,
                CommandRequest::ListWindows {
                    session: current_session.clone(),
                },
            )?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::ListPanes => {
            let response = request_response(
                paths,
                CommandRequest::ListPanes {
                    target: current_session.clone(),
                },
            )?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::ChooseTree => Ok(Some("use Ctrl-b s".into())),
        InteractiveCommand::DetachClient => Ok(Some("use Ctrl-b d".into())),
        InteractiveCommand::RenameWindow { name } => {
            let target = format!("{}:{}", current_session, snapshot.active_window_id);
            let _ = request_response(paths, CommandRequest::RenameWindow { target, name })?;
            Ok(None)
        }
        InteractiveCommand::SendKeys { keys } => {
            let _ = request_response(
                paths,
                CommandRequest::SendKeys {
                    target: current_session.clone(),
                    keys,
                },
            )?;
            Ok(None)
        }
    }
}

fn format_list_response(response: CommandResponse) -> String {
    match response {
        CommandResponse::SessionList { sessions } => sessions.join(" "),
        CommandResponse::WindowList { windows } => windows
            .into_iter()
            .map(|window| {
                if window.active {
                    format!("[{}:{}]", window.index, window.name)
                } else {
                    format!("{}:{}", window.index, window.name)
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        CommandResponse::PaneList { panes } => panes
            .into_iter()
            .map(|pane| pane.id.to_string())
            .collect::<Vec<_>>()
            .join(" "),
        CommandResponse::Error { message } => message,
        _ => String::new(),
    }
}

fn command_completions(prefix: &str) -> Vec<String> {
    complete_commands(prefix)
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn resolve_window_target(snapshot: &RenderSnapshot, session: &str, target: &str) -> String {
    if let Ok(index) = target.parse::<usize>()
        && let Some(window) = snapshot.windows.iter().find(|window| window.index == index)
    {
        return format!("{session}:{}", window.id);
    }
    if target.contains(':') {
        target.to_string()
    } else {
        format!("{session}:{target}")
    }
}

fn build_choose_tree(paths: &RuntimePaths, current_session: &str) -> Result<ChooseTreeState> {
    let mut state = ChooseTreeState {
        items: Vec::new(),
        lines: Vec::new(),
        selected: 0,
        expanded_sessions: BTreeSet::new(),
        expanded_windows: BTreeSet::new(),
        attached_session: current_session.to_string(),
        search_input: None,
        last_search: None,
    };
    rebuild_choose_tree(paths, &mut state)?;
    Ok(state)
}

fn rebuild_choose_tree(paths: &RuntimePaths, state: &mut ChooseTreeState) -> Result<()> {
    let sessions = match request_response(paths, CommandRequest::ListSessions)? {
        CommandResponse::SessionList { sessions } => sessions,
        other => return Err(anyhow!("unexpected session list response: {other:?}")),
    };
    let mut items = Vec::new();
    let mut lines = Vec::new();

    for session in sessions {
        let expanded = state.expanded_sessions.contains(&session);
        items.push(ChooseItem::Session(session.clone()));
        let window_count = match request_response(
            paths,
            CommandRequest::ListWindows {
                session: session.clone(),
            },
        )? {
            CommandResponse::WindowList { windows } => windows.len(),
            _ => 0,
        };
        lines.push(TreeLine {
            depth: 0,
            label: if session == state.attached_session {
                format!("{session}: {window_count} windows (attached)")
            } else {
                format!("{session}: {window_count} windows")
            },
            selected: false,
            expanded,
            has_children: true,
        });
        if !expanded {
            continue;
        }
        let windows = match request_response(
            paths,
            CommandRequest::ListWindows {
                session: session.clone(),
            },
        )? {
            CommandResponse::WindowList { windows } => windows,
            _ => Vec::new(),
        };
        for window in windows {
            let expanded_window = state
                .expanded_windows
                .contains(&(session.clone(), window.id));
            items.push(ChooseItem::Window {
                session: session.clone(),
                window_id: window.id,
            });
            lines.push(TreeLine {
                depth: 1,
                label: format!("{}:{}", window.index, window.name),
                selected: false,
                expanded: expanded_window,
                has_children: true,
            });
            if !expanded_window {
                continue;
            }
            let panes = match request_response(
                paths,
                CommandRequest::ListPanes {
                    target: format!("{session}:{}", window.id),
                },
            )? {
                CommandResponse::PaneList { panes } => panes,
                _ => Vec::new(),
            };
            for pane in panes {
                items.push(ChooseItem::Pane {
                    session: session.clone(),
                    window_id: window.id,
                    pane_id: pane.id,
                });
                lines.push(TreeLine {
                    depth: 2,
                    label: format!("{} ({})", pane.id, pane.title),
                    selected: false,
                    expanded: false,
                    has_children: false,
                });
            }
        }
    }

    if !items.is_empty() {
        state.selected = state.selected.min(items.len() - 1);
        if let Some(line) = lines.get_mut(state.selected) {
            line.selected = true;
        }
    } else {
        state.selected = 0;
    }
    state.items = items;
    state.lines = lines;
    Ok(())
}

fn chooser_preview(
    paths: &RuntimePaths,
    tree: &ChooseTreeState,
) -> Result<(String, RenderSnapshot)> {
    let Some(item) = tree.items.get(tree.selected) else {
        return Ok((
            "no sessions".into(),
            fallback_snapshot(String::new(), 80, 24),
        ));
    };
    let session = match item {
        ChooseItem::Session(session)
        | ChooseItem::Window { session, .. }
        | ChooseItem::Pane { session, .. } => session.clone(),
    };
    let snapshot = match request_response(
        paths,
        CommandRequest::Attach {
            session: Some(session.clone()),
        },
    )? {
        CommandResponse::Attached {
            preview, snapshot, ..
        } => snapshot.unwrap_or_else(|| fallback_snapshot(preview, 80, 24)),
        _ => fallback_snapshot(String::new(), 80, 24),
    };
    Ok((session, snapshot))
}

fn handle_choose_tree_key(
    paths: &RuntimePaths,
    tree: &mut ChooseTreeState,
    key: crossterm::event::KeyEvent,
    current_session: &mut String,
) -> Result<bool> {
    if let Some(query) = tree.search_input.as_mut() {
        match key.code {
            KeyCode::Esc => {
                tree.search_input = None;
                return Ok(true);
            }
            KeyCode::Enter => {
                let committed = query.trim().to_string();
                tree.search_input = None;
                if committed.is_empty() {
                    return Ok(true);
                }
                tree.last_search = Some(committed.clone());
                apply_choose_tree_search(tree, &committed, true);
                return Ok(true);
            }
            KeyCode::Backspace => {
                query.pop();
                return Ok(true);
            }
            KeyCode::Char(ch)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                query.push(ch);
                return Ok(true);
            }
            _ => return Ok(true),
        }
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return Ok(false),
        KeyCode::Up => {
            if tree.selected > 0 {
                tree.selected -= 1;
            }
        }
        KeyCode::Down => {
            if tree.selected + 1 < tree.items.len() {
                tree.selected += 1;
            }
        }
        KeyCode::Tab => toggle_choose_selected(tree),
        KeyCode::Char('=') if key.modifiers.contains(KeyModifiers::ALT) => {
            expand_all_choose_items(paths, tree)?
        }
        KeyCode::Char('+') if key.modifiers.contains(KeyModifiers::ALT) => {
            expand_all_choose_items(paths, tree)?
        }
        KeyCode::Char('-') if key.modifiers.contains(KeyModifiers::ALT) => {
            collapse_all_choose_items(tree)
        }
        KeyCode::Char('+') => toggle_choose_item(tree, true),
        KeyCode::Char('-') => toggle_choose_item(tree, false),
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            tree.search_input = Some(String::new());
        }
        KeyCode::Char('n') => repeat_choose_tree_search(tree, true),
        KeyCode::Char('N') => repeat_choose_tree_search(tree, false),
        KeyCode::Enter => {
            if let Some(item) = tree.items.get(tree.selected).cloned() {
                match item {
                    ChooseItem::Session(session) => {
                        *current_session = session;
                        tree.attached_session = current_session.clone();
                    }
                    ChooseItem::Window { session, window_id } => {
                        let _ = request_response(
                            paths,
                            CommandRequest::SelectWindow {
                                target: format!("{session}:{window_id}"),
                            },
                        )?;
                        *current_session = session;
                        tree.attached_session = current_session.clone();
                    }
                    ChooseItem::Pane {
                        session,
                        window_id,
                        pane_id,
                    } => {
                        let _ = request_response(
                            paths,
                            CommandRequest::SelectWindow {
                                target: format!("{session}:{window_id}"),
                            },
                        )?;
                        let _ = request_response(
                            paths,
                            CommandRequest::SelectPane {
                                target: Some(format!("{session}:{window_id}.{pane_id}")),
                                direction: None,
                            },
                        )?;
                        *current_session = session;
                        tree.attached_session = current_session.clone();
                    }
                }
                return Ok(false);
            }
        }
        _ => {}
    }
    rebuild_choose_tree(paths, tree)?;
    Ok(true)
}

fn toggle_choose_item(tree: &mut ChooseTreeState, expand: bool) {
    if let Some(item) = tree.items.get(tree.selected) {
        match item {
            ChooseItem::Session(session) => {
                if expand {
                    tree.expanded_sessions.insert(session.clone());
                } else {
                    tree.expanded_sessions.remove(session);
                }
            }
            ChooseItem::Window { session, window_id } => {
                let key = (session.clone(), *window_id);
                if expand {
                    tree.expanded_windows.insert(key);
                } else {
                    tree.expanded_windows.remove(&key);
                }
            }
            ChooseItem::Pane { .. } => {}
        }
    }
}

fn toggle_choose_selected(tree: &mut ChooseTreeState) {
    if let Some(item) = tree.items.get(tree.selected) {
        match item {
            ChooseItem::Session(session) => {
                if tree.expanded_sessions.contains(session) {
                    tree.expanded_sessions.remove(session);
                } else {
                    tree.expanded_sessions.insert(session.clone());
                }
            }
            ChooseItem::Window { session, window_id } => {
                let key = (session.clone(), *window_id);
                if tree.expanded_windows.contains(&key) {
                    tree.expanded_windows.remove(&key);
                } else {
                    tree.expanded_windows.insert(key);
                }
            }
            ChooseItem::Pane { .. } => {}
        }
    }
}

fn expand_all_choose_items(paths: &RuntimePaths, tree: &mut ChooseTreeState) -> Result<()> {
    let sessions = match request_response(paths, CommandRequest::ListSessions)? {
        CommandResponse::SessionList { sessions } => sessions,
        other => return Err(anyhow!("unexpected session list response: {other:?}")),
    };
    tree.expanded_sessions = sessions.iter().cloned().collect();
    tree.expanded_windows.clear();
    for session in sessions {
        let windows = match request_response(
            paths,
            CommandRequest::ListWindows {
                session: session.clone(),
            },
        )? {
            CommandResponse::WindowList { windows } => windows,
            _ => Vec::new(),
        };
        for window in windows {
            tree.expanded_windows.insert((session.clone(), window.id));
        }
    }
    Ok(())
}

fn collapse_all_choose_items(tree: &mut ChooseTreeState) {
    tree.expanded_sessions.clear();
    tree.expanded_windows.clear();
}

fn apply_choose_tree_search(tree: &mut ChooseTreeState, query: &str, forward: bool) {
    if tree.lines.is_empty() {
        return;
    }
    let query = query.to_ascii_lowercase();
    let len = tree.lines.len();
    for step in 1..=len {
        let index = if forward {
            (tree.selected + step) % len
        } else {
            (tree.selected + len - (step % len)) % len
        };
        if tree.lines[index]
            .label
            .to_ascii_lowercase()
            .contains(&query)
        {
            tree.selected = index;
            for line in &mut tree.lines {
                line.selected = false;
            }
            if let Some(line) = tree.lines.get_mut(index) {
                line.selected = true;
            }
            break;
        }
    }
}

fn repeat_choose_tree_search(tree: &mut ChooseTreeState, forward: bool) {
    if let Some(query) = tree.last_search.clone() {
        apply_choose_tree_search(tree, &query, forward);
    }
}

fn choose_tree_status(tree: &ChooseTreeState) -> String {
    if let Some(query) = tree.search_input.as_ref() {
        format!("search: {query}")
    } else if let Some(query) = tree.last_search.as_ref() {
        format!("choose-tree | C-s search | n/N repeat ({query}) | Enter select | q cancel")
    } else {
        "choose-tree | C-s search | n/N repeat | Alt-+ expand all | Alt-- collapse all | Enter select | q cancel".into()
    }
}

fn help_lines() -> Vec<String> {
    vec![
        "admux help".into(),
        String::new(),
        "Ctrl-b %      split vertically".into(),
        "Ctrl-b \"      split horizontally".into(),
        "Ctrl-b 0..9   select window by index".into(),
        "Ctrl-b h/j/k/l move pane focus".into(),
        "Ctrl-b H/J/K/L resize active pane".into(),
        "Ctrl-b c      new window".into(),
        "Ctrl-b n/p    next/previous window".into(),
        "Ctrl-b x      kill active pane".into(),
        "Ctrl-b :      command prompt".into(),
        "Ctrl-b s      choose-tree".into(),
        "Ctrl-b d      detach".into(),
        "Ctrl-b ?      help".into(),
    ]
}

#[allow(clippy::too_many_arguments)]
fn handle_mouse_event(
    paths: &RuntimePaths,
    session: &str,
    snapshot: &RenderSnapshot,
    mouse: MouseEvent,
    stdout: &mut impl Write,
    selection_anchor: &mut Option<SelectionAnchor>,
    active_selection: &mut Option<PaneSelection>,
    resize_drag: &mut Option<ResizeDrag>,
    status_message: &mut Option<String>,
) -> Result<()> {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((pane, direction)) = separator_hit(snapshot, mouse.row, mouse.column) {
                *resize_drag = Some(ResizeDrag {
                    pane_id: pane.pane_id,
                    direction,
                    last_row: mouse.row,
                    last_col: mouse.column,
                });
            } else if let Some((pane, row, col)) =
                pane_content_hit(snapshot, mouse.row, mouse.column)
            {
                let _ = request_response(
                    paths,
                    CommandRequest::SelectPane {
                        target: Some(format!(
                            "{session}:{}.{}",
                            snapshot.active_window_id, pane.pane_id
                        )),
                        direction: None,
                    },
                )?;
                *selection_anchor = Some(SelectionAnchor {
                    pane_id: pane.pane_id,
                    row,
                    col,
                });
                *active_selection = Some(PaneSelection {
                    pane_id: pane.pane_id,
                    selection: Selection::new(row, col, row, col),
                });
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(resize) = resize_drag.as_mut() {
                if let Some((direction, delta)) = resize_drag_request(*resize, mouse) {
                    let target =
                        format!("{session}:{}.{}", snapshot.active_window_id, resize.pane_id);
                    let _ = request_response(
                        paths,
                        CommandRequest::ResizePane {
                            target,
                            direction,
                            amount: delta.saturating_mul(20),
                        },
                    )?;
                    resize.last_row = mouse.row;
                    resize.last_col = mouse.column;
                }
            } else if let Some(anchor) = selection_anchor.as_ref()
                && let Some((pane, row, col)) = pane_content_hit(snapshot, mouse.row, mouse.column)
                && pane.pane_id == anchor.pane_id
            {
                *active_selection = Some(PaneSelection {
                    pane_id: pane.pane_id,
                    selection: Selection::new(anchor.row, anchor.col, row, col).normalized(),
                });
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            *resize_drag = None;
            if let Some(anchor) = selection_anchor.take()
                && let Some((pane, row, col)) = pane_content_hit(snapshot, mouse.row, mouse.column)
                && pane.pane_id == anchor.pane_id
            {
                let selection = Selection::new(anchor.row, anchor.col, row, col).normalized();
                let copied = request_response(
                    paths,
                    CommandRequest::CopySelection {
                        session: session.to_string(),
                        pane_id: Some(pane.pane_id),
                        start_row: selection.start_row,
                        start_col: selection.start_col,
                        end_row: selection.end_row,
                        end_col: selection.end_col,
                    },
                )?;
                if let CommandResponse::SelectionCopied { text } = copied {
                    let copied_chars = text.chars().count();
                    if copied_chars > 0 {
                        copy_via_osc52(stdout, &text)
                            .context("failed to send OSC52 clipboard copy")?;
                        *status_message = Some(format!("copied {copied_chars} chars"));
                    }
                }
            }
            *active_selection = None;
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let direction = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                crate::ipc::ScrollDirection::Up
            } else {
                crate::ipc::ScrollDirection::Down
            };
            let _ = request_response(
                paths,
                CommandRequest::MouseScroll {
                    session: session.to_string(),
                    row: mouse.row,
                    col: mouse.column,
                    direction,
                },
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn pane_content_hit(
    snapshot: &RenderSnapshot,
    row: u16,
    col: u16,
) -> Option<(&PaneRender, u16, u16)> {
    snapshot.panes.iter().find_map(|pane| {
        if pane.rect.contains(row, col) {
            Some((pane, row - pane.rect.y, col - pane.rect.x))
        } else {
            None
        }
    })
}

fn separator_hit(
    snapshot: &RenderSnapshot,
    row: u16,
    col: u16,
) -> Option<(&PaneRender, NavigationDirection)> {
    for pane in &snapshot.panes {
        for other in &snapshot.panes {
            if pane.pane_id == other.pane_id {
                continue;
            }
            if pane.rect.x + pane.rect.width + 1 == other.rect.x
                && col == pane.rect.x + pane.rect.width
                && row >= pane.rect.y.max(other.rect.y)
                && row < (pane.rect.y + pane.rect.height).min(other.rect.y + other.rect.height)
            {
                return Some((pane, NavigationDirection::Right));
            }
            if pane.rect.y + pane.rect.height + 1 == other.rect.y
                && row == pane.rect.y + pane.rect.height
                && col >= pane.rect.x.max(other.rect.x)
                && col < (pane.rect.x + pane.rect.width).min(other.rect.x + other.rect.width)
            {
                return Some((pane, NavigationDirection::Down));
            }
        }
    }
    None
}

fn resize_drag_request(
    resize: ResizeDrag,
    mouse: MouseEvent,
) -> Option<(NavigationDirection, u16)> {
    match resize.direction {
        NavigationDirection::Right => {
            let delta = mouse.column.abs_diff(resize.last_col);
            if delta == 0 {
                None
            } else if mouse.column > resize.last_col {
                Some((NavigationDirection::Left, delta))
            } else {
                Some((NavigationDirection::Right, delta))
            }
        }
        NavigationDirection::Down => {
            let delta = mouse.row.abs_diff(resize.last_row);
            if delta == 0 {
                None
            } else if mouse.row > resize.last_row {
                Some((NavigationDirection::Up, delta))
            } else {
                Some((NavigationDirection::Down, delta))
            }
        }
        NavigationDirection::Left | NavigationDirection::Up => None,
    }
}

fn fallback_snapshot(preview: String, width: u16, height: u16) -> RenderSnapshot {
    let rows_plain = preview.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    RenderSnapshot {
        windows: vec![WindowSummary {
            id: 1,
            index: 0,
            name: "shell".into(),
            active: true,
        }],
        panes: vec![PaneRender {
            pane_id: 1,
            title: "shell".into(),
            rect: Rect {
                x: 0,
                y: 0,
                width,
                height: height.saturating_sub(1).max(1),
            },
            focused: true,
            rows_formatted: rows_plain.clone(),
            rows_plain,
            cursor: Some(PaneCursor { row: 0, col: 0 }),
        }],
        dividers: Vec::new(),
        active_window_id: 1,
        active_pane_id: 1,
    }
}

fn send_input_bytes(paths: &RuntimePaths, session: &str, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }

    let keys = vec![String::from_utf8_lossy(bytes).into_owned()];
    let _ = request_response(
        paths,
        CommandRequest::SendKeys {
            target: session.to_string(),
            keys,
        },
    )?;
    Ok(())
}

fn copy_via_osc52(out: &mut impl Write, text: &str) -> Result<()> {
    let encoded = STANDARD.encode(text.as_bytes());
    write!(out, "\x1b]52;c;{encoded}\x07").context("failed to write OSC52 sequence")?;
    out.flush().context("failed to flush OSC52 sequence")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::RuntimePaths;

    #[test]
    fn writes_and_reads_protocol_messages() {
        let response = CommandResponse::SessionCreated {
            session: "work".into(),
            pane_id: 1,
        };
        let encoded = serde_json::to_vec(&response).expect("encode response");
        let decoded: CommandResponse = serde_json::from_slice(&encoded).expect("decode response");
        assert_eq!(decoded, response);
    }

    #[test]
    fn runtime_paths_can_be_used_for_requests() {
        let paths = RuntimePaths {
            socket_path: "/tmp/admux-test/socket".into(),
            config_path: "/tmp/admux-test/config.toml".into(),
        };
        assert!(paths.socket_path.ends_with("socket"));
    }

    #[test]
    fn choose_tree_search_moves_selection_forward() {
        let mut tree = ChooseTreeState {
            items: vec![
                ChooseItem::Session("work".into()),
                ChooseItem::Window {
                    session: "work".into(),
                    window_id: 1,
                },
                ChooseItem::Pane {
                    session: "work".into(),
                    window_id: 1,
                    pane_id: 2,
                },
            ],
            lines: vec![
                TreeLine {
                    depth: 0,
                    label: "work".into(),
                    selected: true,
                    expanded: true,
                    has_children: true,
                },
                TreeLine {
                    depth: 1,
                    label: "1:editor".into(),
                    selected: false,
                    expanded: true,
                    has_children: true,
                },
                TreeLine {
                    depth: 2,
                    label: "2 (logs)".into(),
                    selected: false,
                    expanded: false,
                    has_children: false,
                },
            ],
            selected: 0,
            expanded_sessions: BTreeSet::new(),
            expanded_windows: BTreeSet::new(),
            attached_session: "work".into(),
            search_input: None,
            last_search: None,
        };

        apply_choose_tree_search(&mut tree, "logs", true);

        assert_eq!(tree.selected, 2);
        assert!(tree.lines[2].selected);
    }

    #[test]
    fn choose_tree_repeat_search_moves_backward() {
        let mut tree = ChooseTreeState {
            items: vec![
                ChooseItem::Session("work".into()),
                ChooseItem::Window {
                    session: "work".into(),
                    window_id: 1,
                },
                ChooseItem::Pane {
                    session: "work".into(),
                    window_id: 1,
                    pane_id: 2,
                },
            ],
            lines: vec![
                TreeLine {
                    depth: 0,
                    label: "work".into(),
                    selected: false,
                    expanded: true,
                    has_children: true,
                },
                TreeLine {
                    depth: 1,
                    label: "1:editor".into(),
                    selected: false,
                    expanded: true,
                    has_children: true,
                },
                TreeLine {
                    depth: 2,
                    label: "2 (logs)".into(),
                    selected: true,
                    expanded: false,
                    has_children: false,
                },
            ],
            selected: 2,
            expanded_sessions: BTreeSet::new(),
            expanded_windows: BTreeSet::new(),
            attached_session: "work".into(),
            search_input: None,
            last_search: Some("editor".into()),
        };

        repeat_choose_tree_search(&mut tree, false);

        assert_eq!(tree.selected, 1);
        assert!(tree.lines[1].selected);
    }

    #[test]
    fn collapse_all_choose_items_clears_expansions() {
        let mut tree = ChooseTreeState {
            items: Vec::new(),
            lines: Vec::new(),
            selected: 0,
            expanded_sessions: ["work".to_string()].into_iter().collect(),
            expanded_windows: [("work".to_string(), 1)].into_iter().collect(),
            attached_session: "work".into(),
            search_input: None,
            last_search: None,
        };

        collapse_all_choose_items(&mut tree);

        assert!(tree.expanded_sessions.is_empty());
        assert!(tree.expanded_windows.is_empty());
    }

    #[test]
    fn resize_drag_request_reverses_direction_for_leftward_motion() {
        let resize = ResizeDrag {
            pane_id: 1,
            direction: NavigationDirection::Right,
            last_row: 0,
            last_col: 10,
        };

        let request = resize_drag_request(
            resize,
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 7,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
        );

        assert_eq!(request, Some((NavigationDirection::Right, 3)));
    }

    #[test]
    fn resize_drag_request_grows_left_pane_when_dragging_right() {
        let resize = ResizeDrag {
            pane_id: 1,
            direction: NavigationDirection::Right,
            last_row: 0,
            last_col: 10,
        };

        let request = resize_drag_request(
            resize,
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 13,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
        );

        assert_eq!(request, Some((NavigationDirection::Left, 3)));
    }
}
