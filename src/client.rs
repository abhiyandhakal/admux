use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::OpenOptions,
    io::{self, IsTerminal, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::{Parser, error::ErrorKind};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::{
    alias::AliasRegistry,
    cli::{
        AdmuxCli, AliasAddArgs, AliasArgs, AliasCommand, ClientCommand, NewWindowArgs,
        PasteBufferArgs, ResizePaneArgs, SelectPaneArgs, SetBufferArgs, SplitPaneArgs,
        is_reserved_top_level_name,
    },
    commands::{InteractiveCommand, complete as complete_commands, parse as parse_command},
    clipboard::{ClipboardBackend, ClipboardConfig},
    config::{Config, ResolvedConfig, StatusPosition},
    copy_mode::{CopyMode, Selection},
    input::{InputAction, InputMode, InputState},
    ipc::{
        BufferSummary, ClientViewport, CommandRequest, CommandResponse, CycleDirection, NavigationDirection,
        PaneCursor, PaneMouseKind, PaneRender, RenderSnapshot, SwitchSource,
    },
    layout::SplitAxis,
    numbering::Numbering,
    pane::Rect,
    pty::{HelperMouseEventKind, PaneProcess},
    paths::RuntimePaths,
    render::{
        BottomBar, PaneSelection, TerminalSize, TreeLine, render_buffer_chooser,
        render_choose_tree, render_help_overlay, render_session,
    },
    window::WindowSummary,
};

const ATTACH_INPUT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const SNAPSHOT_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const ALT_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(150);
static INTERACTIVE_CLIENT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn snapshot_refresh_due(elapsed: Duration) -> bool {
    elapsed >= SNAPSHOT_REFRESH_INTERVAL
}

fn interactive_client_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        INTERACTIVE_CLIENT_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

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
        window_index: u64,
    },
    Pane {
        session: String,
        window_index: u64,
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
    preview: Option<(usize, String, RenderSnapshot)>,
}

#[derive(Debug, Clone)]
struct ChooseBufferState {
    buffers: Vec<BufferSummary>,
    selected: usize,
    preview: Option<(usize, String)>,
}

#[derive(Debug, Clone)]
enum OverlayState {
    None,
    Prompt(PromptState),
    ChooseTree(ChooseTreeState),
    ChooseBuffer(ChooseBufferState),
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptResult {
    KeepOpen,
    Close,
    CloseAndClearSelection,
    OpenChooseTree,
    OpenChooseBuffer,
    Detach,
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
    span: u16,
}

#[derive(Debug, Clone, Copy)]
struct MouseCapture {
    pane_id: u64,
    button: MouseButton,
}

fn helper_mouse_kinds(
    button: MouseButton,
) -> Option<(HelperMouseEventKind, HelperMouseEventKind, HelperMouseEventKind)> {
    match button {
        MouseButton::Left => Some((
            HelperMouseEventKind::LeftDown,
            HelperMouseEventKind::LeftDrag,
            HelperMouseEventKind::LeftUp,
        )),
        MouseButton::Middle => Some((
            HelperMouseEventKind::MiddleDown,
            HelperMouseEventKind::MiddleDrag,
            HelperMouseEventKind::MiddleUp,
        )),
        MouseButton::Right => Some((
            HelperMouseEventKind::RightDown,
            HelperMouseEventKind::RightDrag,
            HelperMouseEventKind::RightUp,
        )),
    }
}

fn pane_mouse_kinds(
    button: MouseButton,
) -> Option<(PaneMouseKind, PaneMouseKind, PaneMouseKind)> {
    match button {
        MouseButton::Left => Some((
            PaneMouseKind::LeftDown,
            PaneMouseKind::LeftDrag,
            PaneMouseKind::LeftUp,
        )),
        MouseButton::Middle => Some((
            PaneMouseKind::MiddleDown,
            PaneMouseKind::MiddleDrag,
            PaneMouseKind::MiddleUp,
        )),
        MouseButton::Right => Some((
            PaneMouseKind::RightDown,
            PaneMouseKind::RightDrag,
            PaneMouseKind::RightUp,
        )),
    }
}

struct EventLogger {
    out: std::fs::File,
}

pub fn run_from_env() -> Result<()> {
    let argv = std::env::args_os().collect::<Vec<_>>();
    let paths = RuntimePaths::resolve();
    match AdmuxCli::try_parse_from(&argv) {
        Ok(cli) => run(cli),
        Err(err) => {
            if should_try_alias(&argv, &err)
                && let Some(cli) = try_resolve_alias_invocation(&argv, &paths)?
            {
                return run(cli);
            }
            err.exit()
        }
    }
}

fn should_try_alias(argv: &[OsString], err: &clap::Error) -> bool {
    argv.len() == 2 && matches!(err.kind(), ErrorKind::InvalidSubcommand)
}

fn try_resolve_alias_invocation(argv: &[OsString], paths: &RuntimePaths) -> Result<Option<AdmuxCli>> {
    let Some(name) = argv.get(1).and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    if name.starts_with('-') || is_reserved_top_level_name(name) {
        return Ok(None);
    }
    let registry = AliasRegistry::load(&paths.aliases_path)?;
    let Some(path) = registry.resolve(name) else {
        return Ok(None);
    };
    Ok(Some(AdmuxCli {
        command: ClientCommand::Up(crate::cli::UpArgs {
            detach: false,
            rebuild: false,
            path: Some(path.to_path_buf()),
        }),
    }))
}

pub fn run(cli: AdmuxCli) -> Result<()> {
    let paths = RuntimePaths::resolve();
    let request = match cli.command {
        ClientCommand::Up(args) => {
            let manifest_path = resolve_workspace_manifest_path(args.path.as_deref())?;
            let nested_switch = (!args.detach && interactive_terminal_available())
            .then(nested_switch_source)
            .flatten();
            let response = request_response(
                &paths,
                CommandRequest::UpWorkspace {
                    manifest_path,
                    rebuild: args.rebuild,
                    switch_from: nested_switch.clone(),
                },
            )?;
            let session = match &response {
                CommandResponse::WorkspaceReady { session, .. } => Some(session.clone()),
                _ => None,
            };
            if nested_switch.is_none() {
                print_response(&paths, response)?;
            } else {
                ensure_command_succeeded(response)?;
            }
            if !args.detach && interactive_terminal_available() && nested_switch.is_none()
            {
                let session = session
                    .ok_or_else(|| anyhow!("workspace response did not include a session name"))?;
                attach_interactive(&paths, &session)?;
            }
            return Ok(());
        }
        ClientCommand::Save(args) => CommandRequest::SaveWorkspace {
            session: args.session.or_else(|| std::env::var("ADMUX_SESSION").ok()),
        },
        ClientCommand::New(args) => {
            let args = normalize_new_args(args)?;
            let requested_name = args.name.clone();
            let nested_switch = (!args.detach && interactive_terminal_available())
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
            } else {
                ensure_command_succeeded(response)?;
            }

            if !args.detach && interactive_terminal_available() && nested_switch.is_none()
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
            viewport: None,
        },
        ClientCommand::Ls => CommandRequest::ListSessions,
        ClientCommand::ListWindows(args) => CommandRequest::ListWindows {
            session: args.session,
        },
        ClientCommand::ListPanes(args) => CommandRequest::ListPanes {
            target: args.target,
        },
        ClientCommand::ListBuffers => CommandRequest::ListBuffers,
        ClientCommand::ShowBuffer(args) => CommandRequest::ShowBuffer {
            buffer: args.buffer,
        },
        ClientCommand::DeleteBuffer(args) => CommandRequest::DeleteBuffer {
            buffer: args.buffer,
        },
        ClientCommand::PasteBuffer(PasteBufferArgs { buffer, target }) => {
            CommandRequest::PasteBuffer {
                target: target
                    .or_else(|| std::env::var("ADMUX_SESSION").ok())
                    .unwrap_or_default(),
                buffer,
            }
        }
        ClientCommand::SetBuffer(SetBufferArgs { buffer, data }) => CommandRequest::SetBuffer {
            buffer,
            data,
            append: false,
        },
        ClientCommand::SaveBuffer(args) => CommandRequest::SaveBuffer {
            buffer: args.buffer,
            path: resolve_client_path(args.path)?,
        },
        ClientCommand::LoadBuffer(args) => CommandRequest::LoadBuffer {
            path: resolve_client_path(args.path)?,
            buffer: args.buffer,
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
        ClientCommand::Alias(args) => {
            run_alias_command(&paths, args)?;
            return Ok(());
        }
    };

    let response = request_response(&paths, request)?;
    print_response(&paths, response)
}

fn nested_switch_source() -> Option<SwitchSource> {
    let session = std::env::var("ADMUX_SESSION").ok()?;
    let window_id = std::env::var("ADMUX_WINDOW").ok()?.parse().ok()?;
    let pane_id = std::env::var("ADMUX_PANE").ok()?.parse().ok()?;
    Some(SwitchSource {
        session,
        window_id,
        pane_id,
    })
}

fn resolve_workspace_manifest_path(path: Option<&std::path::Path>) -> Result<std::path::PathBuf> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir()
            .context("failed to resolve current directory")?
            .join("admux.toml"),
    };
    path.canonicalize()
        .with_context(|| format!("failed to resolve workspace manifest {}", path.display()))
}

fn normalize_new_args(mut args: crate::cli::NewArgs) -> Result<crate::cli::NewArgs> {
    if args.cwd.is_none() && args.command.len() == 1 && PathBuf::from(&args.command[0]).is_dir() {
        args.cwd = Some(PathBuf::from(args.command.remove(0)));
    }

    if args.cwd.is_none() {
        args.cwd = Some(std::env::current_dir().context("failed to resolve current directory")?);
    }
    if let Some(cwd) = args.cwd.as_mut()
        && cwd.is_relative()
    {
        *cwd = std::env::current_dir()
            .context("failed to resolve current directory")?
            .join(&*cwd);
    }

    Ok(args)
}

fn resolve_client_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir()
        .context("failed to resolve current directory")?
        .join(path))
}

fn apply_attached_session(
    response: &CommandResponse,
    current_session: &mut String,
    last_size: &mut (u16, u16),
) {
    if let CommandResponse::Attached { session, .. } = response
        && session != current_session
    {
        *current_session = session.clone();
        *last_size = (0, 0);
    }
}

pub fn request_response(paths: &RuntimePaths, request: CommandRequest) -> Result<CommandResponse> {
    // Every request opens a new Unix socket connection. Handshake that
    // connection instead of caching by pathname: a daemon can be replaced at
    // the same path between requests.
    ensure_protocol(paths)?;
    with_connection(paths, |stream| {
        write_message(stream, &request)?;
        read_message(stream)
    })
}

fn ensure_protocol(paths: &RuntimePaths) -> Result<()> {
    let response = with_connection(paths, |stream| {
        write_message(
            stream,
            &CommandRequest::Hello {
                version: crate::ipc::CURRENT_PROTOCOL_VERSION,
            },
        )?;
        read_message(stream)
    })?;

    match response {
        CommandResponse::HelloAck { version }
            if version == crate::ipc::CURRENT_PROTOCOL_VERSION =>
        {
            Ok(())
        }
        CommandResponse::Error { message } => Err(anyhow!(message))
            .context("daemon protocol check failed; restart admuxd so it matches this admux build"),
        other => Err(anyhow!("unexpected hello response: {other:?}"))
            .context("daemon protocol check failed; restart admuxd so it matches this admux build"),
    }
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
        Err(error) if should_autostart_daemon(&error) => {
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
                                "failed to connect to admuxd at {} after autostart; see {}",
                                paths.socket_path.display(),
                                daemon_start_log_path(paths).display(),
                            )
                        });
                    }
                }
            }
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to connect to admuxd at {}; not attempting autostart",
                paths.socket_path.display()
            )
        }),
    }
}

fn should_autostart_daemon(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

fn spawn_daemon(paths: &RuntimePaths) -> Result<()> {
    let daemon_path = resolve_daemon_binary()?;
    let socket = paths.socket_path.display().to_string();
    let state = paths.state_path.display().to_string();
    let config = paths.config_path.display().to_string();
    let log_path = daemon_start_log_path(paths);
    let log_dir = log_path.parent().ok_or_else(|| {
        anyhow!("daemon startup log {} has no parent directory", log_path.display())
    })?;
    std::fs::create_dir_all(log_dir).with_context(|| {
        format!(
            "failed to create daemon startup log directory {}",
            log_dir.display()
        )
    })?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)
        .with_context(|| format!("failed to open daemon startup log {}", log_path.display()))?;
    log.set_permissions(std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to restrict daemon startup log {}", log_path.display()))?;
    let stderr_log = log
        .try_clone()
        .with_context(|| format!("failed to duplicate daemon startup log {}", log_path.display()))?;
    Command::new(daemon_path)
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .arg("--state")
        .arg(state)
        .arg("--config")
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr_log))
        .spawn()
        .context("failed to spawn admuxd")?;
    Ok(())
}

fn daemon_start_log_path(paths: &RuntimePaths) -> PathBuf {
    paths
        .state_path
        .parent()
        .map(|parent| parent.join("admuxd-startup.log"))
        .unwrap_or_else(|| PathBuf::from("admuxd-startup.log"))
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
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
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
    (&mut *stream)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut payload)
        .context("failed to read response payload")?;
    if payload.len() > 1024 * 1024 {
        bail!("response payload exceeds 1048576 byte limit");
    }
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
        CommandResponse::WorkspaceReady { session, created } => {
            if created {
                println!("workspace {session} ready");
            } else {
                println!("workspace {session} attached");
            }
        }
        CommandResponse::WorkspaceSaved { session, path } => {
            println!("saved {session} {}", path.display());
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
            if interactive_terminal_available() {
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
                if session.stale {
                    println!("{} (stale)", session.name);
                } else {
                    println!("{}", session.name);
                }
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
        CommandResponse::BufferList { buffers } => {
            for buffer in buffers {
                println!("{} {} {}", buffer.name, buffer.bytes, buffer.preview);
            }
        }
        CommandResponse::BufferShown { data, .. } => print!("{data}"),
        CommandResponse::BufferSet { name } => println!("set {name}"),
        CommandResponse::BufferDeleted { name } => println!("deleted {name}"),
        CommandResponse::BufferPasted { name } => println!("pasted {name}"),
        CommandResponse::BufferSaved { name, path } => {
            println!("saved {name} {}", path.display());
        }
        CommandResponse::BufferLoaded { name } => println!("loaded {name}"),
        CommandResponse::SessionPreview { .. } => {
            return Err(anyhow!("session preview responses are interactive-only"));
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
        | CommandResponse::InputRegistered
        | CommandResponse::FocusChanged => {}
        CommandResponse::ConfigReloaded => println!("config reloaded"),
        CommandResponse::Error { message } => return Err(anyhow!(message)),
    }
    Ok(())
}

fn handle_interactive_response(
    response: CommandResponse,
    status_message: &mut Option<String>,
) -> bool {
    match response {
        CommandResponse::Error { message } => {
            *status_message = Some(message);
            false
        }
        _ => true,
    }
}

struct TerminalRestore {
    mouse_capture_enabled: bool,
}

impl TerminalRestore {
    fn after_raw_mode_enabled() -> Self {
        Self {
            mouse_capture_enabled: false,
        }
    }

    fn enable_mouse_capture(&mut self, stdout: &mut io::Stdout) -> Result<()> {
        execute!(*stdout, EnableMouseCapture).context("failed to enable mouse capture")?;
        self.mouse_capture_enabled = true;
        Ok(())
    }
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        if self.mouse_capture_enabled {
            let _ = execute!(stdout, DisableMouseCapture);
        }
        let _ = execute!(
            stdout,
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags,
            Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn interactive_terminal_available() -> bool {
    interactive_terminal_available_for(
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        std::env::var_os("ADMUX_NONINTERACTIVE").is_none(),
    )
}

fn interactive_terminal_available_for(
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
    interactive_requested: bool,
) -> bool {
    stdin_is_terminal && stdout_is_terminal && interactive_requested
}

fn attach_interactive(paths: &RuntimePaths, session: &str) -> Result<()> {
    let mut config = load_config(paths)?;
    let mut stdout = io::stdout();
    let mut event_logger = EventLogger::from_env(paths)?;
    terminal::enable_raw_mode().context("failed to enable raw mode")?;
    let mut terminal_restore = TerminalRestore::after_raw_mode_enabled();
    execute!(
        stdout,
        EnterAlternateScreen,
        Hide,
        EnableBracketedPaste,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    )
    .context("failed to enter alternate screen")?;
    if config.mouse.enabled {
        terminal_restore.enable_mouse_capture(&mut stdout)?;
    }

    if let Some(logger) = event_logger.as_mut() {
        logger.log_line("attach session start")?;
    }

    run_attach_loop(
        paths,
        session.to_string(),
        &mut config,
        &mut stdout,
        event_logger.as_mut(),
    )
}

fn run_attach_loop(
    paths: &RuntimePaths,
    mut current_session: String,
    config: &mut ResolvedConfig,
    stdout: &mut impl Write,
    event_logger: Option<&mut EventLogger>,
) -> Result<()> {
    let mut state = InputState::new(config.keys.clone(), config.behavior.resize_step);
    let mut last_size = (0, 0);
    let mut selection_anchor: Option<SelectionAnchor> = None;
    let mut active_selection: Option<PaneSelection> = None;
    let mut copy_mode: Option<CopyMode> = None;
    let mut resize_drag: Option<ResizeDrag> = None;
    let mut mouse_capture: Option<MouseCapture> = None;
    let mut status_message: Option<String> = None;
    let mut prompt_history = Vec::<String>::new();
    let mut overlay = OverlayState::None;
    let client_id = interactive_client_id();
    let mut snapshot = fetch_attach_snapshot(
        paths,
        &mut current_session,
        &mut last_size,
        80,
        24,
        Some(&client_id),
    )?;
    let mut snapshot_dirty = false;
    let mut render_dirty = true;
    let mut last_snapshot_refresh = Instant::now();
    let mut pending_event = None;
    let mut event_logger = event_logger;

    loop {
        let (width, height) = terminal::size().context("failed to read terminal size")?;
        let rows = height.max(1);
        let cols = width.max(1);
        if last_size != (rows, cols) {
            last_size = (rows, cols);
            let updated =
                fetch_attach_snapshot(paths, &mut current_session, &mut last_size, width, height, Some(&client_id))?;
            render_dirty |= updated != snapshot;
            render_dirty |= matches!(overlay, OverlayState::ChooseTree(_));
            snapshot = updated;
            snapshot_dirty = false;
            last_snapshot_refresh = Instant::now();
        } else if snapshot_dirty && snapshot_refresh_due(last_snapshot_refresh.elapsed()) {
            let updated = fetch_attach_snapshot(
                paths,
                &mut current_session,
                &mut last_size,
                width,
                height,
                Some(&client_id),
            )?;
            render_dirty |= updated != snapshot;
            render_dirty |= matches!(overlay, OverlayState::ChooseTree(_));
            snapshot = updated;
            snapshot_dirty = false;
            last_snapshot_refresh = Instant::now();
        }

        let render_selection = copy_mode
            .as_ref()
            .map(copy_mode_selection)
            .or(active_selection);
        let bottom_bar =
            copy_mode
                .as_ref()
                .map(|_| BottomBar::CopyMode)
                .unwrap_or(BottomBar::Status {
                    message: status_message.as_deref(),
                });

        if let Some(copy) = copy_mode.as_mut() {
            if let Some(pane) = snapshot
                .panes
                .iter()
                .find(|pane| pane.pane_id == copy.pane_id)
            {
                copy.clamp_to(
                    pane.rows_plain.len().max(1),
                    pane.rect.width.max(1) as usize,
                );
            } else {
                copy_mode = None;
                state.mode = InputMode::Normal;
            }
        }

        if render_dirty {
            match &mut overlay {
            OverlayState::None => {
                render_session(
                    stdout,
                    &current_session,
                    &snapshot,
                    bottom_bar,
                    render_selection,
                    &config.ui,
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
                    &config.ui,
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
                    &config.ui,
                    TerminalSize { width, height },
                )?;
            }
            OverlayState::ChooseBuffer(chooser) => {
                let preview = buffer_preview(paths, chooser)?;
                render_buffer_chooser(
                    stdout,
                    &current_session,
                    &snapshot,
                    &chooser.buffers,
                    chooser.selected,
                    &preview,
                    &config.ui,
                    TerminalSize { width, height },
                )?;
            }
            OverlayState::Help => {
                render_help_overlay(
                    stdout,
                    &current_session,
                    &snapshot,
                    &help_lines(),
                    &config.ui,
                    TerminalSize { width, height },
                )?;
            }
            }
            render_dirty = false;
        }
        if !event::poll(ATTACH_INPUT_POLL_INTERVAL).context("failed to poll terminal events")? {
            if snapshot_refresh_due(last_snapshot_refresh.elapsed()) {
                let updated = fetch_attach_snapshot(
                    paths,
                    &mut current_session,
                    &mut last_size,
                    width,
                    height,
                    Some(&client_id),
                )?;
                render_dirty |= updated != snapshot;
                render_dirty |= matches!(overlay, OverlayState::ChooseTree(_));
                snapshot = updated;
                snapshot_dirty = false;
                last_snapshot_refresh = Instant::now();
            }
            continue;
        }

        let mut needs_refresh = false;
        let mut refresh_before_next_input = false;
        match read_attach_event(&mut pending_event, event_logger.as_deref_mut())? {
            Event::Key(key) => {
                let current_overlay = std::mem::replace(&mut overlay, OverlayState::None);
                match current_overlay {
                    OverlayState::Prompt(mut prompt) => {
                        match handle_prompt_key(
                            paths,
                            &snapshot,
                            &mut current_session,
                            &mut last_size,
                            &mut state,
                            config,
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
                                needs_refresh = matches!(key.code, KeyCode::Enter);
                                selection_anchor = None;
                                active_selection = None;
                            }
                            PromptResult::OpenChooseTree => {
                                overlay = OverlayState::ChooseTree(build_choose_tree(
                                    paths,
                                    &current_session,
                                )?);
                            }
                            PromptResult::OpenChooseBuffer => {
                                overlay = OverlayState::ChooseBuffer(build_choose_buffer(paths)?);
                            }
                            PromptResult::Detach => break,
                        }
                    }
                    OverlayState::ChooseTree(mut tree) => {
                        if handle_choose_tree_key(
                            paths,
                            &mut tree,
                            key,
                            &mut current_session,
                            &mut last_size,
                            &mut status_message,
                        )? {
                            overlay = OverlayState::ChooseTree(tree);
                        } else {
                            needs_refresh = true;
                        }
                    }
                    OverlayState::ChooseBuffer(mut chooser) => {
                        if handle_choose_buffer_key(
                            paths,
                            &mut chooser,
                            key,
                            &current_session,
                            &mut status_message,
                        )? {
                            overlay = OverlayState::ChooseBuffer(chooser);
                        } else {
                            needs_refresh = true;
                        }
                    }
                    OverlayState::Help => match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {}
                        _ => overlay = OverlayState::Help,
                    },
                    OverlayState::None => {
                        let mode_before = state.mode;
                        let application_cursor = focused_pane(&snapshot)
                            .map(|pane| pane.application_cursor)
                            .unwrap_or(false);
                        let action = state.handle_key_with_application_cursor(key, application_cursor);
                        refresh_before_next_input = action_changes_input_target(&action);
                        if let Some(logger) = event_logger.as_deref_mut() {
                            logger.log_line(&format!(
                                "handled: key={key:?} mode_before={mode_before:?} mode_after={:?} action={action:?}",
                                state.mode
                            ))?;
                        }
                        match action {
                        InputAction::Noop => {}
                        InputAction::Detach => break,
                        InputAction::EnterCopyMode => {
                            if let Some(pane) = focused_pane(&snapshot) {
                                copy_mode = Some(copy_mode_from_pane(pane));
                                active_selection = None;
                                selection_anchor = None;
                            }
                        }
                        InputAction::ExitCopyMode => {
                            copy_mode = None;
                        }
                        InputAction::SendBytes(bytes) => {
                            send_input_bytes(
                                paths,
                                &snapshot,
                                &current_session,
                                &bytes,
                                Some(&client_id),
                            )?;
                            needs_refresh = true;
                        }
                        InputAction::SplitPane(axis) => {
                            let response = request_response(
                                paths,
                                CommandRequest::SplitPane {
                                    target: current_session.clone(),
                                    axis,
                                    command: Vec::new(),
                                },
                            )?;
                            needs_refresh = handle_interactive_response(response, &mut status_message);
                        }
                        InputAction::SelectWindowIndex(index) => {
                            if let Some(window) = snapshot
                                .windows
                                .iter()
                                .find(|window| window.index == index as u64)
                            {
                                if let Some(logger) = event_logger.as_deref_mut() {
                                    logger.log_line(&format!(
                                        "select-window-hit: requested={index} window_id={} window_index={}",
                                        window.id, window.index
                                    ))?;
                                }
                                let response = request_response(
                                    paths,
                                    CommandRequest::SelectWindow {
                                        target: format!("{}:{}", current_session, window.index),
                                    },
                                )?;
                                if let Some(logger) = event_logger.as_deref_mut() {
                                    logger.log_line(&format!(
                                        "select-window-response: {response:?}"
                                    ))?;
                                }
                                needs_refresh = handle_interactive_response(response, &mut status_message);
                            } else if let Some(logger) = event_logger.as_deref_mut() {
                                logger.log_line(&format!(
                                    "select-window-miss: requested={index} snapshot_indexes={:?}",
                                    snapshot
                                        .windows
                                        .iter()
                                        .map(|window| window.index)
                                        .collect::<Vec<_>>()
                                ))?;
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
                            let response = request_response(
                                paths,
                                CommandRequest::NewWindow {
                                    session: current_session.clone(),
                                    name: None,
                                    command: Vec::new(),
                                },
                            )?;
                            needs_refresh = handle_interactive_response(response, &mut status_message);
                        }
                        InputAction::NextWindow => {
                            let response = request_response(
                                paths,
                                CommandRequest::CycleWindow {
                                    session: current_session.clone(),
                                    direction: CycleDirection::Next,
                                },
                            )?;
                            needs_refresh = handle_interactive_response(response, &mut status_message);
                        }
                        InputAction::PrevWindow => {
                            let response = request_response(
                                paths,
                                CommandRequest::CycleWindow {
                                    session: current_session.clone(),
                                    direction: CycleDirection::Prev,
                                },
                            )?;
                            needs_refresh = handle_interactive_response(response, &mut status_message);
                        }
                        InputAction::FocusPane(direction) => {
                            let response = request_response(
                                paths,
                                CommandRequest::SelectPane {
                                    target: Some(current_session.clone()),
                                    direction: Some(direction),
                                },
                            )?;
                            needs_refresh = handle_interactive_response(response, &mut status_message);
                        }
                        InputAction::ResizePane(direction, amount) => {
                            let response = request_response(
                                paths,
                                CommandRequest::ResizePane {
                                    target: current_session.clone(),
                                    direction,
                                    amount,
                                },
                            )?;
                            needs_refresh = handle_interactive_response(response, &mut status_message);
                        }
                        InputAction::KillPane => {
                            let response = request_response(
                                paths,
                                CommandRequest::KillPane {
                                    target: current_session.clone(),
                                },
                            )?;
                            needs_refresh = handle_interactive_response(response, &mut status_message);
                        }
                        InputAction::PasteTopBuffer => {
                            let response = request_response(
                                paths,
                                CommandRequest::PasteBuffer {
                                    target: current_session.clone(),
                                    buffer: None,
                                },
                            )?;
                            needs_refresh = handle_interactive_response(response, &mut status_message);
                        }
                        InputAction::ListBuffers => {
                            let response = request_response(paths, CommandRequest::ListBuffers)?;
                            status_message = Some(format_list_response(response));
                        }
                        InputAction::DeleteTopBuffer => {
                            let response = request_response(
                                paths,
                                CommandRequest::DeleteBuffer { buffer: None },
                            )?;
                            status_message = Some(format_list_response(response));
                            needs_refresh = true;
                        }
                        InputAction::ChooseBuffer => {
                            overlay = OverlayState::ChooseBuffer(build_choose_buffer(paths)?);
                        }
                        InputAction::ReloadConfig => {
                            match reload_interactive_config(paths, &mut state, config) {
                                Ok(()) => {
                                status_message = Some("config reloaded".into());
                                needs_refresh = true;
                                }
                                Err(error) => status_message = Some(error.to_string()),
                            }
                        }
                        InputAction::CopyMove(direction) => {
                            let pane_dims = copy_mode.as_ref().and_then(|copy| {
                                snapshot
                                    .panes
                                    .iter()
                                    .find(|pane| pane.pane_id == copy.pane_id)
                                    .map(|pane| {
                                        (
                                            pane.rows_plain.len().max(1),
                                            pane.rect.width.max(1) as usize,
                                        )
                                    })
                            });
                            if let (Some(copy), Some((rows, cols))) =
                                (copy_mode.as_mut(), pane_dims)
                            {
                                match direction {
                                    NavigationDirection::Left => copy.move_left(),
                                    NavigationDirection::Right => copy.move_right(cols),
                                    NavigationDirection::Up => copy.move_up(),
                                    NavigationDirection::Down => copy.move_down(rows),
                                }
                            }
                        }
                        InputAction::CopyLineStart => {
                            if let Some(copy) = copy_mode.as_mut() {
                                copy.move_line_start();
                            }
                        }
                        InputAction::CopyLineEnd => {
                            let line = copy_mode.as_ref().and_then(|copy| {
                                snapshot
                                    .panes
                                    .iter()
                                    .find(|pane| pane.pane_id == copy.pane_id)
                                    .and_then(|pane| {
                                        pane.rows_plain.get(copy.cursor_row as usize).cloned()
                                    })
                            });
                            if let (Some(copy), Some(line)) = (copy_mode.as_mut(), line) {
                                copy.move_line_end(&line);
                            }
                        }
                        InputAction::CopyTop => {
                            if let Some(copy) = copy_mode.as_mut() {
                                copy.move_top();
                            }
                        }
                        InputAction::CopyBottom => {
                            let rows = copy_mode.as_ref().and_then(|copy| {
                                snapshot
                                    .panes
                                    .iter()
                                    .find(|pane| pane.pane_id == copy.pane_id)
                                    .map(|pane| pane.rows_plain.len().max(1))
                            });
                            if let (Some(copy), Some(rows)) = (copy_mode.as_mut(), rows) {
                                copy.move_bottom(rows);
                            }
                        }
                        InputAction::CopyPageUp => {
                            if let Some(copy) = copy_mode.as_mut() {
                                let page = config
                                    .behavior
                                    .copy_page_size
                                    .map(|value| value.min(i16::MAX as u16) as i16)
                                    .unwrap_or_else(|| {
                                        snapshot
                                            .panes
                                            .iter()
                                            .find(|pane| pane.pane_id == copy.pane_id)
                                            .map(|pane| pane.rect.height.max(1) as i16)
                                            .unwrap_or(10)
                                    });
                                let _ = request_response(
                                    paths,
                                    CommandRequest::ScrollPane {
                                        session: current_session.clone(),
                                        window_id: Some(snapshot.active_window_id),
                                        pane_id: Some(copy.pane_id),
                                        lines: -page,
                                    },
                                )?;
                                needs_refresh = true;
                            }
                        }
                        InputAction::CopyPageDown => {
                            if let Some(copy) = copy_mode.as_mut() {
                                let page = config
                                    .behavior
                                    .copy_page_size
                                    .map(|value| value.min(i16::MAX as u16) as i16)
                                    .unwrap_or_else(|| {
                                        snapshot
                                            .panes
                                            .iter()
                                            .find(|pane| pane.pane_id == copy.pane_id)
                                            .map(|pane| pane.rect.height.max(1) as i16)
                                            .unwrap_or(10)
                                    });
                                let _ = request_response(
                                    paths,
                                    CommandRequest::ScrollPane {
                                        session: current_session.clone(),
                                        window_id: Some(snapshot.active_window_id),
                                        pane_id: Some(copy.pane_id),
                                        lines: page,
                                    },
                                )?;
                                needs_refresh = true;
                            }
                        }
                        InputAction::CopyStartSelection => {
                            if let Some(copy) = copy_mode.as_mut() {
                                copy.start_selection();
                            }
                        }
                        InputAction::CopyYank => {
                            if let Some(copy) = copy_mode.take() {
                                let selection =
                                    copy.selection().unwrap_or_else(|| copy.cursor_selection());
                                let copied = request_response(
                                    paths,
                                    CommandRequest::CopySelection {
                                        session: current_session.clone(),
                                        window_id: Some(snapshot.active_window_id),
                                        pane_id: Some(copy.pane_id),
                                        start_row: selection.start_row,
                                        start_col: selection.start_col,
                                        end_row: selection.end_row,
                                        end_col: selection.end_col,
                                    },
                                )?;
                                if let CommandResponse::SelectionCopied { text } = copied {
                                    status_message =
                                        copy_text_to_buffer_and_clipboard(
                                            paths,
                                            stdout,
                                            &text,
                                            &config.clipboard,
                                        )?;
                                }
                                needs_refresh = true;
                            }
                        }
                        }
                    }
                }
            }
            Event::Paste(text) => {
                if let OverlayState::Prompt(prompt) = &mut overlay {
                    insert_prompt_text(prompt, &text);
                } else if matches!(overlay, OverlayState::None) && copy_mode.is_none() {
                    let mut bytes = Vec::with_capacity(text.len() + 12);
                    bytes.extend_from_slice(b"\x1b[200~");
                    bytes.extend_from_slice(text.as_bytes());
                    bytes.extend_from_slice(b"\x1b[201~");
                    send_input_bytes(
                        paths,
                        &snapshot,
                        &current_session,
                        &bytes,
                        Some(&client_id),
                    )?;
                    needs_refresh = true;
                }
            }
            Event::Mouse(mouse) => {
                if matches!(overlay, OverlayState::None) && copy_mode.is_none() {
                    let local_repaint = handle_mouse_event(
                        paths,
                        &current_session,
                        &snapshot,
                        mouse,
                        config,
                        stdout,
                        &mut selection_anchor,
                        &mut active_selection,
                        &mut resize_drag,
                        &mut mouse_capture,
                        &mut status_message,
                    )?;
                    if local_repaint {
                        render_session(
                            stdout,
                            &current_session,
                            &snapshot,
                            BottomBar::Status {
                                message: status_message.as_deref(),
                            },
                            active_selection,
                            &config.ui,
                            TerminalSize { width, height },
                        )?;
                    } else {
                        needs_refresh = true;
                    }
                }
            }
            Event::Resize(_, _) => needs_refresh = true,
            Event::FocusGained | Event::FocusLost => {}
        }

        render_dirty = true;
        if needs_refresh {
            snapshot_dirty = true;
            if refresh_before_next_input
                || snapshot_refresh_due(last_snapshot_refresh.elapsed())
            {
                let updated = fetch_attach_snapshot(
                    paths,
                    &mut current_session,
                    &mut last_size,
                    width,
                    height,
                    Some(&client_id),
                )?;
                snapshot = updated;
                snapshot_dirty = false;
                last_snapshot_refresh = Instant::now();
            }
        }
    }
    Ok(())
}

fn action_changes_input_target(action: &InputAction) -> bool {
    matches!(
        action,
        InputAction::SplitPane(_)
            | InputAction::SelectWindowIndex(_)
            | InputAction::NewWindow
            | InputAction::NextWindow
            | InputAction::PrevWindow
            | InputAction::FocusPane(_)
            | InputAction::KillPane
    )
}

fn read_attach_event(
    pending_event: &mut Option<Event>,
    mut event_logger: Option<&mut EventLogger>,
) -> Result<Event> {
    let event = loop {
        let event = if let Some(event) = pending_event.take() {
            if let Some(logger) = event_logger.as_deref_mut() {
                logger.log_event("pending", &event)?;
            }
            event
        } else {
            let event = event::read().context("failed to read terminal event")?;
            if let Some(logger) = event_logger.as_deref_mut() {
                logger.log_event("raw", &event)?;
            }
            event
        };
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Release => continue,
            other => break other,
        }
    };

    let Event::Key(key) = event else {
        return Ok(event);
    };

    if !matches!(key.code, KeyCode::Esc) || !key.modifiers.is_empty() {
        let event = Event::Key(key);
        if let Some(logger) = event_logger.as_deref_mut() {
            logger.log_event("normalized", &event)?;
        }
        return Ok(event);
    }

    if !event::poll(ALT_SEQUENCE_TIMEOUT).context("failed to poll terminal event")? {
        let event = Event::Key(key);
        if let Some(logger) = event_logger.as_deref_mut() {
            logger.log_event("normalized", &event)?;
        }
        return Ok(event);
    }

    let next = event::read().context("failed to read terminal event")?;
    if let Some(logger) = event_logger.as_deref_mut() {
        logger.log_event("raw-followup", &next)?;
    }
    let event = coalesce_escape_digit_event(Event::Key(key), next, pending_event);
    if let Some(logger) = event_logger.as_deref_mut() {
        logger.log_event("normalized", &event)?;
    }
    Ok(event)
}

fn coalesce_escape_digit_event(event: Event, next: Event, pending_event: &mut Option<Event>) -> Event {
    let Event::Key(key) = event else {
        return event;
    };
    if !matches!(key.code, KeyCode::Esc) || !key.modifiers.is_empty() {
        return Event::Key(key);
    }
    match next {
        Event::Key(next_key)
            if matches!(next_key.code, KeyCode::Char('1'..='9')) && next_key.modifiers.is_empty() =>
        {
            Event::Key(KeyEvent::new(next_key.code, KeyModifiers::ALT))
        }
        other => {
            *pending_event = Some(other);
            Event::Key(key)
        }
    }
}

impl EventLogger {
    fn from_env(paths: &RuntimePaths) -> Result<Option<Self>> {
        let Some(target) = std::env::var_os("ADMUX_KEY_LOG") else {
            return Ok(None);
        };
        let path = if target == "1" {
            paths.socket_dir().join("attach-events.log")
        } else {
            PathBuf::from(target)
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create event log directory {}", parent.display()))?;
        }
        let out = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open event log {}", path.display()))?;
        let mut logger = Self { out };
        logger.log_line(&format!("log path {}", path.display()))?;
        Ok(Some(logger))
    }

    fn log_event(&mut self, stage: &str, event: &Event) -> Result<()> {
        self.log_line(&format!("{stage}: {event:?}"))
    }

    fn log_line(&mut self, line: &str) -> Result<()> {
        writeln!(self.out, "{line}").context("failed to write event log")
    }

    fn log_snapshot_summary(&mut self, session: &str, snapshot: &RenderSnapshot) -> Result<()> {
        let active_windows = snapshot
            .windows
            .iter()
            .filter(|window| window.active)
            .map(|window| format!("id={} index={} name={}", window.id, window.index, window.name))
            .collect::<Vec<_>>();
        self.log_line(&format!(
            "snapshot: session={session} windows={:?} active_windows={:?}",
            snapshot
                .windows
                .iter()
                .map(|window| (window.id, window.index, window.active))
                .collect::<Vec<_>>(),
            active_windows
        ))
    }
}

fn fetch_attach_snapshot(
    paths: &RuntimePaths,
    current_session: &mut String,
    last_size: &mut (u16, u16),
    width: u16,
    height: u16,
    client_id: Option<&str>,
) -> Result<RenderSnapshot> {
    let response = request_response(
        paths,
        CommandRequest::Attach {
            session: Some(current_session.clone()),
            viewport: client_id.map(|client_id| ClientViewport {
                client_id: client_id.to_string(),
                rows: height.max(1),
                cols: width.max(1),
            }),
        },
    )?;
    apply_attached_session(&response, current_session, last_size);
    let snapshot = match response {
        CommandResponse::Attached {
            preview, snapshot, ..
        } => snapshot.unwrap_or_else(|| fallback_snapshot(preview, width, height)),
        CommandResponse::Error { message } => return Err(anyhow!(message)),
        other => return Err(anyhow!("unexpected attach response: {other:?}")),
    };
    if let Some(mut logger) = EventLogger::from_env(paths)? {
        logger.log_snapshot_summary(current_session, &snapshot)?;
    }
    Ok(snapshot)
}

fn load_config(paths: &RuntimePaths) -> Result<ResolvedConfig> {
    if !paths.config_path.exists() {
        return Config::default().resolve();
    }
    Config::load_from_path(&paths.config_path)?.resolve()
}

fn numbering_from_config(config: &ResolvedConfig) -> Numbering {
    Numbering {
        window_base: config.behavior.window_base,
        pane_base: config.behavior.pane_base,
    }
}

fn run_alias_command(paths: &RuntimePaths, args: AliasArgs) -> Result<()> {
    let mut registry = AliasRegistry::load(&paths.aliases_path)?;
    match args.command {
        AliasCommand::Add(AliasAddArgs { name, path }) => {
            let config = load_config(paths)?;
            let manifest = registry.add(
                &name,
                path.as_deref(),
                numbering_from_config(&config),
                crate::cli::TOP_LEVEL_COMMAND_NAMES,
            )?;
            registry.save(&paths.aliases_path)?;
            println!("added alias {name} {}", manifest.display());
        }
        AliasCommand::List => {
            for (name, path) in registry.list() {
                println!("{name} {}", path.display());
            }
        }
        AliasCommand::Remove(args) => {
            registry.remove(&args.name)?;
            registry.save(&paths.aliases_path)?;
            println!("removed alias {}", args.name);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_prompt_key(
    paths: &RuntimePaths,
    snapshot: &RenderSnapshot,
    current_session: &mut String,
    last_size: &mut (u16, u16),
    input_state: &mut InputState,
    config: &mut ResolvedConfig,
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
                match parse_command(&command) {
                    Ok(parsed) => {
                        if let Some(result) = prompt_overlay_command(&parsed) {
                            history.push(command);
                            return Ok(result);
                        }
                        if matches!(parsed, InteractiveCommand::ReloadConfig) {
                            match reload_interactive_config(paths, input_state, config) {
                                Ok(()) => {
                                    history.push(command);
                                    *status_message = Some("config reloaded".into());
                                }
                                Err(error) => {
                                    *status_message = Some(error.to_string());
                                    return Ok(PromptResult::KeepOpen);
                                }
                            }
                        } else {
                            match execute_prompt_command(
                                paths,
                                snapshot,
                                current_session,
                                last_size,
                                &command,
                            ) {
                                Ok(result) => {
                                    history.push(command);
                                    *status_message = result;
                                }
                                Err(error) => {
                                    *status_message = Some(error.to_string());
                                    return Ok(PromptResult::KeepOpen);
                                }
                            }
                        }
                    }
                    Err(_) => match execute_prompt_command(
                        paths,
                        snapshot,
                        current_session,
                        last_size,
                        &command,
                    ) {
                        Ok(result) => {
                            history.push(command);
                            *status_message = result;
                        }
                        Err(error) => {
                            *status_message = Some(error.to_string());
                            return Ok(PromptResult::KeepOpen);
                        }
                    },
                };
            }
            return Ok(PromptResult::CloseAndClearSelection);
        }
        KeyCode::Tab => {
            cycle_prompt_completion(prompt);
            return Ok(PromptResult::KeepOpen);
        }
        KeyCode::Backspace => {
            if let Some(previous) = previous_char_boundary(&prompt.buffer, prompt.cursor) {
                prompt.buffer.drain(previous..prompt.cursor);
                prompt.cursor = previous;
            }
        }
        KeyCode::Delete => {
            if prompt.cursor < prompt.buffer.len() {
                let next = next_char_boundary(&prompt.buffer, prompt.cursor)
                    .expect("cursor before string end must have a following character");
                prompt.buffer.drain(prompt.cursor..next);
            }
        }
        KeyCode::Left => {
            if let Some(previous) = previous_char_boundary(&prompt.buffer, prompt.cursor) {
                prompt.cursor = previous;
            }
        }
        KeyCode::Right => {
            if let Some(next) = next_char_boundary(&prompt.buffer, prompt.cursor) {
                prompt.cursor = next;
            }
        }
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
            prompt.cursor += ch.len_utf8();
        }
        _ => {}
    }

    refresh_prompt_completions(prompt);
    Ok(PromptResult::KeepOpen)
}

fn prompt_overlay_command(command: &InteractiveCommand) -> Option<PromptResult> {
    match command {
        InteractiveCommand::ChooseTree => Some(PromptResult::OpenChooseTree),
        InteractiveCommand::ChooseBuffer => Some(PromptResult::OpenChooseBuffer),
        InteractiveCommand::DetachClient => Some(PromptResult::Detach),
        _ => None,
    }
}

fn reload_interactive_config(
    paths: &RuntimePaths,
    input_state: &mut InputState,
    config: &mut ResolvedConfig,
) -> Result<()> {
    match request_response(paths, CommandRequest::ReloadConfig)? {
        CommandResponse::ConfigReloaded => {
            let reloaded = load_config(paths)?;
            input_state.replace_config(reloaded.keys.clone(), reloaded.behavior.resize_step);
            *config = reloaded;
            Ok(())
        }
        CommandResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected reload response: {other:?}")),
    }
}

fn insert_prompt_text(prompt: &mut PromptState, text: &str) {
    prompt.buffer.insert_str(prompt.cursor, text);
    prompt.cursor += text.len();
    prompt.history_index = None;
    refresh_prompt_completions(prompt);
}

fn refresh_prompt_completions(prompt: &mut PromptState) {
    let prefix = prompt.buffer.split_whitespace().next().unwrap_or("");
    prompt.completions = command_completions(prefix);
    prompt.selected = 0;
}

fn cycle_prompt_completion(prompt: &mut PromptState) {
    let Some(selected_completion) = prompt.completions.get(prompt.selected) else {
        return;
    };
    if prompt.buffer == *selected_completion {
        prompt.selected = (prompt.selected + 1) % prompt.completions.len();
    }
    let Some(completion) = prompt.completions.get(prompt.selected).cloned() else {
        return;
    };
    prompt.buffer = completion;
    prompt.cursor = prompt.buffer.len();
}

fn previous_char_boundary(text: &str, cursor: usize) -> Option<usize> {
    text.get(..cursor)?.char_indices().next_back().map(|(index, _)| index)
}

fn next_char_boundary(text: &str, cursor: usize) -> Option<usize> {
    let suffix = text.get(cursor..)?;
    suffix.chars().next().map(|ch| cursor + ch.len_utf8())
}

fn execute_prompt_command(
    paths: &RuntimePaths,
    snapshot: &RenderSnapshot,
    current_session: &mut String,
    last_size: &mut (u16, u16),
    input: &str,
) -> Result<Option<String>> {
    match parse_command(input).map_err(anyhow::Error::msg)? {
        InteractiveCommand::SplitWindow { horizontal } => {
            ensure_command_succeeded(request_response(
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
            )?)?;
            Ok(None)
        }
        InteractiveCommand::NewWindow => {
            ensure_command_succeeded(request_response(
                paths,
                CommandRequest::NewWindow {
                    session: current_session.clone(),
                    name: None,
                    command: Vec::new(),
                },
            )?)?;
            Ok(None)
        }
        InteractiveCommand::SelectWindow { target } => {
            let target = resolve_window_target(snapshot, current_session, &target);
            ensure_command_succeeded(request_response(paths, CommandRequest::SelectWindow { target })?)?;
            Ok(None)
        }
        InteractiveCommand::NextWindow => {
            ensure_command_succeeded(request_response(
                paths,
                CommandRequest::CycleWindow {
                    session: current_session.clone(),
                    direction: CycleDirection::Next,
                },
            )?)?;
            Ok(None)
        }
        InteractiveCommand::PreviousWindow => {
            ensure_command_succeeded(request_response(
                paths,
                CommandRequest::CycleWindow {
                    session: current_session.clone(),
                    direction: CycleDirection::Prev,
                },
            )?)?;
            Ok(None)
        }
        InteractiveCommand::KillPane => {
            ensure_command_succeeded(request_response(
                paths,
                CommandRequest::KillPane {
                    target: current_session.clone(),
                },
            )?)?;
            Ok(None)
        }
        InteractiveCommand::KillWindow => {
            let target = format!("{}:{}", current_session, snapshot.active_window_id);
            ensure_command_succeeded(request_response(paths, CommandRequest::KillWindow { target })?)?;
            Ok(None)
        }
        InteractiveCommand::AttachSession { target }
        | InteractiveCommand::SwitchClient { target } => {
            let response = request_response(
                paths,
                CommandRequest::Attach {
                    session: Some(target.clone()),
                    viewport: None,
                },
            )?;
            apply_prompt_session_switch(current_session, last_size, response)?;
            Ok(None)
        }
        InteractiveCommand::ListSessions => {
            let response = ensure_command_succeeded(request_response(paths, CommandRequest::ListSessions)?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::ListWindows => {
            let response = ensure_command_succeeded(request_response(
                paths,
                CommandRequest::ListWindows {
                    session: current_session.clone(),
                },
            )?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::ListPanes => {
            let response = ensure_command_succeeded(request_response(
                paths,
                CommandRequest::ListPanes {
                    target: current_session.clone(),
                },
            )?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::ListBuffers => {
            let response = ensure_command_succeeded(request_response(paths, CommandRequest::ListBuffers)?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::ShowBuffer { buffer } => {
            let response = ensure_command_succeeded(request_response(paths, CommandRequest::ShowBuffer { buffer })?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::DeleteBuffer { buffer } => {
            let response = ensure_command_succeeded(request_response(paths, CommandRequest::DeleteBuffer { buffer })?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::PasteBuffer { buffer, target } => {
            let response = ensure_command_succeeded(request_response(
                paths,
                CommandRequest::PasteBuffer {
                    target: target.unwrap_or_else(|| current_session.clone()),
                    buffer,
                },
            )?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::SetBuffer { buffer, data } => {
            let response = ensure_command_succeeded(request_response(
                paths,
                CommandRequest::SetBuffer {
                    buffer,
                    data,
                    append: false,
                },
            )?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::SaveBuffer { buffer, path } => {
            let response = ensure_command_succeeded(request_response(
                paths,
                CommandRequest::SaveBuffer {
                    buffer,
                    path: resolve_client_path(path.into())?,
                },
            )?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::LoadBuffer { buffer, path } => {
            let response = ensure_command_succeeded(request_response(
                paths,
                CommandRequest::LoadBuffer {
                    path: resolve_client_path(path.into())?,
                    buffer,
                },
            )?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::ChooseBuffer
        | InteractiveCommand::ChooseTree
        | InteractiveCommand::DetachClient => unreachable!("handled before prompt dispatch"),
        InteractiveCommand::RenameWindow { name } => {
            let target = format!("{}:{}", current_session, snapshot.active_window_id);
            ensure_command_succeeded(request_response(paths, CommandRequest::RenameWindow { target, name })?)?;
            Ok(None)
        }
        InteractiveCommand::SaveSession => {
            let response = ensure_command_succeeded(request_response(
                paths,
                CommandRequest::SaveWorkspace {
                    session: Some(current_session.clone()),
                },
            )?)?;
            Ok(Some(format_list_response(response)))
        }
        InteractiveCommand::SendKeys { keys } => {
            ensure_command_succeeded(request_response(
                paths,
                CommandRequest::SendKeys {
                    target: current_session.clone(),
                    keys,
                },
            )?)?;
            Ok(None)
        }
        InteractiveCommand::ReloadConfig => {
            ensure_command_succeeded(request_response(paths, CommandRequest::ReloadConfig)?)?;
            Ok(Some("config reloaded".into()))
        }
    }
}

fn ensure_command_succeeded(response: CommandResponse) -> Result<CommandResponse> {
    match response {
        CommandResponse::Error { message } => Err(anyhow!(message)),
        response => Ok(response),
    }
}

fn apply_prompt_session_switch(
    current_session: &mut String,
    last_size: &mut (u16, u16),
    response: CommandResponse,
) -> Result<()> {
    match response {
        CommandResponse::Attached { session, .. } => {
            switch_client_session(current_session, last_size, session);
            Ok(())
        }
        CommandResponse::Error { message } => Err(anyhow!(message)),
        other => Err(anyhow!("unexpected attach response: {other:?}")),
    }
}

fn switch_client_session(current_session: &mut String, last_size: &mut (u16, u16), session: String) {
    if *current_session != session {
        *current_session = session;
        *last_size = (0, 0);
    }
}

fn format_list_response(response: CommandResponse) -> String {
    match response {
        CommandResponse::SessionList { sessions } => sessions
            .into_iter()
            .map(|session| {
                if session.stale {
                    format!("{}(stale)", session.name)
                } else {
                    session.name
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
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
        CommandResponse::BufferList { buffers } => buffers
            .into_iter()
            .map(|buffer| format!("{}({})", buffer.name, buffer.bytes))
            .collect::<Vec<_>>()
            .join(" "),
        CommandResponse::BufferShown { data, .. } => data,
        CommandResponse::BufferSet { name } => format!("set {name}"),
        CommandResponse::BufferDeleted { name } => format!("deleted {name}"),
        CommandResponse::BufferPasted { name } => format!("pasted {name}"),
        CommandResponse::BufferSaved { name, path } => format!("saved {name} {}", path.display()),
        CommandResponse::BufferLoaded { name } => format!("loaded {name}"),
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
        && let Some(window) = snapshot
            .windows
            .iter()
            .find(|window| window.index == index as u64)
    {
        return format!("{session}:{}", window.index);
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
        preview: None,
    };
    rebuild_choose_tree(paths, &mut state)?;
    Ok(state)
}

fn build_choose_buffer(paths: &RuntimePaths) -> Result<ChooseBufferState> {
    let buffers = match request_response(paths, CommandRequest::ListBuffers)? {
        CommandResponse::BufferList { buffers } => buffers,
        other => return Err(anyhow!("unexpected buffer list response: {other:?}")),
    };
    Ok(ChooseBufferState {
        buffers,
        selected: 0,
        preview: None,
    })
}

fn rebuild_choose_tree(paths: &RuntimePaths, state: &mut ChooseTreeState) -> Result<()> {
    state.preview = None;
    let sessions = match request_response(paths, CommandRequest::ListSessions)? {
        CommandResponse::SessionList { sessions } => sessions,
        other => return Err(anyhow!("unexpected session list response: {other:?}")),
    };
    let mut items = Vec::new();
    let mut lines = Vec::new();

    for session in sessions {
        let session_name = session.name.clone();
        let expanded = state.expanded_sessions.contains(&session_name);
        items.push(ChooseItem::Session(session_name.clone()));
        let window_count = match request_response(
            paths,
            CommandRequest::ListWindows {
                session: session_name.clone(),
            },
        )? {
            CommandResponse::WindowList { windows } => windows.len(),
            _ => 0,
        };
        lines.push(TreeLine {
            depth: 0,
            label: if session_name == state.attached_session {
                if session.stale {
                    format!("{session_name}: {window_count} windows (attached, stale)")
                } else {
                    format!("{session_name}: {window_count} windows (attached)")
                }
            } else if session.stale {
                format!("{session_name}: {window_count} windows (stale)")
            } else {
                format!("{session_name}: {window_count} windows")
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
                session: session_name.clone(),
            },
        )? {
            CommandResponse::WindowList { windows } => windows,
            _ => Vec::new(),
        };
        for window in windows {
            let expanded_window = state
                .expanded_windows
                .contains(&(session_name.clone(), window.index));
            items.push(ChooseItem::Window {
                session: session_name.clone(),
                window_index: window.index,
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
                    target: format!("{session_name}:{}", window.index),
                },
            )? {
                CommandResponse::PaneList { panes } => panes,
                _ => Vec::new(),
            };
            for pane in panes {
                items.push(ChooseItem::Pane {
                    session: session_name.clone(),
                    window_index: window.index,
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
    tree: &mut ChooseTreeState,
) -> Result<(String, RenderSnapshot)> {
    if let Some((selected, title, snapshot)) = &tree.preview
        && *selected == tree.selected
    {
        return Ok((title.clone(), snapshot.clone()));
    }
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
    let target = match item {
        ChooseItem::Session(_) => None,
        ChooseItem::Window { window_index, .. } => Some(format!("{session}:{window_index}")),
        ChooseItem::Pane {
            window_index,
            pane_id,
            ..
        } => Some(format!("{session}:{window_index}.{pane_id}")),
    };
    let snapshot = match request_response(
        paths,
        CommandRequest::PreviewSession {
            session: session.clone(),
            target,
        },
    )? {
        CommandResponse::SessionPreview { snapshot } => snapshot,
        CommandResponse::Error { message } => return Err(anyhow!(message)),
        other => return Err(anyhow!("unexpected session preview response: {other:?}")),
    };
    tree.preview = Some((tree.selected, session.clone(), snapshot.clone()));
    Ok((session, snapshot))
}

fn buffer_preview(paths: &RuntimePaths, chooser: &mut ChooseBufferState) -> Result<String> {
    if let Some((selected, preview)) = &chooser.preview
        && *selected == chooser.selected
    {
        return Ok(preview.clone());
    }
    let Some(buffer) = chooser.buffers.get(chooser.selected) else {
        return Ok(String::new());
    };
    match request_response(
        paths,
        CommandRequest::ShowBuffer {
            buffer: Some(buffer.name.clone()),
        },
    )? {
        CommandResponse::BufferShown { data, .. } => {
            chooser.preview = Some((chooser.selected, data.clone()));
            Ok(data)
        }
        other => Err(anyhow!("unexpected buffer preview response: {other:?}")),
    }
}

fn handle_choose_tree_key(
    paths: &RuntimePaths,
    tree: &mut ChooseTreeState,
    key: crossterm::event::KeyEvent,
    current_session: &mut String,
    last_size: &mut (u16, u16),
    status_message: &mut Option<String>,
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
                        match request_response(
                            paths,
                            CommandRequest::Attach {
                                session: Some(session),
                                viewport: None,
                            },
                        )? {
                            CommandResponse::Attached { session, .. } => {
                                switch_client_session(current_session, last_size, session);
                                tree.attached_session = current_session.clone();
                            }
                            CommandResponse::Error { message } => {
                                *status_message = Some(message);
                                return Ok(true);
                            }
                            other => {
                                *status_message = Some(format!(
                                    "unexpected session attach response: {other:?}"
                                ));
                                return Ok(true);
                            }
                        }
                    }
                    ChooseItem::Window {
                        session,
                        window_index,
                    } => {
                        let response = request_response(
                            paths,
                            CommandRequest::SelectWindow {
                                target: format!("{session}:{window_index}"),
                            },
                        )?;
                        if !chooser_command_succeeded(response, status_message) {
                            return Ok(true);
                        }
                        switch_client_session(current_session, last_size, session);
                        tree.attached_session = current_session.clone();
                    }
                    ChooseItem::Pane {
                        session,
                        window_index,
                        pane_id,
                    } => {
                        let response = request_response(
                            paths,
                            CommandRequest::SelectWindow {
                                target: format!("{session}:{window_index}"),
                            },
                        )?;
                        if !chooser_command_succeeded(response, status_message) {
                            return Ok(true);
                        }
                        let response = request_response(
                            paths,
                            CommandRequest::SelectPane {
                                target: Some(format!("{session}:{window_index}.{pane_id}")),
                                direction: None,
                            },
                        )?;
                        if !chooser_command_succeeded(response, status_message) {
                            return Ok(true);
                        }
                        switch_client_session(current_session, last_size, session);
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

fn chooser_command_succeeded(response: CommandResponse, status_message: &mut Option<String>) -> bool {
    match response {
        CommandResponse::Error { message } => {
            *status_message = Some(message);
            false
        }
        _ => true,
    }
}

fn handle_choose_buffer_key(
    paths: &RuntimePaths,
    chooser: &mut ChooseBufferState,
    key: crossterm::event::KeyEvent,
    current_session: &str,
    status_message: &mut Option<String>,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return Ok(false),
        KeyCode::Up => {
            if chooser.selected > 0 {
                chooser.selected -= 1;
            }
        }
        KeyCode::Down => {
            if chooser.selected + 1 < chooser.buffers.len() {
                chooser.selected += 1;
            }
        }
        KeyCode::Char('d') => {
            if let Some(buffer) = chooser.buffers.get(chooser.selected) {
                let response = request_response(
                    paths,
                    CommandRequest::DeleteBuffer {
                        buffer: Some(buffer.name.clone()),
                    },
                )?;
                *status_message = Some(format_list_response(response));
                *chooser = build_choose_buffer(paths)?;
                if !chooser.buffers.is_empty() {
                    chooser.selected = chooser.selected.min(chooser.buffers.len() - 1);
                }
            }
        }
        KeyCode::Enter | KeyCode::Char('p') => {
            if let Some(buffer) = chooser.buffers.get(chooser.selected) {
                let response = request_response(
                    paths,
                    CommandRequest::PasteBuffer {
                        target: current_session.to_string(),
                        buffer: Some(buffer.name.clone()),
                    },
                )?;
                *status_message = Some(format_list_response(response));
            }
            return Ok(false);
        }
        _ => {}
    }
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
            ChooseItem::Window {
                session,
                window_index,
            } => {
                let key = (session.clone(), *window_index);
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
            ChooseItem::Window {
                session,
                window_index,
            } => {
                let key = (session.clone(), *window_index);
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
    let sessions: Vec<String> = match request_response(paths, CommandRequest::ListSessions)? {
        CommandResponse::SessionList { sessions } => {
            sessions.into_iter().map(|session| session.name).collect()
        }
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
            tree.expanded_windows.insert((session.clone(), window.index));
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

fn focused_pane(snapshot: &RenderSnapshot) -> Option<&PaneRender> {
    snapshot.panes.iter().find(|pane| pane.focused)
}

fn copy_mode_from_pane(pane: &PaneRender) -> CopyMode {
    let cursor = pane.cursor.clone().unwrap_or(PaneCursor { row: 0, col: 0 });
    let mut mode = CopyMode::new(pane.pane_id, cursor.row, cursor.col);
    mode.clamp_to(
        pane.rows_plain.len().max(1),
        pane.rect.width.max(1) as usize,
    );
    mode
}

fn copy_mode_selection(copy_mode: &CopyMode) -> PaneSelection {
    PaneSelection {
        pane_id: copy_mode.pane_id,
        selection: copy_mode
            .selection()
            .unwrap_or_else(|| copy_mode.cursor_selection()),
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
        "Ctrl-b [      copy mode".into(),
        "Ctrl-b ]      paste top buffer".into(),
        "Ctrl-b #      list buffers".into(),
        "Ctrl-b -      delete top buffer".into(),
        "Ctrl-b =      choose buffer".into(),
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
    mut mouse: MouseEvent,
    config: &ResolvedConfig,
    stdout: &mut impl Write,
    selection_anchor: &mut Option<SelectionAnchor>,
    active_selection: &mut Option<PaneSelection>,
    resize_drag: &mut Option<ResizeDrag>,
    mouse_capture: &mut Option<MouseCapture>,
    status_message: &mut Option<String>,
) -> Result<bool> {
    let ui = &config.ui;
    if !config.mouse.enabled {
        return Ok(false);
    }
    if matches!(ui.status_position, StatusPosition::Top) {
        if mouse.row == 0 {
            return Ok(false);
        }
        mouse.row = mouse.row.saturating_sub(1);
    }
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if config.mouse.border_resize
                && let Some((pane, direction)) = separator_hit(snapshot, mouse.row, mouse.column)
            {
                *resize_drag = Some(ResizeDrag {
                    pane_id: pane.pane_id,
                    direction,
                    last_row: mouse.row,
                    last_col: mouse.column,
                    span: resize_drag_span(snapshot, direction),
                });
            } else if let Some((pane, row, col)) =
                pane_content_hit(snapshot, mouse.row, mouse.column)
            {
                if config.mouse.focus_on_click {
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
                }
                if pane.mouse_reporting {
                send_pane_mouse(snapshot, pane.pane_id, row, col, HelperMouseEventKind::LeftDown)
                    .or_else(|_| {
                        request_response(
                            paths,
                            CommandRequest::MousePane {
                                session: session.to_string(),
                                window_id: snapshot.active_window_id,
                                pane_id: pane.pane_id,
                                row,
                                col,
                                kind: PaneMouseKind::LeftDown,
                            },
                        )
                        .map(|_| ())
                    })?;
                    *mouse_capture = Some(MouseCapture {
                        pane_id: pane.pane_id,
                        button: MouseButton::Left,
                    });
                    *selection_anchor = None;
                    *active_selection = None;
                    return Ok(false);
                }
                if config.mouse.selection_copy {
                    *selection_anchor = Some(SelectionAnchor {
                        pane_id: pane.pane_id,
                        row,
                        col,
                    });
                    *active_selection = Some(PaneSelection {
                        pane_id: pane.pane_id,
                        selection: Selection::new(row, col, row, col),
                    });
                    return Ok(true);
                }
            }
        }
        MouseEventKind::Down(button @ (MouseButton::Middle | MouseButton::Right)) => {
            if let Some((pane, row, col)) = pane_content_hit(snapshot, mouse.row, mouse.column)
                && pane.mouse_reporting
            {
                if config.mouse.focus_on_click {
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
                }
                let (helper_down, _, _) = helper_mouse_kinds(button).expect("supported mouse button");
                let (pane_down, _, _) = pane_mouse_kinds(button).expect("supported mouse button");
                send_pane_mouse(snapshot, pane.pane_id, row, col, helper_down)
                    .or_else(|_| {
                        request_response(
                            paths,
                            CommandRequest::MousePane {
                                session: session.to_string(),
                                window_id: snapshot.active_window_id,
                                pane_id: pane.pane_id,
                                row,
                                col,
                                kind: pane_down,
                            },
                        )
                        .map(|_| ())
                    })?;
                *mouse_capture = Some(MouseCapture {
                    pane_id: pane.pane_id,
                    button,
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
                            amount: mouse_resize_amount(delta, resize.span),
                        },
                    )?;
                    resize.last_row = mouse.row;
                    resize.last_col = mouse.column;
                }
            } else if let Some(capture) = *mouse_capture
                && capture.button == MouseButton::Left
                && let Some((row, col)) = captured_pane_mouse_position(snapshot, capture, mouse)
            {
                send_pane_mouse(
                    snapshot,
                    capture.pane_id,
                    row,
                    col,
                    HelperMouseEventKind::LeftDrag,
                )
                .or_else(|_| {
                    request_response(
                        paths,
                        CommandRequest::MousePane {
                            session: session.to_string(),
                            window_id: snapshot.active_window_id,
                            pane_id: capture.pane_id,
                            row,
                            col,
                            kind: PaneMouseKind::LeftDrag,
                        },
                    )
                    .map(|_| ())
                })?;
            } else if let Some((pane, row, col)) =
                pane_content_hit(snapshot, mouse.row, mouse.column)
                && pane.mouse_reporting
            {
                send_pane_mouse(snapshot, pane.pane_id, row, col, HelperMouseEventKind::LeftDrag)
                    .or_else(|_| {
                        request_response(
                            paths,
                            CommandRequest::MousePane {
                                session: session.to_string(),
                                window_id: snapshot.active_window_id,
                                pane_id: pane.pane_id,
                                row,
                                col,
                                kind: PaneMouseKind::LeftDrag,
                            },
                        )
                        .map(|_| ())
                    })?;
            } else if config.mouse.selection_copy
                && let Some(anchor) = selection_anchor.as_ref()
                && let Some((pane, row, col)) = pane_content_hit(snapshot, mouse.row, mouse.column)
                && pane.pane_id == anchor.pane_id
            {
                *active_selection = Some(PaneSelection {
                    pane_id: pane.pane_id,
                    selection: Selection::new(anchor.row, anchor.col, row, col).normalized(),
                });
                return Ok(true);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            *resize_drag = None;
            if mouse_capture.is_some_and(|capture| capture.button == MouseButton::Left)
                && let Some(capture) = mouse_capture.take()
                && let Some((row, col)) = captured_pane_mouse_position(snapshot, capture, mouse)
            {
                send_pane_mouse(snapshot, capture.pane_id, row, col, HelperMouseEventKind::LeftUp)
                    .or_else(|_| {
                        request_response(
                            paths,
                            CommandRequest::MousePane {
                                session: session.to_string(),
                                window_id: snapshot.active_window_id,
                                pane_id: capture.pane_id,
                                row,
                                col,
                                kind: PaneMouseKind::LeftUp,
                            },
                        )
                        .map(|_| ())
                    })?;
                *selection_anchor = None;
                *active_selection = None;
                return Ok(false);
            }
            if config.mouse.selection_copy
                && let Some(anchor) = selection_anchor.take()
                && let Some((pane, row, col)) = pane_content_hit(snapshot, mouse.row, mouse.column)
                && pane.pane_id == anchor.pane_id
            {
                let selection = Selection::new(anchor.row, anchor.col, row, col).normalized();
                let copied = request_response(
                    paths,
                    CommandRequest::CopySelection {
                        session: session.to_string(),
                        window_id: Some(snapshot.active_window_id),
                        pane_id: Some(pane.pane_id),
                        start_row: selection.start_row,
                        start_col: selection.start_col,
                        end_row: selection.end_row,
                        end_col: selection.end_col,
                    },
                )?;
                if let CommandResponse::SelectionCopied { text } = copied {
                    *status_message = copy_text_to_buffer_and_clipboard(
                        paths,
                        stdout,
                        &text,
                        &config.clipboard,
                    )?;
                }
            }
            *active_selection = None;
            return Ok(true);
        }
        MouseEventKind::Drag(button @ (MouseButton::Middle | MouseButton::Right)) => {
            if let Some(capture) = *mouse_capture
                && capture.button == button
                && let Some((row, col)) = captured_pane_mouse_position(snapshot, capture, mouse)
            {
                let (_, helper_drag, _) = helper_mouse_kinds(button).expect("supported mouse button");
                let (_, pane_drag, _) = pane_mouse_kinds(button).expect("supported mouse button");
                send_pane_mouse(snapshot, capture.pane_id, row, col, helper_drag)
                    .or_else(|_| {
                        request_response(
                            paths,
                            CommandRequest::MousePane {
                                session: session.to_string(),
                                window_id: snapshot.active_window_id,
                                pane_id: capture.pane_id,
                                row,
                                col,
                                kind: pane_drag,
                            },
                        )
                        .map(|_| ())
                    })?;
            }
        }
        MouseEventKind::Up(button @ (MouseButton::Middle | MouseButton::Right)) => {
            if mouse_capture.is_some_and(|capture| capture.button == button)
                && let Some(capture) = mouse_capture.take()
                && let Some((row, col)) = captured_pane_mouse_position(snapshot, capture, mouse)
            {
                let (_, _, helper_up) = helper_mouse_kinds(button).expect("supported mouse button");
                let (_, _, pane_up) = pane_mouse_kinds(button).expect("supported mouse button");
                send_pane_mouse(snapshot, capture.pane_id, row, col, helper_up)
                    .or_else(|_| {
                        request_response(
                            paths,
                            CommandRequest::MousePane {
                                session: session.to_string(),
                                window_id: snapshot.active_window_id,
                                pane_id: capture.pane_id,
                                row,
                                col,
                                kind: pane_up,
                            },
                        )
                        .map(|_| ())
                    })?;
                return Ok(false);
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if !config.mouse.wheel_scroll {
                return Ok(false);
            }
            let Some((pane, row, col)) = pane_content_hit(snapshot, mouse.row, mouse.column) else {
                return Ok(false);
            };
            let direction = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                crate::ipc::ScrollDirection::Up
            } else {
                crate::ipc::ScrollDirection::Down
            };
            let response = request_response(
                paths,
                CommandRequest::MouseScroll {
                    session: session.to_string(),
                    window_id: snapshot.active_window_id,
                    pane_id: pane.pane_id,
                    row,
                    col,
                    direction,
                },
            )?;
            if !handle_interactive_response(response, status_message) {
                return Ok(true);
            }
        }
        _ => {}
    }
    Ok(false)
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

fn captured_pane_mouse_position(
    snapshot: &RenderSnapshot,
    capture: MouseCapture,
    mouse: MouseEvent,
) -> Option<(u16, u16)> {
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == capture.pane_id)?;
    let max_row = pane.rect.y.saturating_add(pane.rect.height.saturating_sub(1));
    let max_col = pane.rect.x.saturating_add(pane.rect.width.saturating_sub(1));
    let row = mouse.row.clamp(pane.rect.y, max_row).saturating_sub(pane.rect.y);
    let col = mouse
        .column
        .clamp(pane.rect.x, max_col)
        .saturating_sub(pane.rect.x);
    Some((row, col))
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

fn resize_drag_span(snapshot: &RenderSnapshot, direction: NavigationDirection) -> u16 {
    let span = match direction {
        NavigationDirection::Left | NavigationDirection::Right => snapshot
            .panes
            .iter()
            .map(|pane| pane.rect.right())
            .max(),
        NavigationDirection::Up | NavigationDirection::Down => snapshot
            .panes
            .iter()
            .map(|pane| pane.rect.bottom())
            .max(),
    }
    .unwrap_or(1);
    span.max(1)
}

fn mouse_resize_amount(delta_cells: u16, span: u16) -> u16 {
    let span = u32::from(span.max(1));
    let amount = (u32::from(delta_cells) * 1000).div_ceil(span);
    u16::try_from(amount.clamp(1, 100)).expect("clamped mouse resize amount fits u16")
}

fn fallback_snapshot(preview: String, width: u16, height: u16) -> RenderSnapshot {
    let rows_plain = preview.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    RenderSnapshot {
        sessions: Vec::new(),
        windows: vec![WindowSummary {
            id: 1,
            index: 0,
            name: "shell".into(),
            active: true,
            last_selected: false,
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
            helper_socket: None,
            mouse_reporting: false,
            application_cursor: false,
            preview: preview.clone(),
            formatted_preview: preview.clone(),
            formatted_cursor: String::new(),
            rows_formatted: rows_plain.clone(),
            rows_plain,
            cursor: Some(PaneCursor { row: 0, col: 0 }),
        }],
        dividers: Vec::new(),
        active_window_id: 1,
        active_pane_id: 1,
    }
}

fn send_input_bytes(
    paths: &RuntimePaths,
    snapshot: &RenderSnapshot,
    session: &str,
    bytes: &[u8],
    client_id: Option<&str>,
) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }

    if bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n'))
        && let Some(client_id) = client_id
    {
        ensure_command_succeeded(request_response(
            paths,
            CommandRequest::RegisterInput {
                source: SwitchSource {
                    session: session.to_string(),
                    window_id: snapshot.active_window_id,
                    pane_id: snapshot.active_pane_id,
                },
                client_id: client_id.to_string(),
            },
        )?)?;
    }

    if let Some(pane) = focused_pane(snapshot)
        && let Some(socket) = pane.helper_socket.clone()
        && PaneProcess::send_bytes_to(&socket, bytes).is_ok()
    {
        return Ok(());
    }

    ensure_command_succeeded(request_response(
        paths,
        CommandRequest::SendBytes {
            target: session.to_string(),
            bytes: bytes.to_vec(),
        },
    )?)?;
    Ok(())
}

fn send_pane_mouse(
    snapshot: &RenderSnapshot,
    pane_id: u64,
    row: u16,
    col: u16,
    kind: HelperMouseEventKind,
) -> Result<()> {
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .ok_or_else(|| anyhow!("unknown pane {pane_id}"))?;
    let socket = pane
        .helper_socket
        .clone()
        .ok_or_else(|| anyhow!("pane {} has no helper socket", pane_id))?;
    let process = PaneProcess::connect(socket)?;
    process.handle_mouse_event(kind, row, col)
}

fn copy_via_osc52(out: &mut impl Write, text: &str) -> Result<()> {
    let encoded = STANDARD.encode(text.as_bytes());
    write!(out, "\x1b]52;c;{encoded}\x07").context("failed to write OSC52 sequence")?;
    out.flush().context("failed to flush OSC52 sequence")?;
    Ok(())
}

fn copy_text_to_buffer_and_clipboard(
    paths: &RuntimePaths,
    out: &mut impl Write,
    text: &str,
    clipboard: &ClipboardConfig,
) -> Result<Option<String>> {
    let copied_chars = text.chars().count();
    if copied_chars == 0 {
        return Ok(None);
    }
    let buffer_name = match request_response(
        paths,
        CommandRequest::SetBuffer {
            buffer: None,
            data: text.to_string(),
            append: false,
        },
    )? {
        CommandResponse::BufferSet { name } => name,
        CommandResponse::Error { message } => return Err(anyhow!(message)),
        other => return Err(anyhow!("unexpected set-buffer response: {other:?}")),
    };
    copy_to_clipboard(clipboard, out, text)?;
    Ok(Some(format!(
        "copied {copied_chars} chars to {buffer_name}"
    )))
}

fn copy_to_clipboard(
    clipboard: &ClipboardConfig,
    out: &mut impl Write,
    text: &str,
) -> Result<()> {
    match clipboard.backend {
        ClipboardBackend::Osc52 => {
            copy_via_osc52(out, text).context("failed to send OSC52 clipboard copy")
        }
        ClipboardBackend::ExternalCommand => {
            let (program, arguments) = clipboard
                .command
                .split_first()
                .ok_or_else(|| anyhow!("clipboard.command is required for external-command"))?;
            let mut child = Command::new(program)
                .args(arguments)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .with_context(|| format!("failed to spawn clipboard command {program}"))?;
            child
                .stdin
                .as_mut()
                .ok_or_else(|| anyhow!("clipboard command stdin is unavailable"))?
                .write_all(text.as_bytes())
                .context("failed to write clipboard command stdin")?;
            let status = child.wait().context("failed to wait for clipboard command")?;
            if !status.success() {
                bail!("clipboard command {program} exited with {status}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::RuntimePaths;
    use std::{
        collections::VecDeque,
        ffi::OsString,
        fs,
        io::{Read, Write},
        os::unix::net::UnixListener,
    };
    use tempfile::{TempDir, tempdir as make_tempdir};

    fn tempdir() -> TempDir {
        make_tempdir().expect("tempdir")
    }
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
            state_path: "/tmp/admux-test/state.json".into(),
            aliases_path: "/tmp/admux-test/aliases.json".into(),
        };
        assert!(paths.socket_path.ends_with("socket"));
    }

    #[test]
    fn bare_unknown_subcommand_can_resolve_to_alias() {
        let dir = tempdir();
        let manifest = dir.path().join("admux.toml");
        fs::write(
            &manifest,
            r#"
version = 1

[workspace]
name = "demo"

[[windows]]
name = "shell"
root = { command = ["sh"] }
"#,
        )
        .expect("write manifest");
        let aliases = dir.path().join("aliases.json");
        fs::write(
            &aliases,
            format!(r#"{{"aliases":{{"demo":"{}"}}}}"#, manifest.display()),
        )
        .expect("write aliases");
        let paths = RuntimePaths {
            socket_path: dir.path().join("socket"),
            config_path: dir.path().join("config.toml"),
            state_path: dir.path().join("state.json"),
            aliases_path: aliases,
        };

        let cli = try_resolve_alias_invocation(
            &[OsString::from("admux"), OsString::from("demo")],
            &paths,
        )
        .expect("resolve alias")
        .expect("alias cli");

        assert_eq!(
            cli.command,
            ClientCommand::Up(crate::cli::UpArgs {
                detach: false,
                rebuild: false,
                path: Some(manifest.canonicalize().expect("canonical manifest")),
            })
        );
    }

    #[test]
    fn should_only_try_alias_for_single_unknown_subcommand() {
        let argv = vec![OsString::from("admux"), OsString::from("demo")];
        let error = AdmuxCli::try_parse_from(&argv).expect_err("unknown subcommand");
        assert!(should_try_alias(&argv, &error));

        let argv = vec![
            OsString::from("admux"),
            OsString::from("demo"),
            OsString::from("--detach"),
        ];
        let error = AdmuxCli::try_parse_from(&argv).expect_err("unknown subcommand");
        assert!(!should_try_alias(&argv, &error));
    }

    #[test]
    fn interactive_attachment_requires_both_terminal_streams() {
        assert!(interactive_terminal_available_for(true, true, true));
        assert!(!interactive_terminal_available_for(false, true, true));
        assert!(!interactive_terminal_available_for(true, false, true));
        assert!(!interactive_terminal_available_for(true, true, false));
    }

    #[test]
    fn prompt_completion_starts_at_the_first_candidate_and_cycles() {
        let mut prompt = PromptState {
            buffer: "s".into(),
            cursor: 1,
            completions: vec!["send-keys".into(), "select-pane".into(), "split-window".into()],
            selected: 0,
            history_index: None,
        };

        cycle_prompt_completion(&mut prompt);
        assert_eq!(prompt.buffer, "send-keys");
        assert_eq!(prompt.selected, 0);

        cycle_prompt_completion(&mut prompt);
        assert_eq!(prompt.buffer, "select-pane");
        assert_eq!(prompt.selected, 1);

        cycle_prompt_completion(&mut prompt);
        assert_eq!(prompt.buffer, "split-window");
        assert_eq!(prompt.selected, 2);

        cycle_prompt_completion(&mut prompt);
        assert_eq!(prompt.buffer, "send-keys");
        assert_eq!(prompt.selected, 0);
    }

    #[test]
    fn prompt_command_error_responses_are_failures() {
        let error = ensure_command_succeeded(CommandResponse::Error {
            message: "unknown pane".into(),
        })
        .expect_err("daemon error must not look successful");
        assert!(error.to_string().contains("unknown pane"));
    }

    #[test]
    fn chooser_command_errors_keep_the_chooser_open() {
        let mut status = None;
        assert!(!chooser_command_succeeded(
            CommandResponse::Error {
                message: "unknown window".into(),
            },
            &mut status,
        ));
        assert_eq!(status.as_deref(), Some("unknown window"));
    }

    #[test]
    fn prompt_overlay_commands_open_their_real_interactive_targets() {
        assert_eq!(
            prompt_overlay_command(&InteractiveCommand::ChooseTree),
            Some(PromptResult::OpenChooseTree)
        );
        assert_eq!(
            prompt_overlay_command(&InteractiveCommand::ChooseBuffer),
            Some(PromptResult::OpenChooseBuffer)
        );
        assert_eq!(
            prompt_overlay_command(&InteractiveCommand::DetachClient),
            Some(PromptResult::Detach)
        );
    }

    #[test]
    fn focus_and_window_actions_refresh_before_following_input() {
        assert!(action_changes_input_target(&InputAction::FocusPane(
            NavigationDirection::Right
        )));
        assert!(action_changes_input_target(&InputAction::NextWindow));
        assert!(action_changes_input_target(&InputAction::SplitPane(SplitAxis::Horizontal)));
        assert!(!action_changes_input_target(&InputAction::SendBytes(vec![b'x'])));
        assert!(!action_changes_input_target(&InputAction::ResizePane(
            NavigationDirection::Right,
            1,
        )));
    }

    #[test]
    fn ensure_protocol_surfaces_mismatch() {
        let dir = tempdir();
        let socket_path = dir.path().join("socket");
        let paths = RuntimePaths {
            socket_path: socket_path.clone(),
            config_path: dir.path().join("config.toml"),
            state_path: dir.path().join("state.json"),
            aliases_path: dir.path().join("aliases.json"),
        };
        let listener = UnixListener::bind(&socket_path).expect("bind");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut input = Vec::new();
            stream.read_to_end(&mut input).expect("read hello");
            let response = CommandResponse::Error {
                message: "protocol mismatch: client=5, server=4".into(),
            };
            let encoded = serde_json::to_vec(&response).expect("encode response");
            stream.write_all(&encoded).expect("write response");
        });

        let error = ensure_protocol(&paths).unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains("protocol mismatch"));
        assert!(rendered.contains("restart admuxd"));
    }

    #[test]
    fn daemon_autostart_is_limited_to_missing_or_refused_sockets() {
        assert!(should_autostart_daemon(&std::io::Error::from(
            std::io::ErrorKind::NotFound,
        )));
        assert!(should_autostart_daemon(&std::io::Error::from(
            std::io::ErrorKind::ConnectionRefused,
        )));
        assert!(!should_autostart_daemon(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        assert!(!should_autostart_daemon(&std::io::Error::from(
            std::io::ErrorKind::InvalidInput,
        )));
    }

    #[test]
    fn daemon_startup_log_lives_with_state_files() {
        let dir = tempdir();
        let paths = RuntimePaths {
            socket_path: dir.path().join("runtime/socket"),
            config_path: dir.path().join("config.toml"),
            state_path: dir.path().join("state.json"),
            aliases_path: dir.path().join("aliases.json"),
        };

        assert_eq!(
            daemon_start_log_path(&paths),
            dir.path().join("admuxd-startup.log")
        );
    }

    #[test]
    fn every_request_revalidates_the_daemon_protocol() {
        let dir = tempdir();
        let socket_path = dir.path().join("socket");
        let paths = RuntimePaths {
            socket_path: socket_path.clone(),
            config_path: dir.path().join("config.toml"),
            state_path: dir.path().join("state.json"),
            aliases_path: dir.path().join("aliases.json"),
        };
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).expect("bind");
        let server = std::thread::spawn(move || {
            for request_index in 0..2 {
                for hello in [true, false] {
                    let (mut stream, _) = listener.accept().expect("accept");
                    let mut input = Vec::new();
                    stream.read_to_end(&mut input).expect("read request");
                    let request: CommandRequest = serde_json::from_slice(&input).expect("decode request");
                    assert_eq!(hello, matches!(request, CommandRequest::Hello { .. }));
                    let response = if hello {
                        CommandResponse::HelloAck {
                            version: crate::ipc::CURRENT_PROTOCOL_VERSION,
                        }
                    } else {
                        assert_eq!(request, CommandRequest::ListSessions);
                        CommandResponse::SessionList { sessions: Vec::new() }
                    };
                    stream
                        .write_all(&serde_json::to_vec(&response).expect("encode response"))
                        .expect("write response");
                }
                assert!(request_index < 2);
            }
        });

        assert!(matches!(
            request_response(&paths, CommandRequest::ListSessions).expect("first request"),
            CommandResponse::SessionList { .. }
        ));
        assert!(matches!(
            request_response(&paths, CommandRequest::ListSessions).expect("second request"),
            CommandResponse::SessionList { .. }
        ));
        server.join().expect("server thread");
    }

    #[test]
    fn reload_interactive_config_refreshes_the_client_key_state() {
        let dir = tempdir();
        let socket_path = dir.path().join("socket");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[behavior]\nresize_step = 7").expect("write config");
        let paths = RuntimePaths {
            socket_path: socket_path.clone(),
            config_path,
            state_path: dir.path().join("state.json"),
            aliases_path: dir.path().join("aliases.json"),
        };
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).expect("bind");
        let server = std::thread::spawn(move || {
            for hello in [true, false] {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut input = Vec::new();
                stream.read_to_end(&mut input).expect("read request");
                let request: CommandRequest = serde_json::from_slice(&input).expect("decode request");
                assert_eq!(hello, matches!(request, CommandRequest::Hello { .. }));
                let response = if hello {
                    CommandResponse::HelloAck {
                        version: crate::ipc::CURRENT_PROTOCOL_VERSION,
                    }
                } else {
                    assert_eq!(request, CommandRequest::ReloadConfig);
                    CommandResponse::ConfigReloaded
                };
                stream
                    .write_all(&serde_json::to_vec(&response).expect("encode response"))
                    .expect("write response");
            }
        });
        let mut config = Config::default().resolve().expect("default config");
        let mut input_state = InputState::new(config.keys.clone(), config.behavior.resize_step);

        reload_interactive_config(&paths, &mut input_state, &mut config).expect("reload config");

        assert_eq!(config.behavior.resize_step, 7);
        let _ = input_state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            input_state.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)),
            InputAction::ResizePane(NavigationDirection::Right, 7)
        );
        server.join().expect("server thread");
    }

    #[test]
    fn normalize_new_args_defaults_to_current_directory() {
        let args = crate::cli::NewArgs {
            detach: true,
            name: Some("work".into()),
            cwd: None,
            command: Vec::new(),
        };

        let normalized = normalize_new_args(args).expect("normalize");
        assert_eq!(
            normalized.cwd,
            Some(std::env::current_dir().expect("current dir"))
        );
        assert!(normalized.command.is_empty());
    }

    #[test]
    fn normalize_new_args_treats_single_directory_argument_as_cwd() {
        let dir = tempdir();
        let args = crate::cli::NewArgs {
            detach: true,
            name: Some("work".into()),
            cwd: None,
            command: vec![dir.path().display().to_string()],
        };

        let normalized = normalize_new_args(args).expect("normalize");
        assert_eq!(normalized.cwd, Some(dir.path().to_path_buf()));
        assert!(normalized.command.is_empty());
    }

    #[test]
    fn normalize_new_args_makes_explicit_relative_cwd_client_relative() {
        let args = crate::cli::NewArgs {
            detach: true,
            name: Some("work".into()),
            cwd: Some(PathBuf::from("relative-project")),
            command: Vec::new(),
        };

        let normalized = normalize_new_args(args).expect("normalize");
        assert_eq!(
            normalized.cwd,
            Some(
                std::env::current_dir()
                    .expect("current directory")
                    .join("relative-project")
            )
        );
    }

    #[test]
    fn buffer_file_paths_are_resolved_at_the_client() {
        assert_eq!(
            resolve_client_path(PathBuf::from("buffers/output.txt")).expect("resolve relative"),
            std::env::current_dir()
                .expect("current directory")
                .join("buffers/output.txt")
        );
        let absolute = PathBuf::from("/tmp/admux-buffer.txt");
        assert_eq!(
            resolve_client_path(absolute.clone()).expect("preserve absolute"),
            absolute
        );
    }

    #[test]
    fn escape_prefixed_digit_coalesces_to_alt_digit() {
        let mut following = VecDeque::from([Event::Key(KeyEvent::new(
            KeyCode::Char('3'),
            KeyModifiers::NONE,
        ))]);
        let mut pending = None;

        let event = coalesce_escape_digit_event(
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            following.pop_front().expect("followup event"),
            &mut pending,
        );

        assert_eq!(
            event,
            Event::Key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::ALT))
        );
        assert!(pending.is_none());
        assert!(following.is_empty());
    }

    #[test]
    fn escape_preserves_non_digit_followup() {
        let mut following = VecDeque::from([Event::Key(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::NONE,
        ))]);
        let mut pending = None;

        let event = coalesce_escape_digit_event(
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            following.pop_front().expect("followup event"),
            &mut pending,
        );

        assert_eq!(event, Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(
            pending,
            Some(Event::Key(KeyEvent::new(
                KeyCode::Char('h'),
                KeyModifiers::NONE,
            )))
        );
    }

    #[test]
    fn resolve_window_target_uses_public_window_index() {
        let snapshot = RenderSnapshot {
            sessions: Vec::new(),
            windows: vec![
                WindowSummary {
                    id: 210,
                    index: 1,
                    name: "editor".into(),
                    active: false,
                    last_selected: false,
                },
                WindowSummary {
                    id: 211,
                    index: 2,
                    name: "shell".into(),
                    active: true,
                    last_selected: false,
                },
            ],
            panes: Vec::new(),
            dividers: Vec::new(),
            active_window_id: 2,
            active_pane_id: 1,
        };

        assert_eq!(resolve_window_target(&snapshot, "work", "1"), "work:1");
        assert_eq!(resolve_window_target(&snapshot, "work", "2"), "work:2");
    }

    #[test]
    fn choose_tree_search_moves_selection_forward() {
        let mut tree = ChooseTreeState {
            items: vec![
                ChooseItem::Session("work".into()),
                ChooseItem::Window {
                    session: "work".into(),
                    window_index: 1,
                },
                ChooseItem::Pane {
                    session: "work".into(),
                    window_index: 1,
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
            preview: None,
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
                    window_index: 1,
                },
                ChooseItem::Pane {
                    session: "work".into(),
                    window_index: 1,
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
            preview: None,
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
            preview: None,
        };

        collapse_all_choose_items(&mut tree);

        assert!(tree.expanded_sessions.is_empty());
        assert!(tree.expanded_windows.is_empty());
    }

    #[test]
    fn attached_response_updates_current_session_and_resets_size() {
        let response = CommandResponse::Attached {
            session: "logs".into(),
            preview: String::new(),
            formatted_preview: String::new(),
            formatted_cursor: String::new(),
            snapshot: None,
        };
        let mut current_session = String::from("work");
        let mut last_size = (40, 120);

        apply_attached_session(&response, &mut current_session, &mut last_size);

        assert_eq!(current_session, "logs");
        assert_eq!(last_size, (0, 0));
    }

    #[test]
    fn idle_snapshot_refresh_is_not_tied_to_input_poll_frequency() {
        assert!(!snapshot_refresh_due(Duration::from_millis(99)));
        assert!(snapshot_refresh_due(Duration::from_millis(100)));
    }

    #[test]
    fn interactive_command_errors_become_status_messages() {
        let mut status = None;
        assert!(!handle_interactive_response(
            CommandResponse::Error {
                message: "unknown pane".into(),
            },
            &mut status,
        ));
        assert_eq!(status.as_deref(), Some("unknown pane"));
    }

    #[test]
    fn failed_helper_input_falls_back_to_daemon_input() {
        let dir = tempdir();
        let socket_path = dir.path().join("socket");
        let paths = RuntimePaths {
            socket_path: socket_path.clone(),
            config_path: dir.path().join("config.toml"),
            state_path: dir.path().join("state.json"),
            aliases_path: dir.path().join("aliases.json"),
        };
        let listener = UnixListener::bind(&socket_path).expect("bind daemon socket");
        let server = std::thread::spawn(move || {
            for hello in [true, false] {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut input = Vec::new();
                stream.read_to_end(&mut input).expect("read request");
                let request: CommandRequest = serde_json::from_slice(&input).expect("decode request");
                let response = if hello {
                    assert!(matches!(request, CommandRequest::Hello { .. }));
                    CommandResponse::HelloAck {
                        version: crate::ipc::CURRENT_PROTOCOL_VERSION,
                    }
                } else {
                    assert_eq!(
                        request,
                        CommandRequest::SendBytes {
                            target: "work".into(),
                            bytes: b"x".to_vec(),
                        }
                    );
                    CommandResponse::KeysSent
                };
                stream
                    .write_all(&serde_json::to_vec(&response).expect("encode response"))
                    .expect("write response");
            }
        });
        let helper_socket = dir.path().join("helper");
        let helper_listener = UnixListener::bind(&helper_socket).expect("bind helper socket");
        let helper = std::thread::spawn(move || {
            let (mut stream, _) = helper_listener.accept().expect("accept direct send request");
            let mut input = Vec::new();
            stream.read_to_end(&mut input).expect("read direct send request");
            let request = serde_json::from_slice::<serde_json::Value>(&input)
                .expect("decode direct send request");
            assert!(request.get("SendBytes").is_some());
            assert!(request.get("Hello").is_none());
            // Dropping this response stream forces the direct-send failure that must fall back.
        });
        let mut snapshot = fallback_snapshot(String::new(), 80, 24);
        snapshot.panes[0].helper_socket = Some(helper_socket);

        send_input_bytes(&paths, &snapshot, "work", b"x", None)
            .expect("daemon fallback");
        helper.join().expect("helper thread");
        server.join().expect("server thread");
    }

    #[test]
    fn submitted_input_registers_its_client_before_forwarding() {
        let dir = tempdir();
        let socket_path = dir.path().join("socket");
        let paths = RuntimePaths {
            socket_path: socket_path.clone(),
            config_path: dir.path().join("config.toml"),
            state_path: dir.path().join("state.json"),
            aliases_path: dir.path().join("aliases.json"),
        };
        let listener = UnixListener::bind(&socket_path).expect("bind daemon socket");
        let server = std::thread::spawn(move || {
            for expected in [
                CommandRequest::Hello {
                    version: crate::ipc::CURRENT_PROTOCOL_VERSION,
                },
                CommandRequest::RegisterInput {
                    source: SwitchSource {
                        session: "work".into(),
                        window_id: 1,
                        pane_id: 1,
                    },
                    client_id: "client-1".into(),
                },
                CommandRequest::Hello {
                    version: crate::ipc::CURRENT_PROTOCOL_VERSION,
                },
                CommandRequest::SendBytes {
                    target: "work".into(),
                    bytes: b"\r".to_vec(),
                },
            ] {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut input = Vec::new();
                stream.read_to_end(&mut input).expect("read request");
                let request: CommandRequest = serde_json::from_slice(&input).expect("decode request");
                assert_eq!(request, expected);
                let response = match request {
                    CommandRequest::Hello { version } => CommandResponse::HelloAck { version },
                    CommandRequest::RegisterInput { .. } => CommandResponse::InputRegistered,
                    CommandRequest::SendBytes { .. } => CommandResponse::KeysSent,
                    other => panic!("unexpected request: {other:?}"),
                };
                stream
                    .write_all(&serde_json::to_vec(&response).expect("encode response"))
                    .expect("write response");
            }
        });

        let snapshot = fallback_snapshot(String::new(), 80, 24);
        send_input_bytes(&paths, &snapshot, "work", b"\r", Some("client-1"))
            .expect("submit input");
        server.join().expect("server thread");
    }

    #[test]
    fn rejected_prompt_session_switch_keeps_the_current_session() {
        let mut current_session = String::from("work");
        let mut last_size = (24, 80);
        assert!(apply_prompt_session_switch(
            &mut current_session,
            &mut last_size,
            CommandResponse::Error {
                message: "unknown session missing".into(),
            },
        )
        .is_err());
        assert_eq!(current_session, "work");
        assert_eq!(last_size, (24, 80));
    }

    #[test]
    fn prompt_session_switch_resets_viewport_for_the_new_session() {
        let mut current_session = String::from("work");
        let mut last_size = (24, 80);
        apply_prompt_session_switch(
            &mut current_session,
            &mut last_size,
            CommandResponse::Attached {
                session: "logs".into(),
                preview: String::new(),
                formatted_preview: String::new(),
                formatted_cursor: String::new(),
                snapshot: None,
            },
        )
        .expect("switch session");
        assert_eq!(current_session, "logs");
        assert_eq!(last_size, (0, 0));
    }

    #[test]
    fn prompt_parse_errors_stay_open_and_become_status_messages() {
        let dir = tempdir();
        let paths = RuntimePaths {
            socket_path: dir.path().join("socket"),
            config_path: dir.path().join("config.toml"),
            state_path: dir.path().join("state.json"),
            aliases_path: dir.path().join("aliases.json"),
        };
        let snapshot = RenderSnapshot {
            sessions: Vec::new(),
            windows: Vec::new(),
            panes: Vec::new(),
            dividers: Vec::new(),
            active_window_id: 0,
            active_pane_id: 0,
        };
        let mut prompt = PromptState {
            buffer: "send-keys \"unterminated".into(),
            cursor: 24,
            completions: Vec::new(),
            selected: 0,
            history_index: None,
        };
        let mut session = "work".into();
        let mut last_size = (24, 80);
        let mut config = Config::default().resolve().expect("default config");
        let mut input_state = InputState::new(config.keys.clone(), config.behavior.resize_step);
        let mut history = Vec::new();
        let mut status = None;

        let result = handle_prompt_key(
            &paths,
            &snapshot,
            &mut session,
            &mut last_size,
            &mut input_state,
            &mut config,
            &mut prompt,
            &mut history,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut status,
        )
        .expect("handle prompt key");

        assert_eq!(result, PromptResult::KeepOpen);
        assert!(status.is_some_and(|message| message.contains("unterminated")));
        assert!(history.is_empty());
    }

    #[test]
    fn prompt_cursor_moves_on_utf8_character_boundaries() {
        let text = "aé🙂";
        assert_eq!(previous_char_boundary(text, text.len()), Some(3));
        assert_eq!(previous_char_boundary(text, 3), Some(1));
        assert_eq!(previous_char_boundary(text, 1), Some(0));
        assert_eq!(next_char_boundary(text, 0), Some(1));
        assert_eq!(next_char_boundary(text, 1), Some(3));
        assert_eq!(next_char_boundary(text, 3), Some(text.len()));
    }

    #[test]
    fn pasting_into_the_prompt_inserts_at_the_utf8_cursor() {
        let mut prompt = PromptState {
            buffer: "say 🙂".into(),
            cursor: 4,
            completions: Vec::new(),
            selected: 0,
            history_index: Some(0),
        };

        insert_prompt_text(&mut prompt, "é");

        assert_eq!(prompt.buffer, "say é🙂");
        assert_eq!(prompt.cursor, 6);
        assert!(prompt.history_index.is_none());
    }

    #[test]
    fn resize_drag_request_reverses_direction_for_leftward_motion() {
        let resize = ResizeDrag {
            pane_id: 1,
            direction: NavigationDirection::Right,
            last_row: 0,
            last_col: 10,
            span: 80,
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
            span: 80,
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

    #[test]
    fn mouse_resize_amount_scales_with_terminal_span() {
        assert_eq!(mouse_resize_amount(1, 80), 13);
        assert_eq!(mouse_resize_amount(1, 200), 5);
        assert_eq!(mouse_resize_amount(100, 80), 100);
        assert_eq!(mouse_resize_amount(0, 80), 1);
    }

    #[test]
    fn mouse_capture_clamps_drag_and_release_to_the_pressed_pane() {
        let mut snapshot = fallback_snapshot(String::new(), 80, 24);
        snapshot.panes[0].rect = Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 8,
        };
        let capture = MouseCapture {
            pane_id: 1,
            button: MouseButton::Left,
        };
        let mouse = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 79,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };

        assert_eq!(
            captured_pane_mouse_position(&snapshot, capture, mouse),
            Some((0, 19))
        );
    }

    #[test]
    fn application_mouse_button_mappings_include_middle_and_right() {
        assert_eq!(
            helper_mouse_kinds(MouseButton::Middle),
            Some((
                HelperMouseEventKind::MiddleDown,
                HelperMouseEventKind::MiddleDrag,
                HelperMouseEventKind::MiddleUp,
            ))
        );
        assert_eq!(
            pane_mouse_kinds(MouseButton::Right),
            Some((
                PaneMouseKind::RightDown,
                PaneMouseKind::RightDrag,
                PaneMouseKind::RightUp,
            ))
        );
    }

    #[test]
    fn external_clipboard_command_receives_copied_text_on_stdin() {
        let dir = tempdir();
        let output = dir.path().join("clipboard.txt");
        let clipboard = ClipboardConfig {
            backend: ClipboardBackend::ExternalCommand,
            command: vec![
                "sh".into(),
                "-c".into(),
                format!("cat > {}", output.display()),
            ],
        };
        let mut terminal = Vec::new();

        copy_to_clipboard(&clipboard, &mut terminal, "copied text")
            .expect("copy through external command");

        assert_eq!(fs::read_to_string(output).expect("read clipboard output"), "copied text");
        assert!(terminal.is_empty(), "external backend must not emit OSC52");
    }
}
