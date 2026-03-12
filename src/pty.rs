use crate::{
    ipc::ScrollDirection,
    pane::{PaneId, WindowId},
};
use anyhow::{Context, Result, anyhow, bail};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HISTORY_LIMIT: usize = 2 * 1024 * 1024;

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
    pub alive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneProcess {
    socket_path: PathBuf,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum PaneRequest {
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
    Scrollback {
        lines: i16,
    },
    SendKeys {
        keys: Vec<String>,
    },
    Shutdown,
    IsAlive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum PaneResponse {
    Snapshot(PaneSnapshotWire),
    ScreenSize { rows: u16, cols: u16 },
    SelectionText { text: String },
    IsAlive { alive: bool },
    Ok,
    Error { message: String },
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
    alive: bool,
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
            alive: value.alive,
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
    ) -> Result<Self> {
        fs::create_dir_all(helper_dir).with_context(|| {
            format!("failed to create helper directory {}", helper_dir.display())
        })?;
        let socket_path = helper_dir.join(unique_helper_name(admux_context));
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
        };
        let payload = serde_json::to_string(&args).context("failed to encode helper args")?;

        Command::new(helper_bin)
            .arg(payload)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to spawn admux-pane helper")?;

        wait_for_socket(&socket_path)?;
        Ok(Self { socket_path })
    }

    pub fn connect(socket_path: PathBuf) -> Result<Self> {
        if !socket_path.exists() {
            bail!("missing pane helper socket {}", socket_path.display());
        }
        let process = Self { socket_path };
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
    ) -> String {
        match self.request(PaneRequest::SelectionText {
            start_row,
            start_col,
            end_row,
            end_col,
        }) {
            Ok(PaneResponse::SelectionText { text }) => text,
            _ => String::new(),
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

    pub fn scroll_scrollback_by(&self, lines: i16) {
        let _ = self.request(PaneRequest::Scrollback { lines });
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
            PaneResponse::Ok => Ok(()),
            PaneResponse::Error { message } => Err(anyhow!(message)),
            other => Err(anyhow!("unexpected shutdown response: {other:?}")),
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
        let payload = serde_json::to_vec(&request).context("failed to encode pane request")?;
        stream
            .write_all(&payload)
            .context("failed to write pane request")?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .context("failed to finish pane request")?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .context("failed to read pane response")?;
        serde_json::from_slice(&response).context("failed to decode pane response")
    }
}

pub fn run_helper(args: PaneHelperArgs) -> Result<()> {
    if let Some(parent) = args.socket.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create pane helper directory {}",
                parent.display()
            )
        })?;
    }
    if args.socket.exists() {
        fs::remove_file(&args.socket).with_context(|| {
            format!(
                "failed to remove stale pane helper socket {}",
                args.socket.display()
            )
        })?;
    }

    let state = Arc::new(start_helper_state(&args)?);
    let listener = UnixListener::bind(&args.socket).with_context(|| {
        format!(
            "failed to bind pane helper socket {}",
            args.socket.display()
        )
    })?;

    for stream in listener.incoming() {
        let mut stream = stream.context("failed to accept pane helper client")?;
        let request = read_helper_request(&mut stream)?;
        let shutdown = matches!(request, PaneRequest::Shutdown);
        let response = handle_helper_request(&state, request);
        write_helper_response(&mut stream, &response)?;
        if shutdown {
            break;
        }
    }

    let _ = fs::remove_file(&args.socket);
    Ok(())
}

fn start_helper_state(args: &PaneHelperArgs) -> Result<HelperState> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
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
        parser: vt100::Parser::new(24, 80, args.scrollback_lines),
        history: Vec::new(),
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
                        if terminal.history.len() > HISTORY_LIMIT {
                            let drop_len = terminal.history.len() - HISTORY_LIMIT;
                            terminal.history.drain(..drop_len);
                        }
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

fn handle_helper_request(state: &Arc<HelperState>, request: PaneRequest) -> PaneResponse {
    match request {
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
        PaneRequest::Shutdown => {
            let _ = state
                .child
                .lock()
                .expect("pane helper child lock poisoned")
                .kill();
            PaneResponse::Ok
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
    if rows < current_rows || cols < current_cols {
        terminal.parser.screen_mut().set_size(rows, cols);
    } else {
        let history = terminal.history.clone();
        let mut parser = vt100::Parser::new(rows, cols, terminal.scrollback_lines);
        parser.process(&history);
        terminal.parser = parser;
    }
    Ok(())
}

fn helper_mouse_scroll(
    state: &Arc<HelperState>,
    direction: ScrollDirection,
    row: u16,
    col: u16,
) -> Result<()> {
    let mouse_mode = state
        .terminal
        .lock()
        .expect("pane helper terminal lock poisoned")
        .parser
        .screen()
        .mouse_protocol_mode();

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
    let sgr = format!("\x1b[<{};{};{}M", code, col + 1, row + 1);
    let mut writer = state
        .writer
        .lock()
        .expect("pane helper writer lock poisoned");
    writer
        .write_all(sgr.as_bytes())
        .context("failed to write mouse scroll bytes")?;
    writer.flush().context("failed to flush PTY writer")?;
    Ok(())
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
            .write_all(key.as_bytes())
            .context("failed to write key bytes")?;
    }
    writer.flush().context("failed to flush PTY writer")?;
    Ok(())
}

fn read_helper_request(stream: &mut UnixStream) -> Result<PaneRequest> {
    let mut payload = Vec::new();
    stream
        .read_to_end(&mut payload)
        .context("failed to read pane helper request")?;
    serde_json::from_slice(&payload).context("failed to decode pane helper request")
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
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if socket_path.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(anyhow!(
        "timed out waiting for pane helper socket {}",
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

fn unique_helper_name(admux_context: Option<(&str, WindowId, PaneId)>) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    match admux_context {
        Some((session, window_id, pane_id)) => format!(
            "{}-{}-{}-{}.sock",
            sanitize_component(session),
            window_id.0,
            pane_id.0,
            now
        ),
        None => format!("pane-{}.sock", now),
    }
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};
    use tempfile::tempdir;

    fn helper_dir() -> tempfile::TempDir {
        tempdir().expect("tempdir")
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
        )
        .expect("spawn pane");

        thread::sleep(Duration::from_millis(100));
        assert!(pane.preview().contains("hello from pane"));
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
        )
        .expect("spawn pane");

        thread::sleep(Duration::from_millis(100));
        let preview = pane.preview();
        assert!(preview.contains("after"));
        assert!(!preview.contains("beforeafter"));
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
        )
        .expect("spawn pane");

        thread::sleep(Duration::from_millis(100));
        pane.resize(24, 10).expect("resize pane");
        let shrunk = pane.preview();
        assert!(shrunk.contains("one two"));

        pane.resize(24, 80).expect("resize pane");

        let preview = pane.preview();
        assert!(preview.contains("three"));
        assert!(preview.contains("seven"));
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
        )
        .expect("spawn pane");

        thread::sleep(Duration::from_millis(100));
        let reconnected =
            PaneProcess::connect(pane.socket_path().to_path_buf()).expect("reconnect helper");
        assert!(reconnected.preview().contains("reconnect-test"));
    }
}
