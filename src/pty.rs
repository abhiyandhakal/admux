use crate::ipc::ScrollDirection;
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread,
};

const HISTORY_LIMIT: usize = 2 * 1024 * 1024;

struct TerminalState {
    parser: vt100::Parser,
    history: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyState {
    Detached,
    Attached,
}

pub struct PaneProcess {
    state: Arc<Mutex<TerminalState>>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send>>,
}

impl PaneProcess {
    pub fn spawn(
        command: &[String],
        cwd: Option<&Path>,
        admux_context: Option<(&str, crate::pane::PaneId)>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to create PTY pair")?;

        let mut builder = build_command(command, admux_context);
        if let Some(cwd) = cwd {
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
        let state = Arc::new(Mutex::new(TerminalState {
            parser: vt100::Parser::new(24, 80, 10_000),
            history: Vec::new(),
        }));
        let state_clone = Arc::clone(&state);

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(size) => {
                        if let Ok(mut state) = state_clone.lock() {
                            state.history.extend_from_slice(&buf[..size]);
                            if state.history.len() > HISTORY_LIMIT {
                                let drop_len = state.history.len() - HISTORY_LIMIT;
                                state.history.drain(..drop_len);
                            }
                            state.parser.process(&buf[..size]);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            state,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
        })
    }

    pub fn preview(&self) -> String {
        self.state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .contents()
    }

    pub fn formatted_preview(&self) -> String {
        let bytes = self
            .state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .contents_formatted();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn formatted_cursor(&self) -> String {
        let bytes = self
            .state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .cursor_state_formatted();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn visible_rows(&self, width: u16, height: u16) -> Vec<String> {
        self.state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .rows(0, width)
            .take(height as usize)
            .collect()
    }

    pub fn visible_rows_formatted(&self, width: u16, height: u16) -> Vec<String> {
        self.state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .rows_formatted(0, width)
            .take(height as usize)
            .map(|row| String::from_utf8_lossy(&row).into_owned())
            .collect()
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        self.state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .cursor_position()
    }

    pub fn screen_size(&self) -> (u16, u16) {
        self.state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .size()
    }

    pub fn selection_text(
        &self,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    ) -> String {
        self.state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .contents_between(start_row, start_col, end_row, end_col)
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let (current_rows, current_cols) = self.screen_size();
        self.master
            .lock()
            .expect("pane master lock poisoned")
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize PTY")?;
        let mut state = self.state.lock().expect("pane state lock poisoned");
        if rows < current_rows || cols < current_cols {
            state.parser.screen_mut().set_size(rows, cols);
        } else {
            let history = state.history.clone();
            let mut parser = vt100::Parser::new(rows, cols, 10_000);
            parser.process(&history);
            state.parser = parser;
        }
        Ok(())
    }

    pub fn handle_mouse_scroll(
        &self,
        direction: ScrollDirection,
        row: u16,
        col: u16,
    ) -> Result<()> {
        let mouse_mode = self
            .state
            .lock()
            .expect("pane state lock poisoned")
            .parser
            .screen()
            .mouse_protocol_mode();

        if mouse_mode == vt100::MouseProtocolMode::None {
            let mut state = self.state.lock().expect("pane state lock poisoned");
            let current = state.parser.screen().scrollback();
            let next = match direction {
                ScrollDirection::Up => current.saturating_add(3),
                ScrollDirection::Down => current.saturating_sub(3),
            };
            state.parser.screen_mut().set_scrollback(next);
            return Ok(());
        }

        let code = match direction {
            ScrollDirection::Up => 64,
            ScrollDirection::Down => 65,
        };
        let sgr = format!("\x1b[<{};{};{}M", code, col + 1, row + 1);
        let mut writer = self.writer.lock().expect("pane writer lock poisoned");
        writer
            .write_all(sgr.as_bytes())
            .context("failed to write mouse scroll bytes")?;
        writer.flush().context("failed to flush PTY writer")?;
        Ok(())
    }

    pub fn send_keys(&self, keys: &[String]) -> Result<()> {
        let mut writer = self.writer.lock().expect("pane writer lock poisoned");
        for key in keys {
            writer
                .write_all(key.as_bytes())
                .context("failed to write key bytes")?;
        }
        writer.flush().context("failed to flush PTY writer")?;
        Ok(())
    }

    pub fn kill(&self) -> Result<()> {
        self.child
            .lock()
            .expect("pane child lock poisoned")
            .kill()
            .context("failed to kill pane process")?;
        Ok(())
    }

    pub fn is_alive(&self) -> bool {
        self.child
            .lock()
            .expect("pane child lock poisoned")
            .try_wait()
            .map(|status| status.is_none())
            .unwrap_or(false)
    }
}

fn build_command(
    command: &[String],
    admux_context: Option<(&str, crate::pane::PaneId)>,
) -> CommandBuilder {
    if command.is_empty() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut builder = CommandBuilder::new(shell);
        if let Some((session_name, pane_id)) = admux_context {
            builder.env("ADMUX", "1");
            builder.env("ADMUX_SESSION", session_name);
            builder.env("ADMUX_PANE", pane_id.0.to_string());
        }
        builder
    } else {
        let mut builder = CommandBuilder::new(&command[0]);
        if let Some((session_name, pane_id)) = admux_context {
            builder.env("ADMUX", "1");
            builder.env("ADMUX_SESSION", session_name);
            builder.env("ADMUX_PANE", pane_id.0.to_string());
        }
        for arg in &command[1..] {
            builder.arg(arg);
        }
        builder
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};

    #[test]
    fn pane_process_captures_command_output() {
        let pane = PaneProcess::spawn(
            &["sh".into(), "-lc".into(), "printf 'hello from pane'".into()],
            None,
            None,
        )
        .expect("spawn pane");

        thread::sleep(Duration::from_millis(100));
        assert!(pane.preview().contains("hello from pane"));
    }

    #[test]
    fn pane_process_handles_clear_screen_sequences() {
        let pane = PaneProcess::spawn(
            &[
                "sh".into(),
                "-lc".into(),
                "printf 'before'; printf '\\033[2J\\033[Hafter'".into(),
            ],
            None,
            None,
        )
        .expect("spawn pane");

        thread::sleep(Duration::from_millis(100));
        let preview = pane.preview();
        assert!(preview.contains("after"));
        assert!(!preview.contains("beforeafter"));
    }

    #[test]
    fn pane_process_restores_history_after_expanding() {
        let pane = PaneProcess::spawn(
            &[
                "sh".into(),
                "-lc".into(),
                "printf 'one two three four five six seven eight nine ten'".into(),
            ],
            None,
            None,
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
}
