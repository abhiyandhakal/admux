use crate::{
    ipc::ScrollDirection,
    pane::{PaneId, WindowId},
};
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HISTORY_LIMIT: usize = 2 * 1024 * 1024;
const IPC_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_IPC_MESSAGE_BYTES: u64 = 1024 * 1024;
const HELPER_PROTOCOL_VERSION: u16 = 1;
static HELPER_NAME_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TerminalState {
    parser: vt100::Parser,
    history: Vec<u8>,
    scrollback_lines: usize,
}

struct HelperState {
    terminal: Arc<Mutex<TerminalState>>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayBoundaryState {
    Ground,
    Escape,
    Csi,
    String,
    StringEscape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyState {
    Detached,
    Attached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSnapshot {
    pub preview: String,
    pub formatted_preview: String,
    pub formatted_cursor: String,
    pub rows_plain: Vec<String>,
    pub rows_formatted: Vec<String>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub screen_rows: u16,
    pub screen_cols: u16,
    pub mouse_reporting: bool,
    pub application_cursor: bool,
    pub alive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneProcess {
    socket_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanePersistentSnapshot {
    pub rows: u16,
    pub cols: u16,
    pub vt: String,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRestoreSeed {
    pub rows: u16,
    pub cols: u16,
    pub vt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneHelperArgs {
    pub socket: PathBuf,
    pub cwd: Option<PathBuf>,
    pub session_name: Option<String>,
    pub window_id: Option<u64>,
    pub pane_id: Option<u64>,
    pub default_shell: Option<String>,
    pub scrollback_lines: usize,
    pub command: Vec<String>,
    #[serde(default)]
    pub restore_seed: Option<PaneRestoreSeed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum PaneRequest {
    Hello { version: u16 },
    Snapshot {
        width: u16,
        height: u16,
    },
    ScreenSize,
    SelectionText {
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    },
    Resize {
        rows: u16,
        cols: u16,
    },
    MouseScroll {
        direction: ScrollDirection,
        row: u16,
        col: u16,
    },
    MouseEvent {
        kind: HelperMouseEventKind,
        row: u16,
        col: u16,
    },
    Scrollback {
        lines: i16,
    },
    SendKeys {
        keys: Vec<String>,
    },
    PersistentSnapshot {
        lines: usize,
    },
    Shutdown,
    IsAlive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum PaneResponse {
    HelloAck { version: u16 },
    Snapshot(PaneSnapshotWire),
    ScreenSize { rows: u16, cols: u16 },
    SelectionText { text: String },
    PersistentSnapshot(PanePersistentSnapshotWire),
    IsAlive { alive: bool },
    Ok,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PanePersistentSnapshotWire {
    rows: u16,
    cols: u16,
    vt_b64: String,
    command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PaneSnapshotWire {
    preview: String,
    formatted_preview: String,
    formatted_cursor: String,
    rows_plain: Vec<String>,
    rows_formatted: Vec<String>,
    cursor_row: u16,
    cursor_col: u16,
    screen_rows: u16,
    screen_cols: u16,
    mouse_reporting: bool,
    application_cursor: bool,
    alive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelperMouseEventKind {
    LeftDown,
    LeftDrag,
    LeftUp,
    MiddleDown,
    MiddleDrag,
    MiddleUp,
    RightDown,
    RightDrag,
    RightUp,
}

impl From<PaneSnapshotWire> for PaneSnapshot {
    fn from(value: PaneSnapshotWire) -> Self {
        Self {
            preview: value.preview,
            formatted_preview: value.formatted_preview,
            formatted_cursor: value.formatted_cursor,
            rows_plain: value.rows_plain,
            rows_formatted: value.rows_formatted,
            cursor_row: value.cursor_row,
            cursor_col: value.cursor_col,
            screen_rows: value.screen_rows,
            screen_cols: value.screen_cols,
            mouse_reporting: value.mouse_reporting,
            application_cursor: value.application_cursor,
            alive: value.alive,
        }
    }
}

impl TryFrom<PanePersistentSnapshotWire> for PanePersistentSnapshot {
    type Error = anyhow::Error;

    fn try_from(value: PanePersistentSnapshotWire) -> Result<Self> {
        let vt_bytes = STANDARD
            .decode(value.vt_b64)
            .context("pane snapshot wire contains invalid base64")?;
        let vt = String::from_utf8(vt_bytes).context("pane snapshot wire contains invalid UTF-8")?;
        Ok(Self {
            rows: value.rows,
            cols: value.cols,
            vt,
            command: value.command,
        })
    }
}

impl From<&PanePersistentSnapshot> for PaneRestoreSeed {
    fn from(value: &PanePersistentSnapshot) -> Self {
        Self {
            rows: value.rows,
            cols: value.cols,
            vt: value.vt.clone(),
        }
    }
}

impl PaneProcess {
    pub fn spawn(
        command: &[String],
        cwd: Option<&Path>,
        admux_context: Option<(&str, WindowId, PaneId)>,
        default_shell: Option<&str>,
        scrollback_lines: usize,
        helper_dir: &Path,
        restore_seed: Option<PaneRestoreSeed>,
    ) -> Result<Self> {
        ensure_private_helper_directory(helper_dir)?;
        let socket_path = helper_dir.join(unique_helper_name());
        let helper_bin = resolve_helper_binary()?;

        let args = PaneHelperArgs {
            socket: socket_path.clone(),
            cwd: cwd.map(Path::to_path_buf),
            session_name: admux_context.map(|(session, _, _)| session.to_string()),
            window_id: admux_context.map(|(_, window_id, _)| window_id.0),
            pane_id: admux_context.map(|(_, _, pane_id)| pane_id.0),
            default_shell: default_shell.map(ToOwned::to_owned),
            scrollback_lines,
            command: command.to_vec(),
            restore_seed,
        };
        let payload = serde_json::to_string(&args).context("failed to encode helper args")?;

        let mut child = Command::new(helper_bin)
            .arg(payload)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Keep startup failures visible for manually supervised daemons;
            // otherwise a helper failure surfaces only as a socket timeout.
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to spawn admux-pane helper")?;

        if let Err(error) = wait_for_socket(&socket_path) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&socket_path);
            return Err(error);
        }
        Ok(Self { socket_path })
    }

    pub fn connect(socket_path: PathBuf) -> Result<Self> {
        if !socket_path.exists() {
            bail!("missing pane helper socket {}", socket_path.display());
        }
        let process = Self { socket_path };
        process.ensure_protocol()?;
        Ok(process)
    }

    pub fn connect_live(socket_path: PathBuf) -> Result<Self> {
        let process = Self::connect(socket_path)?;
        if !process.is_alive() {
            bail!(
                "pane helper at {} is not alive",
                process.socket_path.display()
            );
        }
        Ok(process)
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn render(&self, width: u16, height: u16) -> Result<PaneSnapshot> {
        match self.request(PaneRequest::Snapshot { width, height })? {
            PaneResponse::Snapshot(snapshot) => Ok(snapshot.into()),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected pane snapshot response: {other:?}")),
        }
    }

    pub fn preview(&self) -> String {
        self.render_with_current_size()
            .map(|snapshot| snapshot.preview)
            .unwrap_or_default()
    }

    pub fn formatted_preview(&self) -> String {
        self.render_with_current_size()
            .map(|snapshot| snapshot.formatted_preview)
            .unwrap_or_default()
    }

    pub fn formatted_cursor(&self) -> String {
        self.render_with_current_size()
            .map(|snapshot| snapshot.formatted_cursor)
            .unwrap_or_default()
    }

    pub fn visible_rows(&self, width: u16, height: u16) -> Vec<String> {
        self.render(width, height)
            .map(|snapshot| snapshot.rows_plain)
            .unwrap_or_default()
    }

    pub fn visible_rows_formatted(&self, width: u16, height: u16) -> Vec<String> {
        self.render(width, height)
            .map(|snapshot| snapshot.rows_formatted)
            .unwrap_or_default()
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        self.render_with_current_size()
            .map(|snapshot| (snapshot.cursor_row, snapshot.cursor_col))
            .unwrap_or((0, 0))
    }

    pub fn screen_size(&self) -> (u16, u16) {
        match self.request(PaneRequest::ScreenSize) {
            Ok(PaneResponse::ScreenSize { rows, cols }) => (rows, cols),
            _ => (24, 80),
        }
    }

    pub fn selection_text(
        &self,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    ) -> Result<String> {
        match self.request(PaneRequest::SelectionText {
            start_row,
            start_col,
            end_row,
            end_col,
        })? {
            PaneResponse::SelectionText { text } => Ok(text),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected selection text response: {other:?}")),
        }
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        match self.request(PaneRequest::Resize { rows, cols })? {
            PaneResponse::Ok => Ok(()),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected resize response: {other:?}")),
        }
    }

    pub fn handle_mouse_scroll(
        &self,
        direction: ScrollDirection,
        row: u16,
        col: u16,
    ) -> Result<()> {
        match self.request(PaneRequest::MouseScroll {
            direction,
            row,
            col,
        })? {
            PaneResponse::Ok => Ok(()),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected mouse scroll response: {other:?}")),
        }
    }

    pub fn handle_mouse_event(&self, kind: HelperMouseEventKind, row: u16, col: u16) -> Result<()> {
        match self.request(PaneRequest::MouseEvent { kind, row, col })? {
            PaneResponse::Ok => Ok(()),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected mouse event response: {other:?}")),
        }
    }

    pub fn scroll_scrollback_by(&self, lines: i16) -> Result<()> {
        match self.request(PaneRequest::Scrollback { lines })? {
            PaneResponse::Ok => Ok(()),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected scrollback response: {other:?}")),
        }
    }

    pub fn send_keys(&self, keys: &[String]) -> Result<()> {
        match self.request(PaneRequest::SendKeys {
            keys: keys.to_vec(),
        })? {
            PaneResponse::Ok => Ok(()),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected send keys response: {other:?}")),
        }
    }

    pub fn kill(&self) -> Result<()> {
        match self.request(PaneRequest::Shutdown)? {
            PaneResponse::Ok => wait_for_socket_removal(&self.socket_path),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected shutdown response: {other:?}")),
        }
    }

    pub fn persistent_snapshot(&self, lines: usize) -> Result<PanePersistentSnapshot> {
        match self.request(PaneRequest::PersistentSnapshot { lines })? {
            PaneResponse::PersistentSnapshot(snapshot) => snapshot.try_into(),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!(
                "unexpected persistent snapshot response: {other:?}"
            )),
        }
    }

    pub fn is_alive(&self) -> bool {
        matches!(
            self.request(PaneRequest::IsAlive),
            Ok(PaneResponse::IsAlive { alive: true })
        )
    }

    fn render_with_current_size(&self) -> Result<PaneSnapshot> {
        let (rows, cols) = self.screen_size();
        self.render(cols.max(1), rows.max(1))
    }

    fn request(&self, request: PaneRequest) -> Result<PaneResponse> {
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "failed to connect pane helper {}",
                self.socket_path.display()
            )
        })?;
        configure_ipc_stream(&stream)?;
        let payload = serde_json::to_vec(&request).context("failed to encode pane request")?;
        stream
            .write_all(&payload)
            .context("failed to write pane request")?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .context("failed to finish pane request")?;
        let response = read_limited(&mut stream, "pane response")?;
        serde_json::from_slice(&response).context("failed to decode pane response")
    }

    fn ensure_protocol(&self) -> Result<()> {
        match self.request(PaneRequest::Hello {
            version: HELPER_PROTOCOL_VERSION,
        })? {
            PaneResponse::HelloAck { version } if version == HELPER_PROTOCOL_VERSION => Ok(()),
            PaneResponse::HelloAck { version } => bail!(
                "pane helper protocol mismatch: daemon={}, helper={version}",
                HELPER_PROTOCOL_VERSION
            ),
            PaneResponse::Error { message } => bail!("pane helper protocol rejected handshake: {message}"),
            other => bail!("pane helper returned invalid handshake response: {other:?}"),
        }
    }
}

pub fn run_helper(args: PaneHelperArgs) -> Result<()> {
    if let Some(parent) = args.socket.parent() {
        ensure_private_helper_directory(parent)?;
    }
    if args.socket.exists() {
        fs::remove_file(&args.socket).with_context(|| {
            format!(
                "failed to remove stale pane helper socket {}",
                args.socket.display()
            )
        })?;
    }

    let listener = UnixListener::bind(&args.socket).with_context(|| {
        format!(
            "failed to bind pane helper socket {}",
            args.socket.display()
        )
    })?;
    fs::set_permissions(&args.socket, fs::Permissions::from_mode(0o600)).with_context(|| {
        format!(
            "failed to restrict pane helper socket permissions {}",
            args.socket.display()
        )
    })?;
    let state = match start_helper_state(&args) {
        Ok(state) => Arc::new(state),
        Err(error) => {
            let _ = fs::remove_file(&args.socket);
            return Err(error);
        }
    };

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        if let Err(error) = configure_ipc_stream(&stream) {
            eprintln!("admux-pane: failed to configure client stream: {error:#}");
            continue;
        }
        let request = match read_helper_request(&mut stream) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("admux-pane: rejected client request: {error:#}");
                continue;
            }
        };
        let shutdown_requested = matches!(request, PaneRequest::Shutdown);
        let response = handle_helper_request(&state, request);
        if let Err(error) = write_helper_response(&mut stream, &response) {
            eprintln!("admux-pane: failed to write client response: {error:#}");
            continue;
        }
        if shutdown_requested && matches!(response, PaneResponse::Ok) {
            break;
        }
    }

    let _ = fs::remove_file(&args.socket);
    Ok(())
}

fn ensure_private_helper_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create pane helper directory {}", path.display()))?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect pane helper directory {}", path.display()))?;
    if !metadata.is_dir() {
        bail!("pane helper path {} is not a directory", path.display());
    }
    if metadata.uid() != unsafe { libc::geteuid() } {
        bail!("refusing pane helper directory not owned by the effective user: {}", path.display());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).with_context(|| {
        format!("failed to restrict pane helper directory permissions {}", path.display())
    })?;
    Ok(())
}

fn start_helper_state(args: &PaneHelperArgs) -> Result<HelperState> {
    let initial_rows = args
        .restore_seed
        .as_ref()
        .map(|seed| seed.rows.max(1))
        .unwrap_or(24);
    let initial_cols = args
        .restore_seed
        .as_ref()
        .map(|seed| seed.cols.max(1))
        .unwrap_or(80);
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: initial_rows,
            cols: initial_cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to create PTY pair")?;

    let context = match (&args.session_name, args.window_id, args.pane_id) {
        (Some(session), Some(window_id), Some(pane_id)) => {
            Some((session.as_str(), WindowId(window_id), PaneId(pane_id)))
        }
        _ => None,
    };
    let mut builder = build_command(&args.command, context, args.default_shell.as_deref());
    if let Some(cwd) = args.cwd.as_deref() {
        builder.cwd(cwd);
    }
    builder.env("TERM", "screen-256color");

    let mut parser = vt100::Parser::new(initial_rows, initial_cols, args.scrollback_lines);
    let mut history = Vec::new();
    if let Some(seed) = &args.restore_seed {
        history.extend_from_slice(seed.vt.as_bytes());
        parser.process(seed.vt.as_bytes());
    }

    let child = pair
        .slave
        .spawn_command(builder)
        .context("failed to spawn pane command")?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let writer = pair
        .master
        .take_writer()
        .context("failed to acquire PTY writer")?;
    let terminal = Arc::new(Mutex::new(TerminalState {
        parser,
        history,
        scrollback_lines: args.scrollback_lines,
    }));
    let terminal_clone = Arc::clone(&terminal);

    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(size) => {
                    if let Ok(mut terminal) = terminal_clone.lock() {
                        terminal.history.extend_from_slice(&buf[..size]);
                        truncate_history_at_safe_boundary(&mut terminal.history, HISTORY_LIMIT);
                        terminal.parser.process(&buf[..size]);
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(HelperState {
        terminal,
        writer: Mutex::new(writer),
        master: Mutex::new(pair.master),
        child: Mutex::new(child),
    })
}

fn truncate_history_at_safe_boundary(history: &mut Vec<u8>, limit: usize) {
    if history.len() <= limit {
        return;
    }

    let minimum_drop = history.len() - limit;
    let mut state = ReplayBoundaryState::Ground;
    for (index, byte) in history.iter().copied().enumerate() {
        state = match state {
            ReplayBoundaryState::Ground if byte == 0x1b => ReplayBoundaryState::Escape,
            ReplayBoundaryState::Ground => ReplayBoundaryState::Ground,
            ReplayBoundaryState::Escape if byte == b'[' => ReplayBoundaryState::Csi,
            ReplayBoundaryState::Escape if matches!(byte, b']' | b'P' | b'^' | b'_') => {
                ReplayBoundaryState::String
            }
            ReplayBoundaryState::Escape => ReplayBoundaryState::Ground,
            ReplayBoundaryState::Csi if (0x40..=0x7e).contains(&byte) => {
                ReplayBoundaryState::Ground
            }
            ReplayBoundaryState::Csi => ReplayBoundaryState::Csi,
            ReplayBoundaryState::String if byte == 0x07 => ReplayBoundaryState::Ground,
            ReplayBoundaryState::String if byte == 0x1b => ReplayBoundaryState::StringEscape,
            ReplayBoundaryState::String => ReplayBoundaryState::String,
            ReplayBoundaryState::StringEscape if byte == b'\\' => ReplayBoundaryState::Ground,
            ReplayBoundaryState::StringEscape if byte == 0x1b => ReplayBoundaryState::StringEscape,
            ReplayBoundaryState::StringEscape => ReplayBoundaryState::String,
        };

        let next = index + 1;
        let starts_utf8_boundary = history
            .get(next)
            .is_none_or(|next_byte| !(0x80..=0xbf).contains(next_byte));
        if next >= minimum_drop
            && state == ReplayBoundaryState::Ground
            && starts_utf8_boundary
        {
            history.drain(..next);
            return;
        }
    }

    // The retained history never reached a replay-safe boundary (for example,
    // a single unterminated OSC payload). Discard it rather than replaying a
    // partial control sequence into a fresh parser.
    history.clear();
}

fn handle_helper_request(state: &Arc<HelperState>, request: PaneRequest) -> PaneResponse {
    match request {
        PaneRequest::Hello { version } => {
            if version == HELPER_PROTOCOL_VERSION {
                PaneResponse::HelloAck { version }
            } else {
                PaneResponse::Error {
                    message: format!(
                        "pane helper protocol mismatch: daemon={version}, helper={HELPER_PROTOCOL_VERSION}"
                    ),
                }
            }
        }
        PaneRequest::Snapshot { width, height } => match helper_snapshot(state, width, height) {
            Ok(snapshot) => PaneResponse::Snapshot(snapshot),
            Err(error) => PaneResponse::Error {
                message: error.to_string(),
            },
        },
        PaneRequest::ScreenSize => {
            let (rows, cols) = helper_screen_size(state);
            PaneResponse::ScreenSize { rows, cols }
        }
        PaneRequest::SelectionText {
            start_row,
            start_col,
            end_row,
            end_col,
        } => PaneResponse::SelectionText {
            text: state
                .terminal
                .lock()
                .expect("pane helper terminal lock poisoned")
                .parser
                .screen()
                .contents_between(start_row, start_col, end_row, end_col),
        },
        PaneRequest::Resize { rows, cols } => match helper_resize(state, rows, cols) {
            Ok(()) => PaneResponse::Ok,
            Err(error) => PaneResponse::Error {
                message: error.to_string(),
            },
        },
        PaneRequest::MouseScroll {
            direction,
            row,
            col,
        } => match helper_mouse_scroll(state, direction, row, col) {
            Ok(()) => PaneResponse::Ok,
            Err(error) => PaneResponse::Error {
                message: error.to_string(),
            },
        },
        PaneRequest::MouseEvent { kind, row, col } => {
            match helper_mouse_event(state, kind, row, col) {
                Ok(()) => PaneResponse::Ok,
                Err(error) => PaneResponse::Error {
                    message: error.to_string(),
                },
            }
        }
        PaneRequest::Scrollback { lines } => {
            helper_scroll_scrollback(state, lines);
            PaneResponse::Ok
        }
        PaneRequest::SendKeys { keys } => match helper_send_keys(state, &keys) {
            Ok(()) => PaneResponse::Ok,
            Err(error) => PaneResponse::Error {
                message: error.to_string(),
            },
        },
        PaneRequest::PersistentSnapshot { lines } => {
            match helper_persistent_snapshot(state, lines) {
                Ok(snapshot) => PaneResponse::PersistentSnapshot(snapshot),
                Err(error) => PaneResponse::Error {
                    message: error.to_string(),
                },
            }
        }
        PaneRequest::Shutdown => {
            let mut child = state
                .child
                .lock()
                .expect("pane helper child lock poisoned");
            let shutdown = match child.try_wait() {
                Ok(Some(_)) => Ok(()),
                Ok(None) => match child.kill() {
                    Ok(()) => child.wait().map(|_| ()),
                    Err(kill_error) => match child.try_wait() {
                        Ok(Some(_)) => Ok(()),
                        _ => Err(kill_error),
                    },
                },
                Err(error) => Err(error),
            };
            match shutdown {
                Ok(()) => PaneResponse::Ok,
                Err(error) => PaneResponse::Error {
                    message: format!("failed to kill pane child: {error}"),
                },
            }
        }
        PaneRequest::IsAlive => PaneResponse::IsAlive {
            alive: state
                .child
                .lock()
                .expect("pane helper child lock poisoned")
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(false),
        },
    }
}

fn helper_snapshot(state: &Arc<HelperState>, width: u16, height: u16) -> Result<PaneSnapshotWire> {
    let terminal = state
        .terminal
        .lock()
        .expect("pane helper terminal lock poisoned");
    let screen = terminal.parser.screen();
    let (screen_rows, screen_cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let mouse_reporting = screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None;
    let application_cursor = screen.application_cursor();
    let alive = state
        .child
        .lock()
        .expect("pane helper child lock poisoned")
        .try_wait()
        .map(|status| status.is_none())
        .unwrap_or(false);
    Ok(PaneSnapshotWire {
        preview: screen.contents(),
        formatted_preview: String::from_utf8_lossy(&screen.contents_formatted()).into_owned(),
        formatted_cursor: String::from_utf8_lossy(&screen.cursor_state_formatted()).into_owned(),
        rows_plain: screen.rows(0, width).take(height as usize).collect(),
        rows_formatted: screen
            .rows_formatted(0, width)
            .take(height as usize)
            .map(|row| String::from_utf8_lossy(&row).into_owned())
            .collect(),
        cursor_row,
        cursor_col,
        screen_rows,
        screen_cols,
        mouse_reporting,
        application_cursor,
        alive,
    })
}

fn helper_screen_size(state: &Arc<HelperState>) -> (u16, u16) {
    state
        .terminal
        .lock()
        .expect("pane helper terminal lock poisoned")
        .parser
        .screen()
        .size()
}

fn helper_resize(state: &Arc<HelperState>, rows: u16, cols: u16) -> Result<()> {
    let (current_rows, current_cols) = helper_screen_size(state);
    state
        .master
        .lock()
        .expect("pane helper master lock poisoned")
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to resize PTY")?;
    let mut terminal = state
        .terminal
        .lock()
        .expect("pane helper terminal lock poisoned");
    if rows > current_rows || cols > current_cols {
        let history = terminal.history.clone();
        let mut parser = vt100::Parser::new(rows, cols, terminal.scrollback_lines);
        parser.process(&history);
        terminal.parser = parser;
    } else {
        terminal.parser.screen_mut().set_size(rows, cols);
    }
    Ok(())
}

fn helper_mouse_scroll(
    state: &Arc<HelperState>,
    direction: ScrollDirection,
    row: u16,
    col: u16,
) -> Result<()> {
    let (mouse_mode, mouse_encoding) = helper_mouse_protocol(state);

    if mouse_mode == vt100::MouseProtocolMode::None {
        let mut terminal = state
            .terminal
            .lock()
            .expect("pane helper terminal lock poisoned");
        let current = terminal.parser.screen().scrollback();
        let next = match direction {
            ScrollDirection::Up => current.saturating_add(3),
            ScrollDirection::Down => current.saturating_sub(3),
        };
        terminal.parser.screen_mut().set_scrollback(next);
        return Ok(());
    }

    let code = match direction {
        ScrollDirection::Up => 64,
        ScrollDirection::Down => 65,
    };
    let report = encode_mouse_report(mouse_encoding, code, false, false, row, col)?;
    let mut writer = state
        .writer
        .lock()
        .expect("pane helper writer lock poisoned");
    writer
        .write_all(&report)
        .context("failed to write mouse scroll bytes")?;
    writer.flush().context("failed to flush PTY writer")?;
    Ok(())
}

fn helper_mouse_event(
    state: &Arc<HelperState>,
    kind: HelperMouseEventKind,
    row: u16,
    col: u16,
) -> Result<()> {
    let (mouse_mode, mouse_encoding) = helper_mouse_protocol(state);
    if mouse_mode == vt100::MouseProtocolMode::None {
        return Ok(());
    }

    let (button, drag, release) = match kind {
        HelperMouseEventKind::LeftDown => (0, false, false),
        HelperMouseEventKind::LeftDrag => (0, true, false),
        HelperMouseEventKind::LeftUp => (0, false, true),
        HelperMouseEventKind::MiddleDown => (1, false, false),
        HelperMouseEventKind::MiddleDrag => (1, true, false),
        HelperMouseEventKind::MiddleUp => (1, false, true),
        HelperMouseEventKind::RightDown => (2, false, false),
        HelperMouseEventKind::RightDrag => (2, true, false),
        HelperMouseEventKind::RightUp => (2, false, true),
    };

    if !mouse_event_is_requested(mouse_mode, drag, release) {
        return Ok(());
    }

    let report = encode_mouse_report(mouse_encoding, button, drag, release, row, col)?;
    let mut writer = state
        .writer
        .lock()
        .expect("pane helper writer lock poisoned");
    writer
        .write_all(&report)
        .context("failed to write mouse event bytes")?;
    writer.flush().context("failed to flush PTY writer")?;
    Ok(())
}

fn helper_mouse_protocol(
    state: &Arc<HelperState>,
) -> (vt100::MouseProtocolMode, vt100::MouseProtocolEncoding) {
    let terminal = state
        .terminal
        .lock()
        .expect("pane helper terminal lock poisoned");
    let screen = terminal.parser.screen();
    (
        screen.mouse_protocol_mode(),
        screen.mouse_protocol_encoding(),
    )
}

fn mouse_event_is_requested(mode: vt100::MouseProtocolMode, drag: bool, release: bool) -> bool {
    match mode {
        vt100::MouseProtocolMode::None => false,
        vt100::MouseProtocolMode::Press => !drag && !release,
        vt100::MouseProtocolMode::PressRelease => !drag,
        vt100::MouseProtocolMode::ButtonMotion | vt100::MouseProtocolMode::AnyMotion => true,
    }
}

fn encode_mouse_report(
    encoding: vt100::MouseProtocolEncoding,
    button: u8,
    drag: bool,
    release: bool,
    row: u16,
    col: u16,
) -> Result<Vec<u8>> {
    let code = if release && encoding != vt100::MouseProtocolEncoding::Sgr {
        3
    } else {
        button + u8::from(drag) * 32
    };
    let x = u32::from(col) + 33;
    let y = u32::from(row) + 33;

    match encoding {
        vt100::MouseProtocolEncoding::Sgr => Ok(format!(
            "\x1b[<{};{};{}{}",
            code,
            u32::from(col) + 1,
            u32::from(row) + 1,
            if release { 'm' } else { 'M' }
        )
        .into_bytes()),
        vt100::MouseProtocolEncoding::Default => {
            let x =
                u8::try_from(x).context("mouse column exceeds the default xterm encoding limit")?;
            let y =
                u8::try_from(y).context("mouse row exceeds the default xterm encoding limit")?;
            Ok(vec![b'\x1b', b'[', b'M', code + 32, x, y])
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            let x = char::from_u32(x).context("invalid UTF-8 mouse column")?;
            let y = char::from_u32(y).context("invalid UTF-8 mouse row")?;
            let mut bytes = vec![b'\x1b', b'[', b'M', code + 32];
            bytes.extend(x.to_string().bytes());
            bytes.extend(y.to_string().bytes());
            Ok(bytes)
        }
    }
}

fn helper_scroll_scrollback(state: &Arc<HelperState>, lines: i16) {
    let mut terminal = state
        .terminal
        .lock()
        .expect("pane helper terminal lock poisoned");
    let current = terminal.parser.screen().scrollback();
    let next = if lines.is_negative() {
        current.saturating_add(lines.unsigned_abs() as usize)
    } else {
        current.saturating_sub(lines as usize)
    };
    terminal.parser.screen_mut().set_scrollback(next);
}

fn helper_send_keys(state: &Arc<HelperState>, keys: &[String]) -> Result<()> {
    let mut writer = state
        .writer
        .lock()
        .expect("pane helper writer lock poisoned");
    for key in keys {
        writer
            .write_all(&encode_send_key(key))
            .context("failed to write key bytes")?;
    }
    writer.flush().context("failed to flush PTY writer")?;
    Ok(())
}

/// Decode the small, tmux-compatible key vocabulary accepted by `send-keys`.
/// Unrecognised values are deliberately written verbatim so quoted command text
/// such as `send-keys "echo hello" Enter` keeps working.
fn encode_send_key(key: &str) -> Vec<u8> {
    if let Some(key) = key.strip_prefix("C-").or_else(|| key.strip_prefix("Ctrl-")) {
        if key.len() == 1 {
            let byte = key.as_bytes()[0];
            return match byte {
                b'a'..=b'z' | b'A'..=b'Z' => vec![byte.to_ascii_lowercase() - b'a' + 1],
                b'@' | b' ' => vec![0],
                b'['..=b'_' => vec![byte - b'@'],
                b'?' => vec![0x7f],
                _ => key.as_bytes().to_vec(),
            };
        }
    }

    if let Some(key) = key.strip_prefix("M-").or_else(|| key.strip_prefix("Alt-")) {
        let mut sequence = vec![0x1b];
        sequence.extend_from_slice(&encode_send_key(key));
        return sequence;
    }

    match key {
        "Enter" | "Return" => b"\r".to_vec(),
        "Tab" => b"\t".to_vec(),
        "BTab" => b"\x1b[Z".to_vec(),
        "Escape" | "Esc" => b"\x1b".to_vec(),
        "Space" => b" ".to_vec(),
        "Backspace" | "BSpace" => vec![0x7f],
        "Left" => b"\x1b[D".to_vec(),
        "Right" => b"\x1b[C".to_vec(),
        "Up" => b"\x1b[A".to_vec(),
        "Down" => b"\x1b[B".to_vec(),
        "Home" => b"\x1b[H".to_vec(),
        "End" => b"\x1b[F".to_vec(),
        "Insert" => b"\x1b[2~".to_vec(),
        "Delete" | "DC" => b"\x1b[3~".to_vec(),
        "PageUp" | "PPage" => b"\x1b[5~".to_vec(),
        "PageDown" | "NPage" => b"\x1b[6~".to_vec(),
        "F1" => b"\x1bOP".to_vec(),
        "F2" => b"\x1bOQ".to_vec(),
        "F3" => b"\x1bOR".to_vec(),
        "F4" => b"\x1bOS".to_vec(),
        "F5" => b"\x1b[15~".to_vec(),
        "F6" => b"\x1b[17~".to_vec(),
        "F7" => b"\x1b[18~".to_vec(),
        "F8" => b"\x1b[19~".to_vec(),
        "F9" => b"\x1b[20~".to_vec(),
        "F10" => b"\x1b[21~".to_vec(),
        "F11" => b"\x1b[23~".to_vec(),
        "F12" => b"\x1b[24~".to_vec(),
        _ => key.as_bytes().to_vec(),
    }
}

fn helper_persistent_snapshot(
    state: &Arc<HelperState>,
    lines: usize,
) -> Result<PanePersistentSnapshotWire> {
    let terminal = state
        .terminal
        .lock()
        .expect("pane helper terminal lock poisoned");
    let screen = terminal.parser.screen();
    let (rows, cols) = screen.size();
    let mut formatted_rows: Vec<String> = screen
        .rows_formatted(0, cols.max(1))
        .map(|row| String::from_utf8_lossy(&row).into_owned())
        .collect();
    let keep = lines.max(rows as usize).min(formatted_rows.len());
    if formatted_rows.len() > keep {
        let drop = formatted_rows.len() - keep;
        formatted_rows.drain(..drop);
    }
    let mut vt = String::from("\x1b[2J\x1b[H");
    if !formatted_rows.is_empty() {
        vt.push_str(&formatted_rows.join("\r\n"));
    }
    vt.push_str("\x1b[0m");
    vt.push_str(&String::from_utf8_lossy(&screen.cursor_state_formatted()));
    Ok(PanePersistentSnapshotWire {
        rows,
        cols,
        vt_b64: STANDARD.encode(vt.as_bytes()),
        command: helper_foreground_command(state).unwrap_or_default(),
    })
}

fn helper_foreground_command(state: &Arc<HelperState>) -> Option<Vec<String>> {
    let pid = state
        .master
        .lock()
        .expect("pane helper master lock poisoned")
        .process_group_leader()?;
    foreground_command_for_pid(pid)
}

fn foreground_command_for_pid(pid: i32) -> Option<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        let path = PathBuf::from(format!("/proc/{pid}/cmdline"));
        if let Ok(raw) = fs::read(path)
            && !raw.is_empty()
        {
            let args: Vec<String> = raw
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect();
            if !args.is_empty() {
                return Some(args);
            }
        }
    }

    let output = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    shell_words::split(&raw)
        .ok()
        .filter(|args| !args.is_empty())
}

fn read_helper_request(stream: &mut UnixStream) -> Result<PaneRequest> {
    let payload = read_limited(stream, "pane helper request")?;
    serde_json::from_slice(&payload).context("failed to decode pane helper request")
}

fn configure_ipc_stream(stream: &UnixStream) -> Result<()> {
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    Ok(())
}

fn read_limited(stream: &mut UnixStream, kind: &str) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    (&mut *stream)
        .take(MAX_IPC_MESSAGE_BYTES + 1)
        .read_to_end(&mut payload)
        .with_context(|| format!("failed to read {kind}"))?;
    if payload.len() as u64 > MAX_IPC_MESSAGE_BYTES {
        bail!("{kind} exceeds {MAX_IPC_MESSAGE_BYTES} byte limit");
    }
    Ok(payload)
}

fn write_helper_response(stream: &mut UnixStream, response: &PaneResponse) -> Result<()> {
    let payload = serde_json::to_vec(response).context("failed to encode pane helper response")?;
    stream
        .write_all(&payload)
        .context("failed to write pane helper response")?;
    Ok(())
}

fn build_command(
    command: &[String],
    admux_context: Option<(&str, WindowId, PaneId)>,
    default_shell: Option<&str>,
) -> CommandBuilder {
    if command.is_empty() {
        let shell = default_shell
            .map(ToOwned::to_owned)
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/sh".into());
        let mut builder = CommandBuilder::new(shell);
        if let Some((session_name, window_id, pane_id)) = admux_context {
            builder.env("ADMUX", "1");
            builder.env("ADMUX_SESSION", session_name);
            builder.env("ADMUX_WINDOW", window_id.0.to_string());
            builder.env("ADMUX_PANE", pane_id.0.to_string());
        }
        builder
    } else {
        let mut builder = CommandBuilder::new(&command[0]);
        if let Some((session_name, window_id, pane_id)) = admux_context {
            builder.env("ADMUX", "1");
            builder.env("ADMUX_SESSION", session_name);
            builder.env("ADMUX_WINDOW", window_id.0.to_string());
            builder.env("ADMUX_PANE", pane_id.0.to_string());
        }
        for arg in &command[1..] {
            builder.arg(arg);
        }
        builder
    }
}

fn wait_for_socket(socket_path: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if socket_path
            .metadata()
            .map(|metadata| metadata.file_type().is_socket())
            .unwrap_or(false)
            && PaneProcess::connect(socket_path.to_path_buf()).is_ok()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(anyhow!(
        "timed out waiting for pane helper socket {}",
        socket_path.display()
    ))
}

fn wait_for_socket_removal(socket_path: &Path) -> Result<()> {
    let deadline = Instant::now() + IPC_TIMEOUT;
    while Instant::now() < deadline {
        if !socket_path.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(anyhow!(
        "timed out waiting for pane helper socket {} to close",
        socket_path.display()
    ))
}

fn resolve_helper_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ADMUX_PANE_BIN") {
        return Ok(path.into());
    }
    let current = std::env::current_exe().context("failed to resolve current executable path")?;
    let candidates = [
        current.with_file_name("admux-pane"),
        current
            .parent()
            .and_then(Path::parent)
            .map(|parent| parent.join("admux-pane"))
            .unwrap_or_else(|| current.with_file_name("admux-pane")),
    ];
    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!(
        "could not locate admux-pane binary near {}",
        current.display()
    )
}

fn unique_helper_name() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let counter = HELPER_NAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Unix-domain socket paths have a small, platform-defined maximum length.
    // Session/window/pane identity is supplied in the helper payload, so it
    // must not be repeated in the filename. PID + timestamp + counter keeps
    // the name unique without allowing user-controlled input to exhaust the
    // pathname budget.
    format!("pane-{pid}-{now}-{counter}.sock")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};
    use tempfile::{TempDir, tempdir as make_tempdir};

    fn helper_dir() -> TempDir {
        make_tempdir().expect("tempdir")
    }

    #[test]
    fn send_keys_decodes_control_and_named_key_tokens() {
        assert_eq!(encode_send_key("C-l"), vec![0x0c]);
        assert_eq!(encode_send_key("Ctrl-c"), vec![0x03]);
        assert_eq!(encode_send_key("M-x"), b"\x1bx".to_vec());
        assert_eq!(encode_send_key("Enter"), b"\r".to_vec());
        assert_eq!(encode_send_key("PageDown"), b"\x1b[6~".to_vec());
        assert_eq!(encode_send_key("F5"), b"\x1b[15~".to_vec());
    }

    #[test]
    fn send_keys_preserves_literal_text() {
        assert_eq!(encode_send_key("echo hello"), b"echo hello".to_vec());
    }

    #[test]
    fn mouse_reports_honor_requested_encoding() {
        assert_eq!(
            encode_mouse_report(vt100::MouseProtocolEncoding::Default, 1, false, false, 2, 4,)
                .expect("default mouse report"),
            b"\x1b[M!%#"
        );
        assert_eq!(
            encode_mouse_report(vt100::MouseProtocolEncoding::Default, 1, false, true, 2, 4,)
                .expect("default release report"),
            b"\x1b[M#%#"
        );
        assert_eq!(
            encode_mouse_report(vt100::MouseProtocolEncoding::Utf8, 2, true, false, 300, 400,)
                .expect("UTF-8 mouse report"),
            format!(
                "\x1b[M{}{}{}",
                66u8 as char,
                char::from_u32(433).unwrap(),
                char::from_u32(333).unwrap()
            )
            .into_bytes()
        );
        assert_eq!(
            encode_mouse_report(vt100::MouseProtocolEncoding::Sgr, 2, false, true, 2, 4,)
                .expect("SGR release report"),
            b"\x1b[<2;5;3m"
        );
    }

    #[test]
    fn mouse_reports_honor_requested_motion_mode() {
        assert!(mouse_event_is_requested(
            vt100::MouseProtocolMode::Press,
            false,
            false
        ));
        assert!(!mouse_event_is_requested(
            vt100::MouseProtocolMode::Press,
            true,
            false
        ));
        assert!(!mouse_event_is_requested(
            vt100::MouseProtocolMode::PressRelease,
            true,
            false
        ));
        assert!(mouse_event_is_requested(
            vt100::MouseProtocolMode::PressRelease,
            false,
            true
        ));
        assert!(mouse_event_is_requested(
            vt100::MouseProtocolMode::ButtonMotion,
            true,
            false
        ));
    }

    #[test]
    fn default_mouse_encoding_rejects_unrepresentable_coordinates() {
        let error = encode_mouse_report(
            vt100::MouseProtocolEncoding::Default,
            0,
            false,
            false,
            0,
            223,
        )
        .expect_err("default encoding must not wrap coordinates");
        assert!(error.to_string().contains("column exceeds"));
    }

    #[test]
    fn connect_rejects_incompatible_helper_protocol() {
        let dir = helper_dir();
        let socket = dir.path().join("helper");
        let listener = UnixListener::bind(&socket).expect("bind helper socket");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept handshake");
            let mut request = Vec::new();
            stream.read_to_end(&mut request).expect("read handshake");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&request).expect("decode handshake"),
                serde_json::json!({"Hello":{"version":HELPER_PROTOCOL_VERSION}}),
            );
            stream
                .write_all(br#"{"HelloAck":{"version":999}}"#)
                .expect("write mismatched handshake");
        });

        let error = PaneProcess::connect(socket).expect_err("incompatible helper must fail");
        assert!(error.to_string().contains("protocol mismatch"));
        server.join().expect("server thread");
    }

    #[test]
    fn invalid_persistent_snapshot_wire_is_an_error_not_a_panic() {
        let invalid_base64 = PanePersistentSnapshotWire {
            rows: 24,
            cols: 80,
            vt_b64: "not base64!".into(),
            command: Vec::new(),
        };
        assert!(PanePersistentSnapshot::try_from(invalid_base64).is_err());

        let invalid_utf8 = PanePersistentSnapshotWire {
            rows: 24,
            cols: 80,
            vt_b64: STANDARD.encode([0xff]),
            command: Vec::new(),
        };
        assert!(PanePersistentSnapshot::try_from(invalid_utf8).is_err());
    }

    #[test]
    fn helper_startup_failure_removes_its_prebound_socket() {
        let dir = helper_dir();
        let socket = dir.path().join("helper.sock");
        let error = run_helper(PaneHelperArgs {
            socket: socket.clone(),
            cwd: None,
            session_name: None,
            window_id: None,
            pane_id: None,
            default_shell: None,
            scrollback_lines: 10_000,
            command: vec!["/definitely/not/an-admux-command".into()],
            restore_seed: None,
        })
        .expect_err("invalid command should fail helper startup");

        assert!(error.to_string().contains("failed to spawn pane command"));
        assert!(!socket.exists(), "failed helper startup must remove its socket");
    }

    #[test]
    fn history_truncation_never_starts_inside_utf8_or_csi() {
        let mut history = b"12345678\xc3\xa9abcdef\x1b[38;2;255;0;0mgreen".to_vec();

        truncate_history_at_safe_boundary(&mut history, 12);

        assert!(std::str::from_utf8(&history).is_ok());
        assert!(
            !history.starts_with(b"\x1b[") && history.starts_with(b"green"),
            "history should resume after the completed CSI sequence"
        );
    }

    #[test]
    fn history_truncation_discards_unterminated_control_strings() {
        let mut history = b"prefix\x1b]0;unterminated-title".to_vec();

        truncate_history_at_safe_boundary(&mut history, 5);

        assert!(history.is_empty());
    }

    fn wait_for_preview(pane: &PaneProcess, needle: &str) -> String {
        for _ in 0..50 {
            let preview = pane.preview();
            if preview.contains(needle) {
                return preview;
            }
            thread::sleep(Duration::from_millis(20));
        }
        pane.preview()
    }

    #[test]
    fn pane_process_captures_command_output() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &["sh".into(), "-lc".into(), "printf 'hello from pane'".into()],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");

        assert!(wait_for_preview(&pane, "hello from pane").contains("hello from pane"));
    }

    #[test]
    fn pane_process_handles_clear_screen_sequences() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &[
                "sh".into(),
                "-lc".into(),
                "printf 'before'; printf '\\033[2J\\033[Hafter'".into(),
            ],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");

        let preview = wait_for_preview(&pane, "after");
        assert!(preview.contains("after"));
        assert!(!preview.contains("beforeafter"));
    }

    #[test]
    fn pane_snapshot_reports_application_cursor_mode() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &[
                "sh".into(),
                "-lc".into(),
                "printf '\\033[?1h'; sleep 1".into(),
            ],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");

        let mut application_cursor = false;
        for _ in 0..50 {
            application_cursor = pane
                .render(80, 24)
                .expect("render pane")
                .application_cursor;
            if application_cursor {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(application_cursor);
        pane.kill().expect("clean up pane");
    }

    #[test]
    fn pane_process_restores_history_after_expanding() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &[
                "sh".into(),
                "-lc".into(),
                "printf 'one two three four five six seven eight nine ten'".into(),
            ],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");

        let _ = wait_for_preview(&pane, "one two");
        pane.resize(24, 10).expect("resize pane");
        let shrunk = pane.preview();
        assert!(shrunk.contains("one two"));

        pane.resize(24, 80).expect("resize pane");

        let preview = pane.preview();
        assert!(preview.contains("three"));
        assert!(preview.contains("seven"));
    }

    #[test]
    fn pane_process_replays_history_when_one_resize_axis_expands() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &[
                "sh".into(),
                "-lc".into(),
                "printf 'one two three four five six seven eight nine ten'; sleep 1".into(),
            ],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");

        let _ = wait_for_preview(&pane, "one two");
        pane.resize(20, 10).expect("shrink pane width");
        pane.resize(10, 80)
            .expect("expand width while shrinking height");

        assert!(
            pane.preview().contains("one two three four five six seven"),
            "an expanding axis must replay history even when the other shrinks"
        );
    }

    #[test]
    fn pane_process_can_reconnect_to_existing_helper() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &[
                "sh".into(),
                "-lc".into(),
                "printf reconnect-test; sleep 1".into(),
            ],
            None,
            Some(("work", WindowId(1), PaneId(0))),
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");

        let reconnected =
            PaneProcess::connect(pane.socket_path().to_path_buf()).expect("reconnect helper");
        assert!(wait_for_preview(&reconnected, "reconnect-test").contains("reconnect-test"));
    }

    #[test]
    fn scrollback_reports_helper_transport_failures() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &["sh".into(), "-lc".into(), "sleep 1".into()],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");
        let socket = pane.socket_path().to_path_buf();
        let hidden = socket.with_extension("hidden");
        fs::rename(&socket, &hidden).expect("hide helper socket");

        assert!(pane.scroll_scrollback_by(1).is_err());

        fs::rename(&hidden, &socket).expect("restore helper socket");
        pane.kill().expect("clean up helper");
    }

    #[test]
    fn selection_text_reports_helper_transport_failures() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &["sh".into(), "-lc".into(), "printf selected; sleep 1".into()],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");
        let socket = pane.socket_path().to_path_buf();
        let hidden = socket.with_extension("hidden");
        fs::rename(&socket, &hidden).expect("hide helper socket");

        assert!(pane.selection_text(0, 0, 0, 7).is_err());

        fs::rename(&hidden, &socket).expect("restore helper socket");
        pane.kill().expect("clean up helper");
    }

    #[test]
    fn pane_process_can_restore_persistent_snapshot() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &[
                "sh".into(),
                "-lc".into(),
                "printf 'snapshot-one\\nsnapshot-two'; sleep 1".into(),
            ],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");
        assert!(wait_for_preview(&pane, "snapshot-two").contains("snapshot-two"));
        let snapshot = pane.persistent_snapshot(500).expect("persistent snapshot");
        let restored = PaneProcess::spawn(
            &["sh".into(), "-lc".into(), "sleep 1".into()],
            None,
            None,
            None,
            10_000,
            dir.path(),
            Some((&snapshot).into()),
        )
        .expect("spawn restored pane");
        let preview = wait_for_preview(&restored, "snapshot-two");
        assert!(preview.contains("snapshot-one"));
        assert!(preview.contains("snapshot-two"));
    }

    #[test]
    fn persistent_snapshot_prefers_foreground_command() {
        let dir = helper_dir();
        let pane = PaneProcess::spawn(
            &["sh".into(), "-lc".into(), "exec sleep 3".into()],
            None,
            None,
            None,
            10_000,
            dir.path(),
            None,
        )
        .expect("spawn pane");
        let mut command = None;
        for _ in 0..50 {
            let snapshot = pane.persistent_snapshot(500).expect("persistent snapshot");
            command = snapshot.command.first().cloned();
            if command.as_deref() == Some("sleep") {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(command.as_deref(), Some("sleep"));
        pane.kill().expect("clean up pane");
    }

    #[test]
    fn helper_socket_names_are_bounded_and_unique() {
        let dir = helper_dir();
        let first = unique_helper_name();
        let second = unique_helper_name();

        assert!(
            first.len() < 80,
            "socket filename should preserve pathname budget"
        );
        assert!(
            dir.path().join(&first).as_os_str().len() < 100,
            "full helper socket path should fit typical Unix-domain socket limits"
        );
        assert_ne!(first, second);
    }

    #[test]
    fn helper_directory_and_socket_are_private() {
        let dir = helper_dir();
        let helper_root = dir.path().join("helpers");
        ensure_private_helper_directory(&helper_root).expect("create private helper directory");
        assert_eq!(
            fs::metadata(&helper_root).expect("directory metadata").mode() & 0o777,
            0o700
        );

        let pane = PaneProcess::spawn(
            &["sh".into(), "-lc".into(), "sleep 1".into()],
            None,
            None,
            None,
            10_000,
            &helper_root,
            None,
        )
        .expect("spawn pane");
        assert_eq!(
            fs::metadata(pane.socket_path())
                .expect("socket metadata")
                .mode()
                & 0o777,
            0o600
        );
        pane.kill().expect("clean up pane");
    }

    #[test]
    fn helper_directory_restricts_insecure_existing_permissions() {
        let dir = helper_dir();
        let helper_root = dir.path().join("insecure");
        fs::create_dir(&helper_root).expect("create helper directory");
        fs::set_permissions(&helper_root, fs::Permissions::from_mode(0o755))
            .expect("make directory insecure");
        ensure_private_helper_directory(&helper_root).expect("restrict helper directory");
        assert_eq!(
            fs::metadata(&helper_root).expect("directory metadata").mode() & 0o777,
            0o700
        );
    }
}
