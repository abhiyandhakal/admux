use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};

use crate::{
    ipc::{
        CURRENT_PROTOCOL_VERSION, CommandRequest, CommandResponse, CycleDirection,
        NavigationDirection, ProtocolVersion,
    },
    pane::{PaneId, WindowId},
    session::Session,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Starting,
    Running,
}

#[derive(Default)]
pub struct SessionStore {
    sessions: BTreeMap<String, Session>,
    last_session: Option<String>,
    next_pane_id: u64,
    next_window_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetRef {
    session: String,
    window: Option<WindowId>,
    pane: Option<PaneId>,
}

impl SessionStore {
    pub fn handle(&mut self, request: CommandRequest) -> CommandResponse {
        self.prune_dead_sessions();

        match request {
            CommandRequest::Hello { version } => self.handle_hello(version),
            CommandRequest::NewSession { name, cwd, command } => {
                let name = name.unwrap_or_else(|| format!("session-{}", self.sessions.len() + 1));
                let window_id = self.next_window();
                let pane_id = self.next_pane();
                match Session::new(name.clone(), cwd, command, window_id, pane_id) {
                    Ok(session) => {
                        self.sessions.insert(name.clone(), session);
                        self.last_session = Some(name.clone());
                        CommandResponse::SessionCreated {
                            session: name,
                            pane_id: pane_id.0,
                        }
                    }
                    Err(error) => CommandResponse::Error {
                        message: error.to_string(),
                    },
                }
            }
            CommandRequest::Attach { session } => {
                let Some(session_name) = self.resolve_session(session) else {
                    return CommandResponse::Error {
                        message: "no sessions available".into(),
                    };
                };
                let Some(session) = self.sessions.get(&session_name) else {
                    return CommandResponse::Error {
                        message: format!("unknown session {session_name}"),
                    };
                };
                let snapshot = session.render_snapshot(session.pane_area());
                CommandResponse::Attached {
                    session: session_name,
                    preview: session.active_pane_preview(),
                    formatted_preview: session.active_pane_formatted_preview(),
                    formatted_cursor: session.active_pane_formatted_cursor(),
                    snapshot,
                }
            }
            CommandRequest::ListSessions => CommandResponse::SessionList {
                sessions: self.sessions.keys().cloned().collect(),
            },
            CommandRequest::ListWindows { session } => match self.sessions.get(&session) {
                Some(session) => CommandResponse::WindowList {
                    windows: session.list_windows(),
                },
                None => CommandResponse::Error {
                    message: format!("unknown session {session}"),
                },
            },
            CommandRequest::ListPanes { target } => match parse_target(&target) {
                Ok(target) => match self.sessions.get(&target.session) {
                    Some(session) => CommandResponse::PaneList {
                        panes: session.list_panes(target.window),
                    },
                    None => CommandResponse::Error {
                        message: format!("unknown session {}", target.session),
                    },
                },
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::KillSession { session } => {
                if let Some(removed) = self.sessions.remove(&session) {
                    let _ = removed.kill();
                    if self.last_session.as_deref() == Some(session.as_str()) {
                        self.last_session = self.sessions.keys().next_back().cloned();
                    }
                    CommandResponse::SessionKilled { session }
                } else {
                    CommandResponse::Error {
                        message: format!("unknown session {session}"),
                    }
                }
            }
            CommandRequest::KillWindow { target } => match parse_target(&target) {
                Ok(target) => self.kill_window(target),
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::KillPane { target } => match parse_target(&target) {
                Ok(target) => self.kill_pane(target),
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::SendKeys { target, keys } => match parse_target(&target) {
                Ok(target) => match self.sessions.get(&target.session) {
                    Some(session) => match session.send_keys(target.window, target.pane, &keys) {
                        Ok(_) => CommandResponse::KeysSent,
                        Err(error) => CommandResponse::Error {
                            message: error.to_string(),
                        },
                    },
                    None => CommandResponse::Error {
                        message: format!("unknown session {}", target.session),
                    },
                },
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::SplitPane {
                target,
                axis,
                command,
            } => match parse_target(&target) {
                Ok(target) => self.split_pane(target, axis, command),
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::NewWindow {
                session,
                name,
                command,
            } => {
                let window_id = self.next_window();
                let pane_id = self.next_pane();
                match self.sessions.get_mut(&session) {
                    Some(session) => match session.new_window(window_id, pane_id, name, &command) {
                        Ok(created) => CommandResponse::WindowCreated {
                            session: session.name.clone(),
                            window_id: created.window_id.0,
                            pane_id: created.pane_id.0,
                        },
                        Err(error) => CommandResponse::Error {
                            message: error.to_string(),
                        },
                    },
                    None => CommandResponse::Error {
                        message: format!("unknown session {session}"),
                    },
                }
            }
            CommandRequest::SelectPane { target, direction } => self.select_pane(target, direction),
            CommandRequest::SelectWindow { target } => match parse_target(&target) {
                Ok(target) => self.select_window(target),
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::CycleWindow { session, direction } => {
                match self.sessions.get_mut(&session) {
                    Some(session) => {
                        match session.cycle_window(matches!(direction, CycleDirection::Next)) {
                            Ok(_) => CommandResponse::FocusChanged,
                            Err(error) => CommandResponse::Error {
                                message: error.to_string(),
                            },
                        }
                    }
                    None => CommandResponse::Error {
                        message: format!("unknown session {session}"),
                    },
                }
            }
            CommandRequest::ResizePane {
                target,
                direction,
                amount,
            } => match parse_target(&target) {
                Ok(target) => match self.sessions.get_mut(&target.session) {
                    Some(session) => {
                        match session.resize_active_pane(target.window, direction, amount) {
                            Ok(_) => CommandResponse::Resized,
                            Err(error) => CommandResponse::Error {
                                message: error.to_string(),
                            },
                        }
                    }
                    None => CommandResponse::Error {
                        message: format!("unknown session {}", target.session),
                    },
                },
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::RenameWindow { target, name } => match parse_target(&target) {
                Ok(target) => self.rename_window(target, name),
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::MouseScroll {
                session,
                row,
                col,
                direction,
            } => match self.sessions.get(&session) {
                Some(session) => {
                    let pane_id = session
                        .render_snapshot(session.pane_area())
                        .and_then(|snapshot| {
                            snapshot
                                .panes
                                .into_iter()
                                .find(|pane| pane.rect.contains(row, col))
                        })
                        .map(|pane| PaneId(pane.pane_id));
                    match session.handle_mouse_scroll(pane_id, direction, row, col) {
                        Ok(_) => CommandResponse::Scrolled,
                        Err(error) => CommandResponse::Error {
                            message: error.to_string(),
                        },
                    }
                }
                None => CommandResponse::Error {
                    message: format!("unknown session {session}"),
                },
            },
            CommandRequest::CopySelection {
                session,
                pane_id,
                start_row,
                start_col,
                end_row,
                end_col,
            } => match self.sessions.get(&session) {
                Some(session) => CommandResponse::SelectionCopied {
                    text: session.active_pane_selection_text(
                        pane_id.map(PaneId),
                        start_row,
                        start_col,
                        end_row,
                        end_col,
                    ),
                },
                None => CommandResponse::Error {
                    message: format!("unknown session {session}"),
                },
            },
            CommandRequest::Resize {
                session,
                rows,
                cols,
            } => match self.sessions.get_mut(&session) {
                Some(session) => match session.set_viewport(rows, cols) {
                    Ok(_) => CommandResponse::Resized,
                    Err(error) => CommandResponse::Error {
                        message: error.to_string(),
                    },
                },
                None => CommandResponse::Error {
                    message: format!("unknown session {session}"),
                },
            },
            CommandRequest::ReloadConfig => CommandResponse::ConfigReloaded,
        }
    }

    fn next_pane(&mut self) -> PaneId {
        self.next_pane_id += 1;
        PaneId(self.next_pane_id)
    }

    fn next_window(&mut self) -> WindowId {
        self.next_window_id += 1;
        WindowId(self.next_window_id)
    }

    fn prune_dead_sessions(&mut self) {
        self.sessions.retain(|_, session| session.prune_dead());
        if let Some(last) = self.last_session.as_ref()
            && !self.sessions.contains_key(last)
        {
            self.last_session = self.sessions.keys().next_back().cloned();
        }
    }

    fn handle_hello(&self, version: ProtocolVersion) -> CommandResponse {
        if version == CURRENT_PROTOCOL_VERSION {
            CommandResponse::HelloAck { version }
        } else {
            CommandResponse::Error {
                message: format!(
                    "protocol mismatch: client={}, server={}",
                    version.0, CURRENT_PROTOCOL_VERSION.0
                ),
            }
        }
    }

    fn resolve_session(&self, requested: Option<String>) -> Option<String> {
        match requested {
            Some(session) if self.sessions.contains_key(&session) => Some(session),
            Some(_) => None,
            None => self.last_session.clone(),
        }
    }

    fn split_pane(
        &mut self,
        target: TargetRef,
        axis: crate::layout::SplitAxis,
        command: Vec<String>,
    ) -> CommandResponse {
        let pane_id = self.next_pane();
        match self.sessions.get_mut(&target.session) {
            Some(session) => {
                if let Some(window) = target.window {
                    session.active_window = window;
                }
                if let Some(pane) = target.pane {
                    let _ = session.select_pane(target.window, pane);
                }
                match session.split_active_pane(axis, pane_id, &command) {
                    Ok(split) => CommandResponse::PaneSplit {
                        session: session.name.clone(),
                        window_id: split.window_id.0,
                        pane_id: split.pane_id.0,
                    },
                    Err(error) => CommandResponse::Error {
                        message: error.to_string(),
                    },
                }
            }
            None => CommandResponse::Error {
                message: format!("unknown session {}", target.session),
            },
        }
    }

    fn select_pane(
        &mut self,
        target: Option<String>,
        direction: Option<NavigationDirection>,
    ) -> CommandResponse {
        match (target, direction) {
            (Some(target), _) => match parse_target(&target) {
                Ok(target) => match self.sessions.get_mut(&target.session) {
                    Some(session) => match target.pane {
                        Some(pane_id) => match session.select_pane(target.window, pane_id) {
                            Ok(_) => CommandResponse::FocusChanged,
                            Err(error) => CommandResponse::Error {
                                message: error.to_string(),
                            },
                        },
                        None => CommandResponse::Error {
                            message: "select-pane target requires a pane id".into(),
                        },
                    },
                    None => CommandResponse::Error {
                        message: format!("unknown session {}", target.session),
                    },
                },
                Err(message) => CommandResponse::Error { message },
            },
            (None, Some(direction)) => {
                let Some(session_name) = self.last_session.clone() else {
                    return CommandResponse::Error {
                        message: "no sessions available".into(),
                    };
                };
                match self.sessions.get_mut(&session_name) {
                    Some(session) => match session.move_focus(direction, session.pane_area()) {
                        Ok(_) => CommandResponse::FocusChanged,
                        Err(error) => CommandResponse::Error {
                            message: error.to_string(),
                        },
                    },
                    None => CommandResponse::Error {
                        message: format!("unknown session {session_name}"),
                    },
                }
            }
            _ => CommandResponse::Error {
                message: "select-pane requires a target or direction".into(),
            },
        }
    }

    fn select_window(&mut self, target: TargetRef) -> CommandResponse {
        match self.sessions.get_mut(&target.session) {
            Some(session) => match target.window {
                Some(window_id) => match session.select_window(window_id) {
                    Ok(_) => CommandResponse::FocusChanged,
                    Err(error) => CommandResponse::Error {
                        message: error.to_string(),
                    },
                },
                None => CommandResponse::Error {
                    message: "select-window target requires a window id".into(),
                },
            },
            None => CommandResponse::Error {
                message: format!("unknown session {}", target.session),
            },
        }
    }

    fn kill_pane(&mut self, target: TargetRef) -> CommandResponse {
        match self.sessions.get_mut(&target.session) {
            Some(session) => match session.kill_pane(target.window, target.pane) {
                Ok(Some(killed)) => CommandResponse::PaneKilled {
                    session: session.name.clone(),
                    window_id: killed.window_id.0,
                    pane_id: killed.pane_id.0,
                },
                Ok(None) => {
                    if !session.is_alive() {
                        self.sessions.remove(&target.session);
                    }
                    CommandResponse::FocusChanged
                }
                Err(error) => CommandResponse::Error {
                    message: error.to_string(),
                },
            },
            None => CommandResponse::Error {
                message: format!("unknown session {}", target.session),
            },
        }
    }

    fn kill_window(&mut self, target: TargetRef) -> CommandResponse {
        match self.sessions.get_mut(&target.session) {
            Some(session) => match target.window {
                Some(window_id) => match session.kill_window(window_id) {
                    Ok(still_alive) => {
                        if !still_alive {
                            self.sessions.remove(&target.session);
                        }
                        CommandResponse::WindowKilled {
                            session: target.session,
                            window_id: window_id.0,
                        }
                    }
                    Err(error) => CommandResponse::Error {
                        message: error.to_string(),
                    },
                },
                None => CommandResponse::Error {
                    message: "kill-window target requires a window id".into(),
                },
            },
            None => CommandResponse::Error {
                message: format!("unknown session {}", target.session),
            },
        }
    }

    fn rename_window(&mut self, target: TargetRef, name: String) -> CommandResponse {
        match self.sessions.get_mut(&target.session) {
            Some(session) => {
                if let Some(window_id) = target.window {
                    if let Err(error) = session.select_window(window_id) {
                        return CommandResponse::Error {
                            message: error.to_string(),
                        };
                    }
                }
                match session.rename_active_window(name) {
                    Ok(_) => CommandResponse::FocusChanged,
                    Err(error) => CommandResponse::Error {
                        message: error.to_string(),
                    },
                }
            }
            None => CommandResponse::Error {
                message: format!("unknown session {}", target.session),
            },
        }
    }
}

pub fn serve(socket_path: &Path) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create socket directory {}", parent.display()))?;
    }
    if socket_path.exists() {
        fs::remove_file(socket_path)
            .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind socket {}", socket_path.display()))?;
    let state = Arc::new(Mutex::new(SessionStore::default()));
    for stream in listener.incoming() {
        let mut stream = stream.context("failed to accept client")?;
        let response = {
            let request = read_request(&mut stream)?;
            let mut state = state.lock().expect("session store lock poisoned");
            state.handle(request)
        };
        write_response(&mut stream, &response)?;
    }

    bail!("listener stopped unexpectedly")
}

fn parse_target(target: &str) -> Result<TargetRef, String> {
    let (session, rest) = target
        .split_once(':')
        .map_or((target, None), |(session, rest)| (session, Some(rest)));
    if session.is_empty() {
        return Err("target requires a session name".into());
    }
    let (window, pane) = match rest {
        Some(rest) => {
            let (window, pane) = rest
                .split_once('.')
                .map_or((rest, None), |(window, pane)| (window, Some(pane)));
            let window = if window.is_empty() {
                None
            } else {
                Some(
                    window
                        .parse::<u64>()
                        .map(WindowId)
                        .map_err(|_| format!("invalid window id in target {target}"))?,
                )
            };
            let pane = match pane {
                Some(value) if !value.is_empty() => Some(
                    value
                        .parse::<u64>()
                        .map(PaneId)
                        .map_err(|_| format!("invalid pane id in target {target}"))?,
                ),
                _ => None,
            };
            (window, pane)
        }
        None => (None, None),
    };

    Ok(TargetRef {
        session: session.into(),
        window,
        pane,
    })
}

fn read_request(stream: &mut UnixStream) -> Result<CommandRequest> {
    let mut payload = Vec::new();
    stream
        .read_to_end(&mut payload)
        .context("failed to read request payload")?;
    let request = serde_json::from_slice(&payload).context("failed to decode request")?;
    Ok(request)
}

fn write_response(stream: &mut UnixStream, response: &CommandResponse) -> Result<()> {
    let payload = serde_json::to_vec(response).context("failed to encode response")?;
    stream
        .write_all(&payload)
        .context("failed to write response payload")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ipc::CommandRequest, layout::SplitAxis};

    #[test]
    fn store_creates_and_lists_sessions() {
        let mut store = SessionStore::default();
        let created = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into(), "-lc".into(), "printf test".into()],
        });
        assert!(matches!(
            created,
            CommandResponse::SessionCreated {
                session,
                pane_id: 1
            } if session == "work"
        ));

        let listed = store.handle(CommandRequest::ListSessions);
        assert_eq!(
            listed,
            CommandResponse::SessionList {
                sessions: vec!["work".into()]
            }
        );
    }

    #[test]
    fn split_command_creates_second_pane() {
        let mut store = SessionStore::default();
        let _ = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into()],
        });

        let response = store.handle(CommandRequest::SplitPane {
            target: "work".into(),
            axis: SplitAxis::Vertical,
            command: Vec::new(),
        });

        assert!(matches!(
            response,
            CommandResponse::PaneSplit {
                session,
                window_id: 1,
                pane_id: 2
            } if session == "work"
        ));
    }

    #[test]
    fn attach_defaults_to_last_session() {
        let mut store = SessionStore::default();
        let _ = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into(), "-lc".into(), "printf attached; sleep 1".into()],
        });
        std::thread::sleep(std::time::Duration::from_millis(100));

        let attached = store.handle(CommandRequest::Attach { session: None });

        assert!(matches!(
            attached,
            CommandResponse::Attached {
                session,
                preview,
                ..
            } if session == "work" && preview.contains("attached")
        ));
    }

    #[test]
    fn rename_window_updates_active_window_name() {
        let mut store = SessionStore::default();
        let _ = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into()],
        });

        let response = store.handle(CommandRequest::RenameWindow {
            target: "work:1".into(),
            name: "editor".into(),
        });

        assert_eq!(response, CommandResponse::FocusChanged);
        assert_eq!(
            store.handle(CommandRequest::ListWindows {
                session: "work".into(),
            }),
            CommandResponse::WindowList {
                windows: vec![crate::window::WindowSummary {
                    id: 1,
                    index: 0,
                    name: "editor".into(),
                    active: true,
                }]
            }
        );
    }
}
