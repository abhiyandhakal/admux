use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const IPC_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_IPC_MESSAGE_BYTES: u64 = 1024 * 1024;

use anyhow::{Context, Result, bail};

use crate::{
    buffer::BufferStore,
    config::{Config, ResolvedConfig},
    ipc::{
        BufferSummary, CURRENT_PROTOCOL_VERSION, CommandRequest, CommandResponse, CycleDirection,
        NavigationDirection, ProtocolVersion, SessionSummary,
    },
    numbering::Numbering,
    pane::{PaneId, WindowId},
    persistence::{PersistedSession, PersistedState, load_state, save_state},
    session::Session,
    workspace::{WorkspaceLoad, save_workspace},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Starting,
    Running,
}

pub struct SessionStore {
    buffers: BufferStore,
    sessions: BTreeMap<String, Session>,
    persisted_sessions: BTreeMap<String, PersistedSession>,
    state_path: Option<std::path::PathBuf>,
    helper_dir: PathBuf,
    pending_switches: BTreeMap<String, String>,
    workspace_mappings: BTreeMap<String, String>,
    last_session: Option<String>,
    next_window_id: u64,
    config_path: Option<std::path::PathBuf>,
    config: ResolvedConfig,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self {
            buffers: BufferStore::default(),
            sessions: BTreeMap::new(),
            persisted_sessions: BTreeMap::new(),
            state_path: None,
            helper_dir: PathBuf::from("/tmp/admux-helpers"),
            pending_switches: BTreeMap::new(),
            workspace_mappings: BTreeMap::new(),
            last_session: None,
            next_window_id: 0,
            config_path: None,
            config: Config::default()
                .resolve()
                .expect("default config should resolve"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetRef {
    session: String,
    window: Option<WindowId>,
    pane: Option<PaneId>,
}

fn request_changes_persisted_state(request: &CommandRequest) -> bool {
    matches!(
        request,
        CommandRequest::NewSession { .. }
            | CommandRequest::UpWorkspace { .. }
            | CommandRequest::SetBuffer { .. }
            | CommandRequest::DeleteBuffer { .. }
            | CommandRequest::LoadBuffer { .. }
            | CommandRequest::KillSession { .. }
            | CommandRequest::KillWindow { .. }
            | CommandRequest::KillPane { .. }
            | CommandRequest::SplitPane { .. }
            | CommandRequest::NewWindow { .. }
            | CommandRequest::SelectPane { .. }
            | CommandRequest::SelectWindow { .. }
            | CommandRequest::CycleWindow { .. }
            | CommandRequest::ResizePane { .. }
            | CommandRequest::RenameWindow { .. }
            | CommandRequest::Resize { .. }
    )
}

impl SessionStore {
    pub fn with_paths(
        state_path: std::path::PathBuf,
        config_path: std::path::PathBuf,
        helper_dir: PathBuf,
    ) -> Result<Self> {
        let persisted = load_state(&state_path)?;
        let next_window_id = persisted.next_window_id.max(
            persisted
                .sessions
                .values()
                .flat_map(|session| session.windows.keys())
                .map(|window_id| window_id.0)
                .max()
                .unwrap_or(0),
        );
        let mut store = Self {
            buffers: BufferStore::from_persisted(persisted.buffers),
            workspace_mappings: persisted.workspaces,
            persisted_sessions: persisted.sessions,
            state_path: Some(state_path),
            helper_dir,
            last_session: persisted.last_session,
            next_window_id,
            config_path: Some(config_path),
            ..Self::default()
        };
        store.reload_config()?;
        let persisted_sessions = store.persisted_sessions.clone();
        for (name, persisted) in persisted_sessions {
            if let Ok(session) = Session::from_persisted(
                &persisted,
                store.config.behavior.default_shell.clone(),
                store.config.behavior.scrollback_lines,
                store.numbering(),
                store.config.defaults.window.clone(),
                store.helper_dir.clone(),
            ) {
                store.sessions.insert(name, session);
            } else {
                store.persisted_sessions.remove(&name);
            }
        }
        Ok(store)
    }

    pub fn handle(&mut self, request: CommandRequest) -> CommandResponse {
        let persist_requested = request_changes_persisted_state(&request);
        let previous_last_session = self.last_session.clone();
        let pruned = self.prune_dead_sessions();

        let response = match request {
            CommandRequest::Hello { version } => self.handle_hello(version),
            CommandRequest::NewSession {
                name,
                cwd,
                command,
                switch_from,
            } => {
                let name = name.unwrap_or_else(|| self.next_session_name());
                if self.sessions.contains_key(&name) || self.persisted_sessions.contains_key(&name)
                {
                    CommandResponse::Error {
                        message: format!("session {name} already exists"),
                    }
                } else {
                let command = self.effective_command(command);
                let window_id = self.next_window();
                match Session::new(
                    name.clone(),
                    None,
                    cwd,
                    command,
                    window_id,
                    self.config.behavior.default_shell.clone(),
                    self.config.behavior.scrollback_lines,
                    self.numbering(),
                    self.config.defaults.window.clone(),
                    self.helper_dir.clone(),
                ) {
                    Ok(session) => {
                        self.sessions.insert(name.clone(), session);
                        self.last_session = Some(name.clone());
                        if let Some(source) = switch_from
                            && self.sessions.get(&source.session).is_some_and(|session| {
                                session.contains_window_pane(
                                    WindowId(source.window_id),
                                    PaneId(source.pane_id),
                                )
                            })
                        {
                            self.pending_switches.insert(source.session, name.clone());
                        }
                        CommandResponse::SessionCreated {
                            session: name,
                            pane_id: self.config.behavior.pane_base,
                        }
                    }
                    Err(error) => CommandResponse::Error {
                        message: error.to_string(),
                    },
                }
                }
            }
            CommandRequest::UpWorkspace {
                manifest_path,
                rebuild,
                switch_from,
            } => match crate::workspace::load_workspace_with_snapshot(
                &manifest_path,
                self.numbering(),
                !rebuild,
            ) {
                Ok(workspace) => self.up_workspace(workspace, rebuild, switch_from),
                Err(error) => CommandResponse::Error {
                    message: error.to_string(),
                },
            },
            CommandRequest::SaveWorkspace { session } => self.save_workspace(session),
            CommandRequest::Attach { session } => {
                let Some(session_name) = self.resolve_session(session) else {
                    return CommandResponse::Error {
                        message: "no sessions available".into(),
                    };
                };
                let session_name = self
                    .pending_switches
                    .remove(&session_name)
                    .filter(|target| self.sessions.contains_key(target))
                    .unwrap_or(session_name);
                if self.sessions.contains_key(&session_name) {
                    self.last_session = Some(session_name.clone());
                    let session = self.sessions.get(&session_name).expect("checked contains");
                    let snapshot =
                        session
                            .render_snapshot(session.pane_area())
                            .map(|mut snapshot| {
                                snapshot.sessions = self.list_session_summaries();
                                snapshot
                            });
                    CommandResponse::Attached {
                        session: session_name,
                        preview: session.active_pane_preview(),
                        formatted_preview: session.active_pane_formatted_preview(),
                        formatted_cursor: session.active_pane_formatted_cursor(),
                        snapshot,
                    }
                } else if self.persisted_sessions.contains_key(&session_name) {
                    CommandResponse::Error {
                        message: format!(
                            "session {session_name} only has persisted metadata; live pane processes cannot be recovered"
                        ),
                    }
                } else {
                    return CommandResponse::Error {
                        message: format!("unknown session {session_name}"),
                    };
                }
            }
            CommandRequest::PreviewSession { session } => match self.sessions.get(&session) {
                Some(runtime) => match runtime.render_session_preview(crate::pane::Rect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 24,
                }) {
                    Some(mut snapshot) => {
                        snapshot.sessions = self.list_session_summaries();
                        CommandResponse::SessionPreview { snapshot }
                    }
                    None => CommandResponse::Error {
                        message: format!("could not render session preview for {session}"),
                    },
                },
                None => CommandResponse::Error {
                    message: format!("unknown session {session}"),
                },
            },
            CommandRequest::ListSessions => CommandResponse::SessionList {
                sessions: self.list_session_summaries(),
            },
            CommandRequest::ListBuffers => CommandResponse::BufferList {
                buffers: self.list_buffer_summaries(),
            },
            CommandRequest::ShowBuffer { buffer } => match self.buffers.get(buffer.as_deref()) {
                Some(buffer) => CommandResponse::BufferShown {
                    name: buffer.name.clone(),
                    data: buffer.data.clone(),
                },
                None => CommandResponse::Error {
                    message: "no matching paste buffer".into(),
                },
            },
            CommandRequest::SetBuffer {
                buffer,
                data,
                append,
            } => {
                let name = self.buffers.set(buffer, data, append).name.clone();
                CommandResponse::BufferSet { name }
            }
            CommandRequest::DeleteBuffer { buffer } => match self.buffers.delete(buffer.as_deref())
            {
                Some(buffer) => CommandResponse::BufferDeleted { name: buffer.name },
                None => CommandResponse::Error {
                    message: "no matching paste buffer".into(),
                },
            },
            CommandRequest::PasteBuffer { target, buffer } => {
                match self.buffers.get(buffer.as_deref()) {
                    Some(buffer) => match self.parse_target(&target) {
                        Ok(target) => match self.sessions.get(&target.session) {
                            Some(session) => match session.send_keys(
                                target.window,
                                target.pane,
                                &[buffer.data.clone()],
                            ) {
                                Ok(_) => CommandResponse::BufferPasted {
                                    name: buffer.name.clone(),
                                },
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
                    None => CommandResponse::Error {
                        message: "no matching paste buffer".into(),
                    },
                }
            }
            CommandRequest::SaveBuffer { buffer, path } => {
                match self.buffers.get(buffer.as_deref()) {
                    Some(buffer) => match std::fs::write(&path, &buffer.data) {
                        Ok(_) => CommandResponse::BufferSaved {
                            name: buffer.name.clone(),
                            path,
                        },
                        Err(error) => CommandResponse::Error {
                            message: format!("failed to save buffer: {error}"),
                        },
                    },
                    None => CommandResponse::Error {
                        message: "no matching paste buffer".into(),
                    },
                }
            }
            CommandRequest::LoadBuffer { path, buffer } => match std::fs::read_to_string(&path) {
                Ok(data) => {
                    let name = self.buffers.set(buffer, data, false).name.clone();
                    CommandResponse::BufferLoaded { name }
                }
                Err(error) => CommandResponse::Error {
                    message: format!("failed to load buffer: {error}"),
                },
            },
            CommandRequest::ListWindows { session } => {
                if let Some(session) = self.sessions.get(&session) {
                    CommandResponse::WindowList {
                        windows: session.list_windows(),
                    }
                } else if let Some(session) = self.persisted_sessions.get(&session) {
                    CommandResponse::WindowList {
                        windows: session
                            .window_order
                            .iter()
                            .enumerate()
                            .filter_map(|(index, id)| {
                                let window = session.windows.get(id)?;
                                Some(crate::window::WindowSummary::new(
                                    *id,
                                    self.numbering()
                                        .public_window_number(index)
                                        .unwrap_or(index as u64),
                                    window.name.clone(),
                                    *id == session.active_window,
                                    Some(*id) == session.last_window,
                                ))
                            })
                            .collect(),
                    }
                } else {
                    CommandResponse::Error {
                        message: format!("unknown session {session}"),
                    }
                }
            }
            CommandRequest::ListPanes { target } => match self.parse_target(&target) {
                Ok(target) => match self.sessions.get(&target.session) {
                    Some(session) => CommandResponse::PaneList {
                        panes: session.list_panes(target.window),
                    },
                    None => match self.persisted_sessions.get(&target.session) {
                        Some(session) => {
                            let window_id = target.window.unwrap_or(session.active_window);
                            let panes = session
                                .windows
                                .get(&window_id)
                                .map(|window| {
                                    window
                                        .layout
                                        .panes()
                                        .into_iter()
                                        .filter_map(|pane_id| {
                                            let pane = window.panes.get(&pane_id)?;
                                            Some(crate::ipc::PaneSummary {
                                                id: self
                                                    .numbering()
                                                    .public_pane_number(pane.id)
                                                    .unwrap_or(pane.id.0),
                                                title: pane.title.clone(),
                                                active: pane_id == window.layout.active,
                                                window_id: self
                                                    .numbering()
                                                    .public_window_id(
                                                        window.id,
                                                        &session.window_order,
                                                    )
                                                    .unwrap_or(window.id.0),
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            CommandResponse::PaneList { panes }
                        }
                        None => CommandResponse::Error {
                            message: format!("unknown session {}", target.session),
                        },
                    },
                },
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::KillSession { session } => {
                if let Some(removed) = self.sessions.remove(&session) {
                    let _ = removed.kill();
                    self.persisted_sessions.remove(&session);
                    if self.last_session.as_deref() == Some(session.as_str()) {
                        self.last_session = self.sessions.keys().next_back().cloned();
                    }
                    CommandResponse::SessionKilled { session }
                } else if self.persisted_sessions.remove(&session).is_some() {
                    CommandResponse::SessionKilled { session }
                } else {
                    CommandResponse::Error {
                        message: format!("unknown session {session}"),
                    }
                }
            }
            CommandRequest::KillWindow { target } => match self.parse_target(&target) {
                Ok(target) => self.kill_window(target),
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::KillPane { target } => match self.parse_target(&target) {
                Ok(target) => self.kill_pane(target),
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::SendKeys { target, keys } => match self.parse_target(&target) {
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
            } => match self.parse_target(&target) {
                Ok(target) => self.split_pane(target, axis, command),
                Err(message) => CommandResponse::Error { message },
            },
            CommandRequest::NewWindow {
                session,
                name,
                command,
            } => {
                if !self.sessions.contains_key(&session) {
                    CommandResponse::Error {
                        message: format!("unknown session {session}"),
                    }
                } else {
                    let window_id = self.next_window();
                    let command = self.effective_command(command);
                    match self.sessions.get_mut(&session) {
                    Some(session) => match session.new_window(window_id, name, &command) {
                        Ok(created) => CommandResponse::WindowCreated {
                            session: session.name.clone(),
                            window_id: session
                                .numbering
                                .public_window_id(created.window_id, &session.window_order)
                                .unwrap_or(created.window_id.0),
                            pane_id: session
                                .numbering
                                .public_pane_number(created.pane_id)
                                .unwrap_or(created.pane_id.0),
                        },
                        Err(error) => CommandResponse::Error {
                            message: error.to_string(),
                        },
                    },
                    None => unreachable!("session existence checked before allocation"),
                    }
                }
            }
            CommandRequest::SelectPane { target, direction } => self.select_pane(target, direction),
            CommandRequest::SelectWindow { target } => match self.parse_target(&target) {
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
            } => match self.parse_target(&target) {
                Ok(target) => match self.sessions.get_mut(&target.session) {
                    Some(session) => {
                        match session.resize_active_pane(
                            target.window,
                            target.pane,
                            direction,
                            amount,
                        ) {
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
            CommandRequest::RenameWindow { target, name } => match self.parse_target(&target) {
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
            CommandRequest::MousePane {
                session,
                pane_id,
                row,
                col,
                kind,
            } => match self.sessions.get(&session) {
                Some(session) => {
                    match session.handle_pane_mouse(Some(PaneId(pane_id)), kind, row, col) {
                        Ok(_) => CommandResponse::FocusChanged,
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
            CommandRequest::ScrollPane {
                session,
                pane_id,
                lines,
            } => match self.sessions.get(&session) {
                Some(session) => match session.scroll_pane(pane_id.map(PaneId), lines) {
                    Ok(_) => CommandResponse::Scrolled,
                    Err(error) => CommandResponse::Error {
                        message: error.to_string(),
                    },
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
            CommandRequest::ReloadConfig => match self.reload_config() {
                Ok(()) => CommandResponse::ConfigReloaded,
                Err(error) => CommandResponse::Error {
                    message: error.to_string(),
                },
            },
        };
        if !(persist_requested || pruned || self.last_session != previous_last_session) {
            return response;
        }
        match self.persist_metadata() {
            Ok(()) => response,
            Err(error) => match response {
                CommandResponse::Error { message } => CommandResponse::Error {
                    message: format!("{message}; additionally failed to persist state: {error}"),
                },
                _ => CommandResponse::Error {
                    message: format!("command completed in memory but failed to persist state: {error}"),
                },
            },
        }
    }

    fn next_window(&mut self) -> WindowId {
        self.next_window_id += 1;
        WindowId(self.next_window_id)
    }

    fn next_session_name(&self) -> String {
        let prefix = &self.config.defaults.session.name_prefix;
        let mut suffix = 1_u64;
        loop {
            let candidate = format!("{prefix}-{suffix}");
            if !self.sessions.contains_key(&candidate)
                && !self.persisted_sessions.contains_key(&candidate)
            {
                return candidate;
            }
            suffix = suffix.saturating_add(1);
        }
    }

    fn prune_dead_sessions(&mut self) -> bool {
        let mut changed = false;
        let dead_sessions: Vec<_> = self
            .sessions
            .iter_mut()
            .filter_map(|(name, session)| (!session.prune_dead()).then_some(name.clone()))
            .collect();
        for session in dead_sessions {
            self.sessions.remove(&session);
            self.persisted_sessions.remove(&session);
            changed = true;
        }
        let pending_switches = self.pending_switches.len();
        self.pending_switches.retain(|source, target| {
            self.sessions.contains_key(source) && self.sessions.contains_key(target)
        });
        changed |= self.pending_switches.len() != pending_switches;
        if let Some(last) = self.last_session.as_ref()
            && !self.sessions.contains_key(last)
        {
            self.last_session = self.sessions.keys().next_back().cloned();
            changed = true;
        }
        changed
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
            Some(session)
                if self.sessions.contains_key(&session)
                    || self.persisted_sessions.contains_key(&session) =>
            {
                Some(session)
            }
            Some(_) => None,
            None => self
                .last_session
                .clone()
                .or_else(|| self.persisted_sessions.keys().next_back().cloned()),
        }
    }

    fn list_session_summaries(&self) -> Vec<SessionSummary> {
        let mut sessions = BTreeMap::<String, bool>::new();
        for name in self.persisted_sessions.keys() {
            sessions.insert(name.clone(), true);
        }
        for name in self.sessions.keys() {
            sessions.insert(name.clone(), false);
        }
        sessions
            .into_iter()
            .map(|(name, stale)| SessionSummary { name, stale })
            .collect()
    }

    fn list_buffer_summaries(&self) -> Vec<BufferSummary> {
        self.buffers
            .summaries()
            .into_iter()
            .map(|(name, bytes, preview)| BufferSummary {
                name,
                bytes,
                preview,
            })
            .collect()
    }

    fn persist_metadata(&mut self) -> Result<()> {
        for (name, session) in &self.sessions {
            self.persisted_sessions
                .insert(name.clone(), PersistedSession::from_live(session));
        }
        let Some(path) = self.state_path.as_ref() else {
            return Ok(());
        };
        let state = PersistedState {
            schema_version: crate::persistence::STATE_SCHEMA_VERSION,
            last_session: self.last_session.clone(),
            next_window_id: self.next_window_id,
            buffers: self.buffers.snapshot(),
            workspaces: self.workspace_mappings.clone(),
            sessions: self.persisted_sessions.clone(),
        };
        save_state(path, &state)
    }

    fn split_pane(
        &mut self,
        target: TargetRef,
        axis: crate::layout::SplitAxis,
        command: Vec<String>,
    ) -> CommandResponse {
        let command = self.effective_command(command);
        match self.sessions.get_mut(&target.session) {
            Some(session) => {
                let window_id = target.window.unwrap_or(session.active_window);
                match session.split_pane_in_window(
                    window_id,
                    target.pane,
                    axis,
                    500,
                    None,
                    &command,
                ) {
                    Ok(split) => {
                        // A successful split becomes the active pane in its
                        // target window; only now make that window visible.
                        session.active_window = split.window_id;
                        CommandResponse::PaneSplit {
                            session: session.name.clone(),
                            window_id: session
                                .numbering
                                .public_window_id(split.window_id, &session.window_order)
                                .unwrap_or(split.window_id.0),
                            pane_id: session
                                .numbering
                                .public_pane_number(split.pane_id)
                                .unwrap_or(split.pane_id.0),
                        }
                    }
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
            (Some(target), Some(direction)) => match self.parse_target(&target) {
                Ok(target) => match self.sessions.get_mut(&target.session) {
                    Some(session) => match session.move_focus(direction, session.pane_area()) {
                        Ok(()) => CommandResponse::FocusChanged,
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
            (Some(target), None) => match self.parse_target(&target) {
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
                let _ = direction;
                CommandResponse::Error {
                    message: "directional select-pane requires a session target".into(),
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
            Some(session) => {
                let window_id = target.window.unwrap_or(session.active_window);
                match session.kill_pane(target.window, target.pane) {
                    Ok(Some(killed)) => CommandResponse::PaneKilled {
                        session: session.name.clone(),
                        window_id: session
                            .numbering
                            .public_window_id(killed.window_id, &session.window_order)
                            .unwrap_or(killed.window_id.0),
                        pane_id: session
                            .numbering
                            .public_pane_number(killed.pane_id)
                            .unwrap_or(killed.pane_id.0),
                    },
                    Ok(None) => {
                        if !session.is_alive() {
                            self.sessions.remove(&target.session);
                            self.persisted_sessions.remove(&target.session);
                            CommandResponse::SessionKilled {
                                session: target.session,
                            }
                        } else {
                            CommandResponse::WindowKilled {
                                session: session.name.clone(),
                                window_id: session
                                    .numbering
                                    .public_window_id(window_id, &session.window_order)
                                    .unwrap_or(window_id.0),
                            }
                        }
                    }
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

    fn kill_window(&mut self, target: TargetRef) -> CommandResponse {
        match self.sessions.get_mut(&target.session) {
            Some(session) => match target.window {
                Some(window_id) => match session.kill_window(window_id) {
                    Ok(still_alive) => {
                        if !still_alive {
                            self.sessions.remove(&target.session);
                            self.persisted_sessions.remove(&target.session);
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

    fn up_workspace(
        &mut self,
        workspace: WorkspaceLoad,
        rebuild: bool,
        switch_from: Option<crate::ipc::SwitchSource>,
    ) -> CommandResponse {
        let manifest_key = workspace.manifest_key.clone();
        if rebuild && let Some(existing) = self.workspace_mappings.get(&manifest_key).cloned() {
            if let Some(session) = self.sessions.remove(&existing) {
                let _ = session.kill();
            }
            self.persisted_sessions.remove(&existing);
        }

        if let Some(existing) = self.workspace_mappings.get(&manifest_key).cloned()
            && self.sessions.get(&existing).is_some_and(|session| {
                session.workspace_manifest.as_deref() == Some(manifest_key.as_str())
            })
        {
            self.last_session = Some(existing.clone());
            return CommandResponse::WorkspaceReady {
                session: existing,
                created: false,
            };
        }

        let session_name = workspace.spec.name.clone();
        if self.sessions.contains_key(&session_name)
            || self.persisted_sessions.contains_key(&session_name)
        {
            return CommandResponse::Error {
                message: format!(
                    "session {session_name} already exists; set [workspace].name to a unique value or use --rebuild"
                ),
            };
        }

        match self.create_workspace_session(&workspace, !rebuild) {
            Ok(()) => {
                self.workspace_mappings
                    .insert(manifest_key.clone(), session_name.clone());
                self.last_session = Some(session_name.clone());
                if let Some(source) = switch_from
                    && self.sessions.get(&source.session).is_some_and(|session| {
                        session.contains_window_pane(
                            WindowId(source.window_id),
                            PaneId(source.pane_id),
                        )
                    })
                {
                    self.pending_switches
                        .insert(source.session, session_name.clone());
                }
                CommandResponse::WorkspaceReady {
                    session: session_name,
                    created: true,
                }
            }
            Err(error) => CommandResponse::Error {
                message: error.to_string(),
            },
        }
    }

    fn create_workspace_session(
        &mut self,
        workspace: &WorkspaceLoad,
        use_snapshot: bool,
    ) -> Result<()> {
        let first_window_id = self.next_window();
        let first_window =
            workspace.spec.windows.first().ok_or_else(|| {
                anyhow::anyhow!("workspace manifest must define at least one window")
            })?;
        let root_seed = use_snapshot
            .then(|| {
                let first_window_public = self.numbering().public_window_number(0).ok()?;
                workspace
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| {
                        snapshot.pane(first_window_public as usize, self.config.behavior.pane_base)
                    })
                    .map(|pane| crate::pty::PaneRestoreSeed {
                        rows: pane.rows,
                        cols: pane.cols,
                        vt: pane.vt.clone(),
                    })
            })
            .flatten();
        let mut session = Session::new_with_restore(
            workspace.spec.name.clone(),
            Some(workspace.manifest_key.clone()),
            Some(first_window.root.cwd.clone()),
            first_window.root.command.clone(),
            first_window_id,
            self.config.behavior.default_shell.clone(),
            self.config.behavior.scrollback_lines,
            self.numbering(),
            self.config.defaults.window.clone(),
            self.helper_dir.clone(),
            root_seed,
        )?;
        session.cwd = Some(workspace.spec.cwd.clone());
        session.rename_active_window(first_window.name.clone())?;
        self.apply_workspace_window(&mut session, first_window_id, first_window, workspace, 0)?;

        for (window_index, window) in workspace.spec.windows.iter().enumerate().skip(1) {
            let window_id = self.next_window();
            let root_seed = use_snapshot
                .then(|| {
                    let window_public = self.numbering().public_window_number(window_index).ok()?;
                    workspace
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| {
                            snapshot.pane(window_public as usize, self.config.behavior.pane_base)
                        })
                        .map(|pane| crate::pty::PaneRestoreSeed {
                            rows: pane.rows,
                            cols: pane.cols,
                            vt: pane.vt.clone(),
                        })
                })
                .flatten();
            session.new_window_with_cwd_and_restore(
                window_id,
                Some(window.name.clone()),
                Some(window.cwd.clone()),
                &window.root.command,
                root_seed,
            )?;
            session.select_window(window_id)?;
            self.apply_workspace_window(&mut session, window_id, window, workspace, window_index)?;
        }

        if let Some(window_id) = session
            .window_order
            .get(workspace.spec.active_window)
            .copied()
        {
            session.select_window(window_id)?;
        }

        self.sessions.insert(session.name.clone(), session);
        Ok(())
    }

    fn apply_workspace_window(
        &self,
        session: &mut Session,
        window_id: WindowId,
        window: &crate::workspace::WorkspaceWindowSpec,
        workspace: &WorkspaceLoad,
        window_index: usize,
    ) -> Result<()> {
        session.select_window(window_id)?;
        for (split_index, split) in window.splits.iter().enumerate() {
            let window_public = self.numbering().public_window_number(window_index).ok();
            let pane_public = self
                .numbering()
                .public_pane_number(PaneId((split_index + 1) as u64))
                .ok();
            let restore_seed = workspace
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot.pane(window_public? as usize, pane_public?)
                })
                .map(|pane| crate::pty::PaneRestoreSeed {
                    rows: pane.rows,
                    cols: pane.cols,
                    vt: pane.vt.clone(),
                });
            session.split_pane_in_window_with_restore(
                window_id,
                Some(PaneId(split.target)),
                split.direction,
                split.ratio,
                Some(split.pane.cwd.clone()),
                &split.pane.command,
                restore_seed,
            )?;
        }
        let active_pane = workspace
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                self.numbering()
                    .public_window_number(window_index)
                    .ok()
                    .and_then(|window_public| snapshot.active_pane(window_public as usize))
            })
            .map(|public| self.numbering().parse_public_pane_number(public))
            .transpose()?
            .map(|pane| pane)
            .unwrap_or(PaneId(window.active_pane));
        session.select_pane(Some(window_id), active_pane)?;
        Ok(())
    }

    fn save_workspace(&self, session: Option<String>) -> CommandResponse {
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
        match save_workspace(session, self.config.behavior.workspace_snapshot_lines) {
            Ok(path) => CommandResponse::WorkspaceSaved {
                session: session_name,
                path,
            },
            Err(error) => CommandResponse::Error {
                message: error.to_string(),
            },
        }
    }

    fn numbering(&self) -> Numbering {
        Numbering {
            window_base: self.config.behavior.window_base,
            pane_base: self.config.behavior.pane_base,
        }
    }

    fn parse_target(&self, target: &str) -> Result<TargetRef, String> {
        let (session, rest) = target
            .split_once(':')
            .map_or((target, None), |(session, rest)| (session, Some(rest)));
        if session.is_empty() {
            return Err("target requires a session name".into());
        }

        let numbering = self.numbering();
        let empty_order: Vec<WindowId> = Vec::new();
        let window_order = if let Some(runtime) = self.sessions.get(session) {
            runtime.window_order.as_slice()
        } else if let Some(persisted) = self.persisted_sessions.get(session) {
            persisted.window_order.as_slice()
        } else {
            empty_order.as_slice()
        };

        let (window, pane) = match rest {
            Some(rest) => {
                let (window, pane) = rest
                    .split_once('.')
                    .map_or((rest, None), |(window, pane)| (window, Some(pane)));
                let window = if window.is_empty() {
                    None
                } else {
                    let public = window
                        .parse::<u64>()
                        .map_err(|_| format!("invalid window id in target {target}"))?;
                    Some(
                        numbering
                            .parse_public_window_id(public, window_order)
                            .map_err(|error| error.to_string())?,
                    )
                };
                let pane = match pane {
                    Some(value) if !value.is_empty() => {
                        let public = value
                            .parse::<u64>()
                            .map_err(|_| format!("invalid pane id in target {target}"))?;
                        Some(
                            numbering
                                .parse_public_pane_number(public)
                                .map_err(|error| error.to_string())?,
                        )
                    }
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

    fn effective_command(&self, command: Vec<String>) -> Vec<String> {
        if command.is_empty() {
            self.config
                .behavior
                .default_shell
                .as_ref()
                .cloned()
                .or_else(|| std::env::var("SHELL").ok())
                .map(|shell| vec![shell])
                .unwrap_or_else(|| vec!["/bin/sh".into()])
        } else {
            command
        }
    }

    fn reload_config(&mut self) -> Result<()> {
        let Some(path) = self.config_path.as_ref() else {
            self.config = Config::default().resolve()?;
            return Ok(());
        };
        self.config = if path.exists() {
            Config::load_from_path(path)?.resolve()?
        } else {
            Config::default().resolve()?
        };
        Ok(())
    }
}

pub fn serve(socket_path: &Path, state_path: &Path, config_path: &Path) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create socket directory {}", parent.display()))?;
    }
    let helper_dir = socket_path
        .parent()
        .map(|parent| parent.join("panes"))
        .unwrap_or_else(|| PathBuf::from("/tmp/admux-panes"));
    fs::create_dir_all(&helper_dir)
        .with_context(|| format!("failed to create helper directory {}", helper_dir.display()))?;
    if socket_path.exists() {
        fs::remove_file(socket_path)
            .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind socket {}", socket_path.display()))?;
    let state = Arc::new(Mutex::new(SessionStore::with_paths(
        state_path.to_path_buf(),
        config_path.to_path_buf(),
        helper_dir,
    )?));
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("admuxd: failed to accept client: {error:#}");
                thread::sleep(Duration::from_millis(25));
                continue;
            }
        };
        let state = Arc::clone(&state);
        thread::spawn(move || handle_client(stream, state));
    }

    bail!("listener stopped unexpectedly")
}

fn handle_client(mut stream: UnixStream, state: Arc<Mutex<SessionStore>>) {
        if let Err(error) = configure_ipc_stream(&stream) {
            eprintln!("admuxd: failed to configure client stream: {error:#}");
            return;
        }
        let response = {
            let request = match read_request(&mut stream) {
                Ok(request) => request,
                Err(error) => {
                    eprintln!("admuxd: rejected client request: {error:#}");
                    return;
                }
            };
            let mut state = state.lock().expect("session store lock poisoned");
            state.handle(request)
        };
        if let Err(error) = write_response(&mut stream, &response) {
            eprintln!("admuxd: failed to write client response: {error:#}");
        }
}

fn read_request(stream: &mut UnixStream) -> Result<CommandRequest> {
    let payload = read_limited(stream, MAX_IPC_MESSAGE_BYTES, "request")?;
    let request = serde_json::from_slice(&payload).context("failed to decode request")?;
    Ok(request)
}

fn configure_ipc_stream(stream: &UnixStream) -> Result<()> {
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    Ok(())
}

fn read_limited(stream: &mut UnixStream, limit: u64, kind: &str) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    (&mut *stream)
        .take(limit + 1)
        .read_to_end(&mut payload)
        .with_context(|| format!("failed to read {kind} payload"))?;
    if payload.len() as u64 > limit {
        bail!("{kind} payload exceeds {limit} byte limit");
    }
    Ok(payload)
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
    use crate::{
        ipc::{CommandRequest, SwitchSource},
        layout::SplitAxis,
    };
    use tempfile::{TempDir, tempdir as make_tempdir};

    fn tempdir() -> TempDir {
        make_tempdir().expect("tempdir")
    }

    #[test]
    fn store_creates_and_lists_sessions() {
        let mut store = SessionStore::default();
        let created = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into()],
            switch_from: None,
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
                sessions: vec![SessionSummary {
                    name: "work".into(),
                    stale: false,
                }]
            }
        );
    }

    #[test]
    fn duplicate_session_creation_preserves_the_existing_session() {
        let mut store = SessionStore::default();
        let request = |command| CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command,
            switch_from: None,
        };

        assert!(matches!(
            store.handle(request(vec!["sh".into()])),
            CommandResponse::SessionCreated { .. }
        ));
        assert!(matches!(
            store.handle(request(vec!["definitely-not-a-command".into()])),
            CommandResponse::Error { ref message } if message == "session work already exists"
        ));
        assert!(store.sessions.contains_key("work"));
    }

    #[test]
    fn automatic_session_names_skip_existing_names() {
        let mut store = SessionStore::default();
        for name in ["session-2", "session-1"] {
            assert!(matches!(
                store.handle(CommandRequest::NewSession {
                    name: Some(name.into()),
                    cwd: None,
                    command: vec!["sh".into()],
                    switch_from: None,
                }),
                CommandResponse::SessionCreated { .. }
            ));
        }
        assert!(matches!(
            store.handle(CommandRequest::NewSession {
                name: None,
                cwd: None,
                command: vec!["sh".into()],
                switch_from: None,
            }),
            CommandResponse::SessionCreated { ref session, .. } if session == "session-3"
        ));
    }

    #[test]
    fn failed_window_creation_does_not_consume_a_window_id() {
        let mut store = SessionStore::default();
        assert_eq!(store.next_window_id, 0);

        assert!(matches!(
            store.handle(CommandRequest::NewWindow {
                session: "missing".into(),
                name: None,
                command: vec!["sh".into()],
            }),
            CommandResponse::Error { .. }
        ));

        assert_eq!(store.next_window_id, 0);
    }

    #[test]
    fn invalid_split_target_does_not_change_focus_or_create_a_pane() {
        let mut store = SessionStore::default();
        assert!(matches!(
            store.handle(CommandRequest::NewSession {
                name: Some("work".into()),
                cwd: None,
                command: vec!["sh".into()],
                switch_from: None,
            }),
            CommandResponse::SessionCreated { .. }
        ));
        let before = store.sessions.get("work").expect("session");
        let active_window = before.active_window;
        let pane_count = before.windows[&active_window].panes.len();

        assert!(matches!(
            store.handle(CommandRequest::SplitPane {
                target: "work:1.99".into(),
                axis: SplitAxis::Vertical,
                command: vec!["sh".into()],
            }),
            CommandResponse::Error { .. }
        ));

        let after = store.sessions.get("work").expect("session");
        assert_eq!(after.active_window, active_window);
        assert_eq!(after.windows[&active_window].panes.len(), pane_count);
    }

    #[test]
    fn buffer_store_roundtrips_through_server_requests() {
        let mut store = SessionStore::default();

        assert_eq!(
            store.handle(CommandRequest::SetBuffer {
                buffer: None,
                data: "alpha".into(),
                append: false,
            }),
            CommandResponse::BufferSet {
                name: "buffer0001".into(),
            }
        );
        assert_eq!(
            store.handle(CommandRequest::ListBuffers),
            CommandResponse::BufferList {
                buffers: vec![BufferSummary {
                    name: "buffer0001".into(),
                    bytes: 5,
                    preview: "alpha".into(),
                }],
            }
        );
        assert_eq!(
            store.handle(CommandRequest::ShowBuffer { buffer: None }),
            CommandResponse::BufferShown {
                name: "buffer0001".into(),
                data: "alpha".into(),
            }
        );
        assert_eq!(
            store.handle(CommandRequest::DeleteBuffer { buffer: None }),
            CommandResponse::BufferDeleted {
                name: "buffer0001".into(),
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
            switch_from: None,
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
            switch_from: None,
        });
        let mut attached = None;
        for _ in 0..50 {
            let response = store.handle(CommandRequest::Attach { session: None });
            if matches!(
                response,
                CommandResponse::Attached { ref preview, .. } if preview.contains("attached")
            ) {
                attached = Some(response);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let attached = attached.expect("attach should eventually expose pane output");

        assert!(matches!(
            attached,
            CommandResponse::Attached {
                session,
                preview,
                ..
            } if session == "work" && preview.contains("attached")
        ));
        store
            .sessions
            .remove("work")
            .expect("session")
            .kill()
            .expect("clean up session");
    }

    #[test]
    fn killing_the_last_pane_reports_that_the_session_was_killed() {
        let mut store = SessionStore::default();
        let _ = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into()],
            switch_from: None,
        });

        assert_eq!(
            store.handle(CommandRequest::KillPane {
                target: "work".into(),
            }),
            CommandResponse::SessionKilled {
                session: "work".into(),
            }
        );
        assert!(!store.sessions.contains_key("work"));
    }

    #[test]
    fn persistence_failures_are_reported_to_the_caller() {
        let dir = tempdir();
        let blocked_parent = dir.path().join("not-a-directory");
        fs::write(&blocked_parent, "file").expect("create blocked parent");
        let mut store = SessionStore::default();
        store.state_path = Some(blocked_parent.join("state.json"));

        let response = store.handle(CommandRequest::SetBuffer {
            buffer: None,
            data: "hello".into(),
            append: false,
        });

        assert!(matches!(
            response,
            CommandResponse::Error { message }
                if message.contains("completed in memory but failed to persist state")
        ));
        assert_eq!(
            store.buffers.get(None).map(|buffer| buffer.data.clone()),
            Some("hello".into())
        );
    }

    #[test]
    fn read_only_requests_do_not_write_state() {
        let dir = tempdir();
        let blocked_parent = dir.path().join("not-a-directory");
        fs::write(&blocked_parent, "file").expect("create blocked parent");
        let mut store = SessionStore::default();
        store.state_path = Some(blocked_parent.join("state.json"));

        assert!(matches!(
            store.handle(CommandRequest::ListSessions),
            CommandResponse::SessionList { sessions } if sessions.is_empty()
        ));
    }

    #[test]
    fn rename_window_updates_active_window_name() {
        let mut store = SessionStore::default();
        let _ = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into()],
            switch_from: None,
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
                    index: 1,
                    name: "editor".into(),
                    active: true,
                    last_selected: false,
                }]
            }
        );
    }

    #[test]
    fn nested_new_session_redirects_next_attach() {
        let mut store = SessionStore::default();
        let created = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into()],
            switch_from: None,
        });
        match created {
            CommandResponse::SessionCreated { .. } => {}
            other => panic!("unexpected response: {other:?}"),
        }

        let response = store.handle(CommandRequest::NewSession {
            name: Some("logs".into()),
            cwd: None,
            command: vec!["sh".into()],
            switch_from: Some(SwitchSource {
                session: "work".into(),
                window_id: 1,
                pane_id: 0,
            }),
        });
        assert!(matches!(
            response,
            CommandResponse::SessionCreated { session, .. } if session == "logs"
        ));

        let attached = store.handle(CommandRequest::Attach {
            session: Some("work".into()),
        });
        assert!(matches!(
            attached,
            CommandResponse::Attached { session, .. } if session == "logs"
        ));
    }

    #[test]
    fn persisted_metadata_survives_store_restart() {
        let dir = tempdir();
        let state_path = dir.path().join("state.json");
        let config_path = dir.path().join("config.toml");

        let mut store = SessionStore::with_paths(
            state_path.clone(),
            config_path.clone(),
            dir.path().join("panes"),
        )
        .expect("create persisted store");
        let _ = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into()],
            switch_from: None,
        });
        let _ = store.handle(CommandRequest::SetBuffer {
            buffer: None,
            data: "hello".into(),
            append: false,
        });
        drop(store);

        let mut restarted =
            SessionStore::with_paths(state_path, config_path, dir.path().join("panes"))
                .expect("reload persisted store");
        assert_eq!(
            restarted.handle(CommandRequest::ListSessions),
            CommandResponse::SessionList {
                sessions: vec![SessionSummary {
                    name: "work".into(),
                    stale: false,
                }],
            }
        );
        assert!(matches!(
            restarted.handle(CommandRequest::ListWindows {
                session: "work".into(),
            }),
            CommandResponse::WindowList { windows } if windows.len() == 1
        ));
        assert!(matches!(
            restarted.handle(CommandRequest::Attach {
                session: Some("work".into()),
            }),
            CommandResponse::Attached { session, .. } if session == "work"
        ));
        assert_eq!(
            restarted.handle(CommandRequest::ShowBuffer { buffer: None }),
            CommandResponse::BufferShown {
                name: "buffer0001".into(),
                data: "hello".into(),
            }
        );
    }

    #[test]
    fn unrecoverable_persisted_sessions_are_pruned_on_startup() {
        let dir = tempdir();
        let state_path = dir.path().join("state.json");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &state_path,
            serde_json::to_vec_pretty(&PersistedState {
                schema_version: crate::persistence::STATE_SCHEMA_VERSION,
                last_session: Some("ghost".into()),
                next_window_id: 1,
                buffers: Vec::new(),
                workspaces: BTreeMap::new(),
                sessions: [(
                    "ghost".into(),
                    PersistedSession {
                        name: "ghost".into(),
                        workspace_manifest: None,
                        cwd: None,
                        command: vec!["sh".into()],
                        rows: 24,
                        cols: 80,
                        window_order: vec![WindowId(1)],
                        active_window: WindowId(1),
                        last_window: None,
                        windows: [(
                            WindowId(1),
                            crate::persistence::PersistedWindow {
                                id: WindowId(1),
                                name: "shell".into(),
                                cwd: None,
                                layout: crate::layout::LayoutTree::new(PaneId(0)),
                                next_pane_id: 1,
                                panes: [(
                                    PaneId(0),
                                    crate::persistence::PersistedPane {
                                        id: PaneId(0),
                                        title: "shell".into(),
                                        cwd: None,
                                        command: vec!["sh".into()],
                                        socket_path: Some(dir.path().join("missing-helper.sock")),
                                    },
                                )]
                                .into_iter()
                                .collect(),
                            },
                        )]
                        .into_iter()
                        .collect(),
                    },
                )]
                .into_iter()
                .collect(),
            })
            .expect("encode state"),
        )
        .expect("write state");

        let mut store = SessionStore::with_paths(state_path, config_path, dir.path().join("panes"))
            .expect("start store");

        assert_eq!(
            store.handle(CommandRequest::ListSessions),
            CommandResponse::SessionList { sessions: vec![] }
        );
    }

    #[test]
    fn reload_config_updates_future_creation_defaults() {
        let dir = tempdir();
        let state_path = dir.path().join("state.json");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
                [defaults.session]
                name_prefix = "work"

                [defaults.window]
                shell_name = "term"
                use_command_name = false
            "#,
        )
        .expect("write config");

        let mut store = SessionStore::with_paths(state_path, config_path, dir.path().join("panes"))
            .expect("create configured store");
        assert_eq!(
            store.handle(CommandRequest::ReloadConfig),
            CommandResponse::ConfigReloaded
        );

        let created = store.handle(CommandRequest::NewSession {
            name: None,
            cwd: None,
            command: Vec::new(),
            switch_from: None,
        });
        assert!(matches!(
            created,
            CommandResponse::SessionCreated { ref session, .. } if session == "work-1"
        ));
        assert!(matches!(
            store.handle(CommandRequest::ListWindows {
                session: "work-1".into(),
            }),
            CommandResponse::WindowList { ref windows }
                if windows.len() == 1 && windows[0].name == "term"
        ));
    }

    #[test]
    fn invalid_reload_keeps_previous_config() {
        let dir = tempdir();
        let state_path = dir.path().join("state.json");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
                [defaults.session]
                name_prefix = "work"
            "#,
        )
        .expect("write config");

        let mut store =
            SessionStore::with_paths(state_path, config_path.clone(), dir.path().join("panes"))
                .expect("create store");
        fs::write(&config_path, "[keys.leader]\ndetach = \"Ctrl-Magic\"\n").expect("overwrite");

        assert!(matches!(
            store.handle(CommandRequest::ReloadConfig),
            CommandResponse::Error { .. }
        ));

        let created = store.handle(CommandRequest::NewSession {
            name: None,
            cwd: None,
            command: Vec::new(),
            switch_from: None,
        });
        assert!(matches!(
            created,
            CommandResponse::SessionCreated { ref session, .. } if session == "work-1"
        ));
    }

    #[test]
    fn configurable_public_numbering_applies_to_sessions_and_targets() {
        let dir = tempdir();
        let state_path = dir.path().join("state.json");
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
                [behavior]
                window_base = 1
                pane_base = 2
            "#,
        )
        .expect("write config");

        let mut store = SessionStore::with_paths(state_path, config_path, dir.path().join("panes"))
            .expect("create store");

        let created = store.handle(CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: None,
            command: vec!["sh".into()],
            switch_from: None,
        });
        assert!(matches!(
            created,
            CommandResponse::SessionCreated {
                session,
                pane_id: 2
            } if session == "work"
        ));

        assert_eq!(
            store.handle(CommandRequest::ListWindows {
                session: "work".into(),
            }),
            CommandResponse::WindowList {
                windows: vec![crate::window::WindowSummary {
                    id: 1,
                    index: 1,
                    name: "sh".into(),
                    active: true,
                    last_selected: false,
                }]
            }
        );

        let split = store.handle(CommandRequest::SplitPane {
            target: "work:1.2".into(),
            axis: SplitAxis::Vertical,
            command: Vec::new(),
        });
        assert!(matches!(
            split,
            CommandResponse::PaneSplit {
                session,
                window_id: 1,
                pane_id: 3
            } if session == "work"
        ));

        assert!(matches!(
            store.handle(CommandRequest::ListPanes {
                target: "work:1".into(),
            }),
            CommandResponse::PaneList { panes }
                if panes.len() == 2
                    && panes.iter().any(|pane| pane.id == 2 && pane.window_id == 1)
                    && panes.iter().any(|pane| pane.id == 3 && pane.window_id == 1)
        ));
    }

    #[test]
    fn effective_command_falls_back_to_shell_when_empty() {
        let store = SessionStore::default();
        let command = store.effective_command(Vec::new());
        assert!(!command.is_empty());
    }
}
