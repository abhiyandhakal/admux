use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    layout::{DividerCell, SplitAxis},
    pane::Rect,
    window::WindowSummary,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion(pub u16);

pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandRequest {
    Hello {
        version: ProtocolVersion,
    },
    NewSession {
        name: Option<String>,
        cwd: Option<PathBuf>,
        command: Vec<String>,
        switch_from: Option<SwitchSource>,
    },
    UpWorkspace {
        manifest_path: PathBuf,
        rebuild: bool,
        switch_from: Option<SwitchSource>,
    },
    SaveWorkspace {
        session: Option<String>,
    },
    Attach {
        session: Option<String>,
    },
    PreviewSession {
        session: String,
    },
    ListSessions,
    ListWindows {
        session: String,
    },
    ListPanes {
        target: String,
    },
    ListBuffers,
    ShowBuffer {
        buffer: Option<String>,
    },
    SetBuffer {
        buffer: Option<String>,
        data: String,
        append: bool,
    },
    DeleteBuffer {
        buffer: Option<String>,
    },
    PasteBuffer {
        target: String,
        buffer: Option<String>,
    },
    SaveBuffer {
        buffer: Option<String>,
        path: PathBuf,
    },
    LoadBuffer {
        path: PathBuf,
        buffer: Option<String>,
    },
    KillSession {
        session: String,
    },
    KillWindow {
        target: String,
    },
    KillPane {
        target: String,
    },
    SendKeys {
        target: String,
        keys: Vec<String>,
    },
    SplitPane {
        target: String,
        axis: SplitAxis,
        command: Vec<String>,
    },
    NewWindow {
        session: String,
        name: Option<String>,
        command: Vec<String>,
    },
    SelectPane {
        target: Option<String>,
        direction: Option<NavigationDirection>,
    },
    SelectWindow {
        target: String,
    },
    CycleWindow {
        session: String,
        direction: CycleDirection,
    },
    ResizePane {
        target: String,
        direction: NavigationDirection,
        amount: u16,
    },
    RenameWindow {
        target: String,
        name: String,
    },
    MouseScroll {
        session: String,
        row: u16,
        col: u16,
        direction: ScrollDirection,
    },
    MousePane {
        session: String,
        pane_id: u64,
        row: u16,
        col: u16,
        kind: PaneMouseKind,
    },
    CopySelection {
        session: String,
        pane_id: Option<u64>,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    },
    ScrollPane {
        session: String,
        pane_id: Option<u64>,
        lines: i16,
    },
    Resize {
        session: String,
        rows: u16,
        cols: u16,
    },
    ReloadConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandResponse {
    HelloAck {
        version: ProtocolVersion,
    },
    SessionCreated {
        session: String,
        pane_id: u64,
    },
    WorkspaceReady {
        session: String,
        created: bool,
    },
    WorkspaceSaved {
        session: String,
        path: PathBuf,
    },
    WindowCreated {
        session: String,
        window_id: u64,
        pane_id: u64,
    },
    PaneSplit {
        session: String,
        window_id: u64,
        pane_id: u64,
    },
    Attached {
        session: String,
        preview: String,
        #[serde(default)]
        formatted_preview: String,
        #[serde(default)]
        formatted_cursor: String,
        #[serde(default)]
        snapshot: Option<RenderSnapshot>,
    },
    SessionPreview {
        snapshot: RenderSnapshot,
    },
    SessionList {
        sessions: Vec<SessionSummary>,
    },
    WindowList {
        windows: Vec<WindowSummary>,
    },
    PaneList {
        panes: Vec<PaneSummary>,
    },
    BufferList {
        buffers: Vec<BufferSummary>,
    },
    BufferShown {
        name: String,
        data: String,
    },
    BufferSet {
        name: String,
    },
    BufferDeleted {
        name: String,
    },
    BufferPasted {
        name: String,
    },
    BufferSaved {
        name: String,
        path: PathBuf,
    },
    BufferLoaded {
        name: String,
    },
    SessionKilled {
        session: String,
    },
    WindowKilled {
        session: String,
        window_id: u64,
    },
    PaneKilled {
        session: String,
        window_id: u64,
        pane_id: u64,
    },
    KeysSent,
    SelectionCopied {
        text: String,
    },
    Scrolled,
    Resized,
    FocusChanged,
    ConfigReloaded,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneMouseKind {
    LeftDown,
    LeftDrag,
    LeftUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CycleDirection {
    Next,
    Prev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchSource {
    pub session: String,
    pub window_id: u64,
    pub pane_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneCursor {
    pub row: u16,
    pub col: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRender {
    pub pane_id: u64,
    pub title: String,
    pub rect: Rect,
    pub focused: bool,
    #[serde(default)]
    pub helper_socket: Option<PathBuf>,
    #[serde(default)]
    pub mouse_reporting: bool,
    pub rows_plain: Vec<String>,
    pub rows_formatted: Vec<String>,
    pub cursor: Option<PaneCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderSnapshot {
    #[serde(default)]
    pub sessions: Vec<SessionSummary>,
    pub windows: Vec<WindowSummary>,
    pub panes: Vec<PaneRender>,
    #[serde(default)]
    pub dividers: Vec<DividerCell>,
    pub active_window_id: u64,
    pub active_pane_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSummary {
    pub id: u64,
    pub title: String,
    pub active: bool,
    pub window_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub name: String,
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferSummary {
    pub name: String,
    pub bytes: usize,
    pub preview: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_roundtrip_preserves_request() {
        let request = CommandRequest::NewSession {
            name: Some("work".into()),
            cwd: Some(PathBuf::from("/tmp")),
            command: vec!["bash".into()],
            switch_from: None,
        };
        let encoded = serde_json::to_vec(&request).expect("encode request");
        let decoded: CommandRequest = serde_json::from_slice(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn protocol_version_is_non_zero() {
        assert!(CURRENT_PROTOCOL_VERSION.0 > 0);
    }

    #[test]
    fn attached_response_accepts_older_payload_without_formatted_preview() {
        let payload = br#"{"Attached":{"session":"work","preview":"plain output"}}"#;
        let decoded: CommandResponse =
            serde_json::from_slice(payload).expect("decode legacy attached response");

        assert_eq!(
            decoded,
            CommandResponse::Attached {
                session: "work".into(),
                preview: "plain output".into(),
                formatted_preview: String::new(),
                formatted_cursor: String::new(),
                snapshot: None,
            }
        );
    }
}
