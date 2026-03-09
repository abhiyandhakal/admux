use crate::{
    ipc::{CURRENT_PROTOCOL_VERSION, CommandRequest, CommandResponse, ProtocolVersion},
    pane::PaneId,
    session::Session,
};
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Starting,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub name: String,
    pub active_pane: PaneId,
}

#[derive(Default)]
pub struct SessionStore {
    sessions: BTreeMap<String, Session>,
    last_session: Option<String>,
    next_pane_id: u64,
}

impl SessionStore {
    pub fn handle(&mut self, request: CommandRequest) -> CommandResponse {
        match request {
            CommandRequest::Hello { version } => self.handle_hello(version),
            CommandRequest::NewSession { name, cwd, command } => {
                let name = name.unwrap_or_else(|| format!("session-{}", self.sessions.len() + 1));
                let pane_id = self.next_pane();
                match Session::new(name.clone(), cwd, command, pane_id) {
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
                let resolved = match session {
                    Some(session) if self.sessions.contains_key(&session) => Some(session),
                    Some(session) => {
                        return CommandResponse::Error {
                            message: format!("unknown session {session}"),
                        };
                    }
                    None => self.last_session.clone(),
                };

                match resolved {
                    Some(session) => {
                        let preview = self
                            .sessions
                            .get(&session)
                            .map(|session| session.active_pane_preview())
                            .unwrap_or_default();
                        let formatted_preview = self
                            .sessions
                            .get(&session)
                            .map(|session| session.active_pane_formatted_preview())
                            .unwrap_or_default();
                        CommandResponse::Attached {
                            session,
                            preview,
                            formatted_preview,
                        }
                    }
                    None => CommandResponse::Error {
                        message: "no sessions available".into(),
                    },
                }
            }
            CommandRequest::ListSessions => CommandResponse::SessionList {
                sessions: self.sessions.keys().cloned().collect(),
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
            CommandRequest::SendKeys { target, keys } => {
                let session_name = target.split(':').next().unwrap_or(target.as_str());
                match self.sessions.get(session_name) {
                    Some(session) => match session.send_keys(&keys) {
                        Ok(_) => CommandResponse::KeysSent,
                        Err(error) => CommandResponse::Error {
                            message: error.to_string(),
                        },
                    },
                    None => CommandResponse::Error {
                        message: format!("unknown session {session_name}"),
                    },
                }
            }
            CommandRequest::Resize {
                session,
                rows,
                cols,
            } => match self.sessions.get(&session) {
                Some(session) => match session.resize(rows, cols) {
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
    use crate::ipc::CommandRequest;

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
    fn attach_defaults_to_last_session() {
        let mut store = SessionStore::default();
        let _ = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into(), "-lc".into(), "printf attached".into()],
        });
        std::thread::sleep(std::time::Duration::from_millis(100));

        let attached = store.handle(CommandRequest::Attach { session: None });

        assert!(matches!(
            attached,
            CommandResponse::Attached {
                session,
                preview,
                formatted_preview: _
            } if session == "work" && preview.contains("attached")
        ));
    }
}
