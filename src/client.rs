use std::{
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
        Event, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::{
    cli::{
        AdmuxCli, ClientCommand, NewWindowArgs, ResizePaneArgs, SelectPaneArgs, SplitPaneArgs,
    },
    copy_mode::Selection,
    input::{InputAction, InputState},
    ipc::{
        CommandRequest, CommandResponse, CycleDirection, NavigationDirection, PaneCursor,
        PaneRender, RenderSnapshot,
    },
    layout::SplitAxis,
    pane::Rect,
    paths::RuntimePaths,
    render::{PaneSelection, TerminalSize, render_session},
    window::WindowSummary,
};

pub fn run_from_env() -> Result<()> {
    let cli = AdmuxCli::parse();
    run(cli)
}

pub fn run(cli: AdmuxCli) -> Result<()> {
    let paths = RuntimePaths::resolve();
    let request = match cli.command {
        ClientCommand::New(args) => {
            let requested_name = args.name.clone();
            let response = request_response(
                &paths,
                CommandRequest::NewSession {
                    name: args.name,
                    cwd: args.cwd,
                    command: args.command,
                },
            )?;
            let created_session = match &response {
                CommandResponse::SessionCreated { session, .. } => Some(session.clone()),
                _ => None,
            };
            print_response(&paths, response)?;

            if !args.detach
                && io::stdout().is_terminal()
                && std::env::var_os("ADMUX_NONINTERACTIVE").is_none()
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
        ClientCommand::ListPanes(args) => CommandRequest::ListPanes { target: args.target },
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

    let result = run_attach_loop(paths, session, &mut stdout);

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

fn run_attach_loop(paths: &RuntimePaths, session: &str, stdout: &mut impl Write) -> Result<()> {
    let mut state = InputState::default();
    let mut last_size = (0, 0);
    let mut selection_anchor: Option<SelectionAnchor> = None;
    let mut active_selection: Option<PaneSelection> = None;
    let mut resize_drag: Option<ResizeDrag> = None;
    let mut status_message: Option<String> = None;

    loop {
        let (width, height) = terminal::size().context("failed to read terminal size")?;
        let rows = height.max(1);
        let cols = width.max(1);
        if last_size != (rows, cols) {
            let _ = request_response(
                paths,
                CommandRequest::Resize {
                    session: session.to_string(),
                    rows,
                    cols,
                },
            )?;
            last_size = (rows, cols);
        }

        let response = request_response(
            paths,
            CommandRequest::Attach {
                session: Some(session.to_string()),
            },
        )?;

        let snapshot = match response {
            CommandResponse::Attached {
                preview,
                snapshot,
                session: _,
                ..
            } => snapshot.unwrap_or_else(|| fallback_snapshot(preview, width, height)),
            CommandResponse::Error { message } => return Err(anyhow!(message)),
            other => return Err(anyhow!("unexpected attach response: {other:?}")),
        };

        render_session(
            stdout,
            session,
            &snapshot,
            status_message.as_deref(),
            active_selection,
            TerminalSize { width, height },
        )
        .context("failed to render session")?;
        status_message = None;

        if event::poll(Duration::from_millis(50)).context("failed to poll terminal events")? {
            match event::read().context("failed to read terminal event")? {
                Event::Key(key) => match state.handle_key(key) {
                    InputAction::Noop => {}
                    InputAction::Detach => break,
                    InputAction::SendBytes(bytes) => send_input_bytes(paths, session, &bytes)?,
                    InputAction::SplitPane(axis) => {
                        let _ = request_response(
                            paths,
                            CommandRequest::SplitPane {
                                target: session.to_string(),
                                axis,
                                command: Vec::new(),
                            },
                        )?;
                    }
                    InputAction::NewWindow => {
                        let _ = request_response(
                            paths,
                            CommandRequest::NewWindow {
                                session: session.to_string(),
                                name: None,
                                command: Vec::new(),
                            },
                        )?;
                    }
                    InputAction::NextWindow => {
                        let _ = request_response(
                            paths,
                            CommandRequest::CycleWindow {
                                session: session.to_string(),
                                direction: CycleDirection::Next,
                            },
                        )?;
                    }
                    InputAction::PrevWindow => {
                        let _ = request_response(
                            paths,
                            CommandRequest::CycleWindow {
                                session: session.to_string(),
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
                                target: session.to_string(),
                                direction,
                                amount,
                            },
                        )?;
                    }
                    InputAction::KillPane => {
                        let _ = request_response(
                            paths,
                            CommandRequest::KillPane {
                                target: session.to_string(),
                            },
                        )?;
                    }
                },
                Event::Paste(text) => send_input_bytes(paths, session, text.as_bytes())?,
                Event::Mouse(mouse) => {
                    handle_mouse_event(
                        paths,
                        session,
                        &snapshot,
                        mouse,
                        stdout,
                        &mut selection_anchor,
                        &mut active_selection,
                        &mut resize_drag,
                        &mut status_message,
                    )?;
                }
                Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => {}
            }
        }
    }
    Ok(())
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
            if let Some((pane, direction)) = border_hit(snapshot, mouse.row, mouse.column) {
                *resize_drag = Some(ResizeDrag {
                    pane_id: pane.pane_id,
                    direction,
                    last_row: mouse.row,
                    last_col: mouse.column,
                });
            } else if let Some((pane, row, col)) = pane_content_hit(snapshot, mouse.row, mouse.column) {
                let _ = request_response(
                    paths,
                    CommandRequest::SelectPane {
                        target: Some(format!("{session}:{}.{}", snapshot.active_window_id, pane.pane_id)),
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
                let delta = match resize.direction {
                    NavigationDirection::Left | NavigationDirection::Right => {
                        mouse.column.abs_diff(resize.last_col)
                    }
                    NavigationDirection::Up | NavigationDirection::Down => {
                        mouse.row.abs_diff(resize.last_row)
                    }
                };
                if delta > 0 {
                    let target = format!("{session}:{}.{}", snapshot.active_window_id, resize.pane_id);
                    let _ = request_response(
                        paths,
                        CommandRequest::ResizePane {
                            target,
                            direction: resize.direction,
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
        let content = pane.rect.content();
        if content.contains(row, col) {
            Some((pane, row - content.y, col - content.x))
        } else {
            None
        }
    })
}

fn border_hit(
    snapshot: &RenderSnapshot,
    row: u16,
    col: u16,
) -> Option<(&PaneRender, NavigationDirection)> {
    snapshot.panes.iter().find_map(|pane| {
        let rect = pane.rect;
        if rect.width < 2 || rect.height < 2 || !rect.contains(row, col) {
            return None;
        }
        if col == rect.x && rect.x > 0 {
            Some((pane, NavigationDirection::Left))
        } else if col + 1 == rect.right() {
            Some((pane, NavigationDirection::Right))
        } else if row == rect.y && rect.y > 0 {
            Some((pane, NavigationDirection::Up))
        } else if row + 1 == rect.bottom() {
            Some((pane, NavigationDirection::Down))
        } else {
            None
        }
    })
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
}
