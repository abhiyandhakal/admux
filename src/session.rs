use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow};

use crate::{
    config::WindowDefaults,
    ipc::{
        NavigationDirection, PaneCursor, PaneRender, PaneSummary, RenderSnapshot, ScrollDirection,
    },
    layout::{Direction, LayoutTree, SplitAxis},
    pane::{PaneId, PaneSnapshot, Rect, WindowId},
    persistence::PersistedSession,
    pty::PaneProcess,
    window::WindowSummary,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionName(pub String);

pub struct Session {
    pub name: String,
    pub cwd: Option<PathBuf>,
    pub command: Vec<String>,
    pub rows: u16,
    pub cols: u16,
    pub windows: BTreeMap<WindowId, WindowRuntime>,
    pub window_order: Vec<WindowId>,
    pub active_window: WindowId,
    pub last_window: Option<WindowId>,
    pub default_shell: Option<String>,
    pub scrollback_lines: usize,
    pub window_defaults: WindowDefaults,
    pub helper_dir: PathBuf,
}

pub struct WindowRuntime {
    pub id: WindowId,
    pub name: String,
    pub layout: LayoutTree,
    pub next_pane_id: u64,
    pub panes: BTreeMap<PaneId, PaneRuntime>,
}

pub struct PaneRuntime {
    pub id: PaneId,
    pub title: String,
    pub process: PaneProcess,
}

pub struct SplitResult {
    pub window_id: WindowId,
    pub pane_id: PaneId,
}

pub struct WindowCreation {
    pub window_id: WindowId,
    pub pane_id: PaneId,
}

pub struct KillResult {
    pub window_id: WindowId,
    pub pane_id: PaneId,
}

impl Session {
    pub fn new(
        name: String,
        cwd: Option<PathBuf>,
        command: Vec<String>,
        window_id: WindowId,
        default_shell: Option<String>,
        scrollback_lines: usize,
        window_defaults: WindowDefaults,
        helper_dir: PathBuf,
    ) -> Result<Self> {
        let mut windows = BTreeMap::new();
        let initial_name = default_window_name(&command, &window_defaults);
        windows.insert(
            window_id,
            WindowRuntime::new(
                window_id,
                initial_name,
                cwd.clone(),
                &command,
                &name,
                default_shell.as_deref(),
                scrollback_lines,
                &window_defaults,
                &helper_dir,
            )?,
        );

        let mut session = Self {
            name,
            cwd,
            command,
            rows: 24,
            cols: 80,
            windows,
            window_order: vec![window_id],
            active_window: window_id,
            last_window: None,
            default_shell,
            scrollback_lines,
            window_defaults,
            helper_dir,
        };
        session.sync_pane_sizes()?;
        Ok(session)
    }

    pub fn from_persisted(
        persisted: &PersistedSession,
        default_shell: Option<String>,
        scrollback_lines: usize,
        window_defaults: WindowDefaults,
        helper_dir: PathBuf,
    ) -> Result<Self> {
        let mut windows = BTreeMap::new();
        for window_id in &persisted.window_order {
            let persisted_window = persisted
                .windows
                .get(window_id)
                .ok_or_else(|| anyhow!("missing persisted window {}", window_id.0))?;
            let mut panes = BTreeMap::new();
            for pane_id in persisted_window.layout.panes() {
                let persisted_pane = persisted_window
                    .panes
                    .get(&pane_id)
                    .ok_or_else(|| anyhow!("missing persisted pane {}", pane_id.0))?;
                let socket_path = persisted_pane
                    .socket_path
                    .clone()
                    .ok_or_else(|| anyhow!("persisted pane {} has no helper socket", pane_id.0))?;
                panes.insert(
                    pane_id,
                    PaneRuntime {
                        id: pane_id,
                        title: persisted_pane.title.clone(),
                        process: PaneProcess::connect(socket_path)?,
                    },
                );
            }
            windows.insert(
                *window_id,
                WindowRuntime {
                    id: persisted_window.id,
                    name: persisted_window.name.clone(),
                    layout: persisted_window.layout.clone(),
                    next_pane_id: persisted_window.next_pane_id,
                    panes,
                },
            );
        }

        let mut session = Self {
            name: persisted.name.clone(),
            cwd: persisted.cwd.clone(),
            command: persisted.command.clone(),
            rows: persisted.rows,
            cols: persisted.cols,
            windows,
            window_order: persisted.window_order.clone(),
            active_window: persisted.active_window,
            last_window: persisted.last_window,
            default_shell,
            scrollback_lines,
            window_defaults,
            helper_dir,
        };
        session.sync_pane_sizes()?;
        Ok(session)
    }

    pub fn active_window(&self) -> Option<&WindowRuntime> {
        self.windows.get(&self.active_window)
    }

    pub fn active_window_mut(&mut self) -> Option<&mut WindowRuntime> {
        self.windows.get_mut(&self.active_window)
    }

    pub fn active_pane_preview(&self) -> String {
        self.active_window()
            .and_then(|window| window.active_pane())
            .map(|pane| pane.process.preview())
            .unwrap_or_default()
    }

    pub fn active_pane_formatted_preview(&self) -> String {
        self.active_window()
            .and_then(|window| window.active_pane())
            .map(|pane| pane.process.formatted_preview())
            .unwrap_or_default()
    }

    pub fn active_pane_formatted_cursor(&self) -> String {
        self.active_window()
            .and_then(|window| window.active_pane())
            .map(|pane| pane.process.formatted_cursor())
            .unwrap_or_default()
    }

    pub fn active_pane_selection_text(
        &self,
        pane_id: Option<PaneId>,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    ) -> String {
        self.active_window()
            .and_then(|window| {
                let pane_id = pane_id.unwrap_or(window.layout.active);
                window.panes.get(&pane_id)
            })
            .map(|pane| {
                pane.process
                    .selection_text(start_row, start_col, end_row, end_col)
            })
            .unwrap_or_default()
    }

    pub fn active_pane_snapshot(&self) -> Option<PaneSnapshot> {
        self.active_window()
            .and_then(|window| window.active_pane())
            .map(|pane| PaneSnapshot {
                id: pane.id,
                title: pane.title.clone(),
                preview: pane.process.preview(),
            })
    }

    pub fn render_snapshot(&self, size: Rect) -> Option<RenderSnapshot> {
        let window = self.active_window()?;
        let rects = window.layout.pane_rects(size);
        let panes = window
            .layout
            .panes()
            .into_iter()
            .filter_map(|pane_id| {
                let pane = window.panes.get(&pane_id)?;
                let rect = *rects.get(&pane_id)?;
                let render = pane.process.render(rect.width, rect.height).ok()?;
                let cursor = clamp_cursor(rect, render.cursor_row, render.cursor_col);

                Some(PaneRender {
                    pane_id: pane.id.0,
                    title: pane.title.clone(),
                    rect,
                    focused: pane_id == window.layout.active,
                    rows_plain: render.rows_plain,
                    rows_formatted: render.rows_formatted,
                    cursor,
                })
            })
            .collect();

        Some(RenderSnapshot {
            sessions: Vec::new(),
            windows: self.list_windows(),
            panes,
            dividers: window.layout.divider_cells(size),
            active_window_id: window.id.0,
            active_pane_id: window.layout.active.0,
        })
    }

    pub fn list_windows(&self) -> Vec<WindowSummary> {
        self.window_order
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                let window = self.windows.get(id)?;
                Some(WindowSummary::new(
                    *id,
                    index,
                    window.name.clone(),
                    *id == self.active_window,
                    Some(*id) == self.last_window,
                ))
            })
            .collect()
    }

    pub fn list_panes(&self, window_id: Option<WindowId>) -> Vec<PaneSummary> {
        let window_id = window_id.unwrap_or(self.active_window);
        self.windows
            .get(&window_id)
            .map(|window| {
                window
                    .layout
                    .panes()
                    .into_iter()
                    .map(|pane_id| PaneSummary {
                        id: pane_id.0,
                        title: window
                            .panes
                            .get(&pane_id)
                            .map(|pane| pane.title.clone())
                            .unwrap_or_else(|| "pane".into()),
                        active: pane_id == window.layout.active,
                        window_id: window.id.0,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn contains_pane(&self, pane_id: PaneId) -> bool {
        self.active_window()
            .is_some_and(|window| window.panes.contains_key(&pane_id))
    }

    pub fn contains_window_pane(&self, window_id: WindowId, pane_id: PaneId) -> bool {
        self.windows
            .get(&window_id)
            .is_some_and(|window| window.panes.contains_key(&pane_id))
    }

    pub fn send_keys(
        &self,
        window_id: Option<WindowId>,
        pane_id: Option<PaneId>,
        keys: &[String],
    ) -> Result<()> {
        let window = self
            .window(window_id)
            .ok_or_else(|| anyhow!("unknown window"))?;
        let pane_id = pane_id.unwrap_or(window.layout.active);
        let pane = window
            .panes
            .get(&pane_id)
            .ok_or_else(|| anyhow!("unknown pane"))?;
        pane.process.send_keys(keys)?;
        Ok(())
    }

    pub fn set_viewport(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        self.sync_pane_sizes()
    }

    pub fn pane_area(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: self.cols.max(1),
            height: self.rows.saturating_sub(1).max(1),
        }
    }

    pub fn handle_mouse_scroll(
        &self,
        pane_id: Option<PaneId>,
        direction: ScrollDirection,
        row: u16,
        col: u16,
    ) -> Result<()> {
        let window = self
            .active_window()
            .ok_or_else(|| anyhow!("unknown window"))?;
        let pane_id = pane_id.unwrap_or(window.layout.active);
        let pane = window
            .panes
            .get(&pane_id)
            .ok_or_else(|| anyhow!("unknown pane"))?;
        pane.process.handle_mouse_scroll(direction, row, col)?;
        Ok(())
    }

    pub fn scroll_pane(&self, pane_id: Option<PaneId>, lines: i16) -> Result<()> {
        let window = self
            .active_window()
            .ok_or_else(|| anyhow!("unknown window"))?;
        let pane_id = pane_id.unwrap_or(window.layout.active);
        let pane = window
            .panes
            .get(&pane_id)
            .ok_or_else(|| anyhow!("unknown pane"))?;
        pane.process.scroll_scrollback_by(lines);
        Ok(())
    }

    pub fn split_active_pane(
        &mut self,
        axis: SplitAxis,
        command: &[String],
    ) -> Result<SplitResult> {
        let cwd = self.cwd.clone();
        let default_command = if command.is_empty() {
            self.command.clone()
        } else {
            command.to_vec()
        };
        let active_window = self.active_window;
        let window = self
            .windows
            .get_mut(&active_window)
            .ok_or_else(|| anyhow!("unknown window"))?;
        let pane_id = window.allocate_pane_id();
        let process = PaneProcess::spawn(
            &default_command,
            cwd.as_deref(),
            Some((&self.name, active_window, pane_id)),
            self.default_shell.as_deref(),
            self.scrollback_lines,
            &self.helper_dir,
        )?;
        let pane = PaneRuntime {
            id: pane_id,
            title: default_window_name(&default_command, &self.window_defaults),
            process,
        };
        window.panes.insert(pane_id, pane);
        window.layout.split_active(axis, pane_id);
        self.sync_pane_sizes()?;
        Ok(SplitResult {
            window_id: active_window,
            pane_id,
        })
    }

    pub fn new_window(
        &mut self,
        window_id: WindowId,
        name: Option<String>,
        command: &[String],
    ) -> Result<WindowCreation> {
        let command = if command.is_empty() {
            self.command.clone()
        } else {
            command.to_vec()
        };
        let window = WindowRuntime::new(
            window_id,
            name.unwrap_or_else(|| default_window_name(&command, &self.window_defaults)),
            self.cwd.clone(),
            &command,
            &self.name,
            self.default_shell.as_deref(),
            self.scrollback_lines,
            &self.window_defaults,
            &self.helper_dir,
        )?;
        let pane_id = window.layout.active;
        self.windows.insert(window_id, window);
        self.window_order.push(window_id);
        self.remember_window_switch(window_id);
        self.sync_pane_sizes()?;
        Ok(WindowCreation { window_id, pane_id })
    }

    pub fn select_pane(&mut self, window_id: Option<WindowId>, pane_id: PaneId) -> Result<()> {
        let window = self
            .window_mut(window_id)
            .ok_or_else(|| anyhow!("unknown window"))?;
        if window.panes.contains_key(&pane_id) {
            window.layout.active = pane_id;
            Ok(())
        } else {
            Err(anyhow!("unknown pane"))
        }
    }

    pub fn move_focus(&mut self, direction: NavigationDirection, area: Rect) -> Result<()> {
        let window = self
            .active_window_mut()
            .ok_or_else(|| anyhow!("unknown window"))?;
        let _ = window
            .layout
            .select_direction(convert_direction(direction), area);
        Ok(())
    }

    pub fn resize_active_pane(
        &mut self,
        window_id: Option<WindowId>,
        pane_id: Option<PaneId>,
        direction: NavigationDirection,
        amount: u16,
    ) -> Result<()> {
        let window = self
            .window_mut(window_id)
            .ok_or_else(|| anyhow!("unknown window"))?;
        if let Some(pane_id) = pane_id {
            window.layout.active = pane_id;
        }
        let _ = window
            .layout
            .resize_active(convert_direction(direction), amount);
        self.sync_pane_sizes()
    }

    pub fn select_window(&mut self, window_id: WindowId) -> Result<()> {
        if self.windows.contains_key(&window_id) {
            self.remember_window_switch(window_id);
            Ok(())
        } else {
            Err(anyhow!("unknown window"))
        }
    }

    pub fn cycle_window(&mut self, next: bool) -> Result<()> {
        let current = self
            .window_order
            .iter()
            .position(|id| *id == self.active_window)
            .ok_or_else(|| anyhow!("unknown window"))?;
        let len = self.window_order.len();
        let next_index = if next {
            (current + 1) % len
        } else {
            (current + len - 1) % len
        };
        self.remember_window_switch(self.window_order[next_index]);
        Ok(())
    }

    pub fn kill_pane(
        &mut self,
        window_id: Option<WindowId>,
        pane_id: Option<PaneId>,
    ) -> Result<Option<KillResult>> {
        let window_id = window_id.unwrap_or(self.active_window);
        let window = match self.windows.get_mut(&window_id) {
            Some(window) => window,
            None => return Err(anyhow!("unknown window")),
        };
        let pane_id = pane_id.unwrap_or(window.layout.active);
        let pane = window
            .panes
            .remove(&pane_id)
            .ok_or_else(|| anyhow!("unknown pane"))?;
        pane.process.kill()?;
        if window.panes.is_empty() {
            self.windows.remove(&window_id);
            self.window_order.retain(|id| *id != window_id);
            if let Some(next_window) = self.window_order.last().copied() {
                self.remember_window_after_removal(window_id, next_window);
            }
            return Ok(None);
        }
        let _ = window.layout.remove_pane(pane_id);
        self.sync_pane_sizes()?;
        Ok(Some(KillResult { window_id, pane_id }))
    }

    pub fn kill_window(&mut self, window_id: WindowId) -> Result<bool> {
        let window = self
            .windows
            .remove(&window_id)
            .ok_or_else(|| anyhow!("unknown window"))?;
        for pane in window.panes.into_values() {
            pane.process.kill()?;
        }
        self.window_order.retain(|id| *id != window_id);
        if let Some(next_window) = self.window_order.last().copied() {
            self.remember_window_after_removal(window_id, next_window);
        }
        if !self.window_order.is_empty() {
            self.sync_pane_sizes()?;
        }
        Ok(!self.window_order.is_empty())
    }

    pub fn rename_active_window(&mut self, name: String) -> Result<()> {
        let window = self
            .active_window_mut()
            .ok_or_else(|| anyhow!("unknown window"))?;
        window.name = name;
        Ok(())
    }

    pub fn kill(self) -> Result<()> {
        for window in self.windows.into_values() {
            for pane in window.panes.into_values() {
                pane.process.kill()?;
            }
        }
        Ok(())
    }

    pub fn is_alive(&self) -> bool {
        self.windows.values().any(WindowRuntime::is_alive)
    }

    pub fn prune_dead(&mut self) -> bool {
        let mut changed = false;
        let mut empty_windows = Vec::new();
        for (window_id, window) in &mut self.windows {
            let before = window.panes.len();
            window.prune_dead();
            changed |= before != window.panes.len();
            if window.panes.is_empty() {
                empty_windows.push(*window_id);
            }
        }

        for window_id in empty_windows {
            self.windows.remove(&window_id);
            self.window_order.retain(|id| *id != window_id);
            changed = true;
        }

        if !self.windows.contains_key(&self.active_window)
            && let Some(window_id) = self.window_order.last().copied()
        {
            self.remember_window_after_removal(self.active_window, window_id);
            changed = true;
        }
        if self
            .last_window
            .is_some_and(|window_id| !self.windows.contains_key(&window_id))
        {
            self.last_window = None;
            changed = true;
        }

        if changed && !self.window_order.is_empty() {
            let _ = self.sync_pane_sizes();
        }

        !self.window_order.is_empty()
    }

    pub fn window(&self, window_id: Option<WindowId>) -> Option<&WindowRuntime> {
        self.windows.get(&window_id.unwrap_or(self.active_window))
    }

    pub fn window_mut(&mut self, window_id: Option<WindowId>) -> Option<&mut WindowRuntime> {
        self.windows
            .get_mut(&window_id.unwrap_or(self.active_window))
    }

    fn sync_pane_sizes(&mut self) -> Result<()> {
        let area = self.pane_area();
        for window in self.windows.values() {
            let rects = window.layout.pane_rects(area);
            for (pane_id, pane) in &window.panes {
                let rect = rects.get(pane_id).copied().unwrap_or(area);
                pane.process.resize(rect.height.max(1), rect.width.max(1))?;
            }
        }
        Ok(())
    }

    fn remember_window_switch(&mut self, next_window: WindowId) {
        if self.active_window != next_window {
            self.last_window = Some(self.active_window);
            self.active_window = next_window;
        }
    }

    fn remember_window_after_removal(&mut self, removed_window: WindowId, next_window: WindowId) {
        self.active_window = next_window;
        self.last_window = self
            .last_window
            .filter(|window_id| *window_id != removed_window && *window_id != next_window);
    }
}

impl WindowRuntime {
    fn new(
        id: WindowId,
        name: String,
        cwd: Option<PathBuf>,
        command: &[String],
        session_name: &str,
        default_shell: Option<&str>,
        scrollback_lines: usize,
        window_defaults: &WindowDefaults,
        helper_dir: &Path,
    ) -> Result<Self> {
        let pane_id = PaneId(0);
        let process = PaneProcess::spawn(
            command,
            cwd.as_deref(),
            Some((session_name, id, pane_id)),
            default_shell,
            scrollback_lines,
            helper_dir,
        )?;
        let pane = PaneRuntime {
            id: pane_id,
            title: default_window_name(command, window_defaults),
            process,
        };
        let mut panes = BTreeMap::new();
        panes.insert(pane_id, pane);

        Ok(Self {
            id,
            name,
            layout: LayoutTree::new(pane_id),
            next_pane_id: 1,
            panes,
        })
    }

    fn allocate_pane_id(&mut self) -> PaneId {
        let pane_id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        pane_id
    }

    fn active_pane(&self) -> Option<&PaneRuntime> {
        self.panes.get(&self.layout.active)
    }

    fn prune_dead(&mut self) {
        let dead: Vec<_> = self
            .panes
            .iter()
            .filter_map(|(pane_id, pane)| (!pane.process.is_alive()).then_some(*pane_id))
            .collect();
        for pane_id in dead {
            self.panes.remove(&pane_id);
            if !self.panes.is_empty() {
                let _ = self.layout.remove_pane(pane_id);
            }
        }
    }

    fn is_alive(&self) -> bool {
        self.panes.values().any(|pane| pane.process.is_alive())
    }
}

fn default_window_name(command: &[String], defaults: &WindowDefaults) -> String {
    if !defaults.use_command_name {
        return defaults.shell_name.clone();
    }
    command
        .first()
        .and_then(|part| part.rsplit('/').next())
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| defaults.shell_name.clone())
}

fn convert_direction(direction: NavigationDirection) -> Direction {
    match direction {
        NavigationDirection::Left => Direction::Left,
        NavigationDirection::Right => Direction::Right,
        NavigationDirection::Up => Direction::Up,
        NavigationDirection::Down => Direction::Down,
    }
}

fn clamp_cursor(content: Rect, row: u16, col: u16) -> Option<PaneCursor> {
    if content.width == 0 || content.height == 0 {
        return None;
    }

    Some(PaneCursor {
        row: row.min(content.height.saturating_sub(1)),
        col: col.min(content.width.saturating_sub(1)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn new_window_uses_full_viewport_width_for_pty() {
        let helper_dir = tempdir().expect("tempdir");
        let mut session = Session::new(
            "work".into(),
            None,
            vec!["sh".into()],
            WindowId(1),
            None,
            10_000,
            WindowDefaults::default(),
            helper_dir.path().to_path_buf(),
        )
        .expect("create session");
        session.set_viewport(30, 120).expect("set viewport");

        let created = session
            .new_window(WindowId(2), Some("logs".into()), &["sh".into()])
            .expect("create window");

        let window = session.windows.get(&created.window_id).expect("window");
        let pane = window.panes.get(&created.pane_id).expect("pane");
        let (rows, cols) = pane.process.screen_size();

        assert_eq!(rows, 29);
        assert_eq!(cols, 120);
    }

    #[test]
    fn cursor_is_clamped_into_pane_content() {
        let cursor = clamp_cursor(
            Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 5,
            },
            20,
            20,
        )
        .expect("cursor");

        assert_eq!(cursor.row, 4);
        assert_eq!(cursor.col, 9);
    }

    #[test]
    fn selecting_window_tracks_last_window() {
        let helper_dir = tempdir().expect("tempdir");
        let mut session = Session::new(
            "work".into(),
            None,
            vec!["sh".into()],
            WindowId(1),
            None,
            10_000,
            WindowDefaults::default(),
            helper_dir.path().to_path_buf(),
        )
        .expect("create session");

        session
            .new_window(WindowId(2), Some("logs".into()), &["sh".into()])
            .expect("create window");
        session.select_window(WindowId(1)).expect("select first");

        assert_eq!(session.active_window, WindowId(1));
        assert_eq!(session.last_window, Some(WindowId(2)));

        let windows = session.list_windows();
        assert!(windows.iter().any(|window| window.active && window.id == 1));
        assert!(
            windows
                .iter()
                .any(|window| window.last_selected && window.id == 2)
        );
    }

    #[test]
    fn pane_ids_are_window_local_and_stable() {
        let helper_dir = tempdir().expect("tempdir");
        let mut session = Session::new(
            "work".into(),
            None,
            vec!["sh".into()],
            WindowId(1),
            None,
            10_000,
            WindowDefaults::default(),
            helper_dir.path().to_path_buf(),
        )
        .expect("create session");

        let first_window_panes = session.list_panes(Some(WindowId(1)));
        assert_eq!(first_window_panes[0].id, 0);

        let split = session
            .split_active_pane(SplitAxis::Vertical, &["sh".into()])
            .expect("split pane");
        assert_eq!(split.pane_id, PaneId(1));

        let second_window = session
            .new_window(WindowId(2), Some("logs".into()), &["sh".into()])
            .expect("create window");
        assert_eq!(second_window.pane_id, PaneId(0));

        session.select_window(WindowId(1)).expect("back to first");
        session
            .kill_pane(Some(WindowId(1)), Some(PaneId(0)))
            .expect("kill pane");
        let remaining = session.list_panes(Some(WindowId(1)));
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, 1);
    }
}
