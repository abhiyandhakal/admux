use crate::ipc::ScrollDirection;
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyState {
    Detached,
    Attached,
}

pub struct PaneProcess {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send>>,
}

impl PaneProcess {
    pub fn spawn(command: &[String], cwd: Option<&Path>) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to create PTY pair")?;

        let mut builder = build_command(command);
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
        let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 10_000)));
        let parser_clone = Arc::clone(&parser);

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(size) => {
                        if let Ok(mut parser) = parser_clone.lock() {
                            parser.process(&buf[..size]);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            parser,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
        })
    }

    pub fn preview(&self) -> String {
        self.parser
            .lock()
            .expect("pane parser lock poisoned")
            .screen()
            .contents()
    }

    pub fn formatted_preview(&self) -> String {
        let bytes = self
            .parser
            .lock()
            .expect("pane parser lock poisoned")
            .screen()
            .contents_formatted();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn formatted_cursor(&self) -> String {
        let bytes = self
            .parser
            .lock()
            .expect("pane parser lock poisoned")
            .screen()
            .cursor_state_formatted();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
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
        self.parser
            .lock()
            .expect("pane parser lock poisoned")
            .screen_mut()
            .set_size(rows, cols);
        Ok(())
    }

    pub fn handle_mouse_scroll(
        &self,
        direction: ScrollDirection,
        row: u16,
        col: u16,
    ) -> Result<()> {
        let mouse_mode = self
            .parser
            .lock()
            .expect("pane parser lock poisoned")
            .screen()
            .mouse_protocol_mode();

        if mouse_mode == vt100::MouseProtocolMode::None {
            let mut parser = self.parser.lock().expect("pane parser lock poisoned");
            let current = parser.screen().scrollback();
            let next = match direction {
                ScrollDirection::Up => current.saturating_add(3),
                ScrollDirection::Down => current.saturating_sub(3),
            };
            parser.screen_mut().set_scrollback(next);
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
}

fn build_command(command: &[String]) -> CommandBuilder {
    if command.is_empty() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        CommandBuilder::new(shell)
    } else {
        let mut builder = CommandBuilder::new(&command[0]);
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
        )
        .expect("spawn pane");

        thread::sleep(Duration::from_millis(100));
        let preview = pane.preview();
        assert!(preview.contains("after"));
        assert!(!preview.contains("beforeafter"));
    }
}
